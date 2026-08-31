//! X-Y マイクステージ — 楽器とマイクの俯瞰図 (承認済みデザイン)。
//!
//! - 上部: 響板の台形 (木の色) と 2 本のブリッジ
//! - マイクノードを**縦にドラッグ**すると Mic Distance (0.3–3.0 m)
//! - ノードから開く 2 本のアームの**ハンドルをドラッグ**すると
//!   X-Y Angle (60–135 deg)
//!
//! 座標⇔値の変換は純粋関数に切り出してテストする。パラメータの真実は
//! 常にホスト側 (アトミック) にあり、ここは表示と操作だけを受け持つ。

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::theme;

/// 操作の結果。`Some` はドラッグでの変更。
pub struct MicStageResponse {
    pub mic_distance_m: Option<f64>,
    pub xy_angle_deg: Option<f64>,
}

/// パラメータの範囲 (プラグイン側 spec と同じ値)。
pub const DISTANCE_RANGE: (f64, f64) = (0.3, 3.0);
pub const ANGLE_RANGE: (f64, f64) = (60.0, 135.0);

/// マイクの可動域 (ステージ矩形に対する比)。上端 = 楽器の手前、下端 = 3 m。
const MIC_TOP: f32 = 0.40;
const MIC_BOTTOM: f32 = 0.92;
/// アームの長さ [px]。
const ARM_LEN: f32 = 34.0;

/// 距離 [m] → ステージ内の y。
pub fn y_for_distance(distance_m: f64, rect: Rect) -> f32 {
    let (lo, hi) = DISTANCE_RANGE;
    let t = ((distance_m - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    rect.top() + rect.height() * (MIC_TOP + (MIC_BOTTOM - MIC_TOP) * t)
}

/// ステージ内の y → 距離 [m] (クランプ込み)。
pub fn distance_for_y(y: f32, rect: Rect) -> f64 {
    let top = rect.top() + rect.height() * MIC_TOP;
    let bottom = rect.top() + rect.height() * MIC_BOTTOM;
    let t = ((y - top) / (bottom - top).max(1.0)).clamp(0.0, 1.0) as f64;
    let (lo, hi) = DISTANCE_RANGE;
    lo + (hi - lo) * t
}

/// ノードから見たポインタ位置 → 開き角 [deg] (クランプ込み)。
///
/// アームは真上 (楽器の方向) を中心に左右対称に開くので、垂直からの
/// 半角 × 2 が開き角。
pub fn angle_for_pointer(node: Pos2, pos: Pos2) -> f64 {
    let dx = (pos.x - node.x).abs();
    let dy = node.y - pos.y; // 上向きが正
    let half = f64::from(dx).atan2(f64::from(dy.max(1.0))).to_degrees();
    let (lo, hi) = ANGLE_RANGE;
    (half * 2.0).clamp(lo, hi)
}

/// ステージを描いて操作を返す。
pub fn mic_stage(ui: &mut Ui, distance_m: f64, angle_deg: f64, room_on: bool) -> MicStageResponse {
    let width = ui.available_width();
    let height = ui.available_height().max(160.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);

    let dim = |c: Color32| -> Color32 {
        if room_on {
            c
        } else {
            // Room off: ステージ全体を沈ませる (操作は生かす)。
            c.gamma_multiply(0.45)
        }
    };

    painter.rect_filled(rect, 3.0, theme::BG);
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );

    // 楽器 (台形の響板)。上辺が奥。
    let inst_top = rect.top() + rect.height() * 0.07;
    let inst_bottom = rect.top() + rect.height() * 0.32;
    let cx = rect.center().x;
    let half_top = rect.width() * 0.33;
    let half_bottom = rect.width() * 0.21;
    let corners = [
        Pos2::new(cx - half_top, inst_top),
        Pos2::new(cx + half_top, inst_top),
        Pos2::new(cx + half_bottom, inst_bottom),
        Pos2::new(cx - half_bottom, inst_bottom),
    ];
    painter.add(egui::Shape::convex_polygon(
        corners.to_vec(),
        dim(theme::WOOD_DARK),
        Stroke::new(1.0, dim(theme::WOOD_LIGHT)),
    ));
    // ブリッジ 2 本 (バス / トレブル)。
    let bridge = |t: f32, lean: f32| {
        let x_top = cx + half_top * t;
        let x_bottom = cx + half_bottom * (t + lean);
        painter.line_segment(
            [
                Pos2::new(x_top, inst_top + 4.0),
                Pos2::new(x_bottom, inst_bottom - 4.0),
            ],
            Stroke::new(2.0, dim(theme::HIGHLIGHT)),
        );
    };
    bridge(-0.45, -0.15);
    bridge(0.40, 0.35);

    // 距離ガイド (縦の点線) と目盛り。
    let node = Pos2::new(cx, y_for_distance(distance_m, rect));
    painter.add(egui::Shape::dashed_line(
        &[
            Pos2::new(cx, inst_bottom),
            Pos2::new(cx, rect.top() + rect.height() * MIC_BOTTOM),
        ],
        Stroke::new(1.0, dim(theme::FAINT)),
        4.0,
        4.0,
    ));
    painter.text(
        Pos2::new(cx + 10.0, (inst_bottom + node.y) * 0.5),
        egui::Align2::LEFT_CENTER,
        format!("{distance_m:.2} m"),
        egui::FontId::monospace(10.0),
        dim(theme::AQUA),
    );

    // X-Y のアーム 2 本とハンドル。
    let half = (angle_deg * 0.5).to_radians();
    let mut handles = [Pos2::ZERO; 2];
    for (i, sign) in [-1.0f32, 1.0].iter().enumerate() {
        let dir = Vec2::new(sign * half.sin() as f32, -(half.cos() as f32));
        let tip = node + dir * ARM_LEN;
        handles[i] = tip;
        painter.line_segment([node, tip], Stroke::new(2.0, dim(theme::TEXT)));
        painter.circle_filled(tip, 4.0, dim(theme::AQUA));
    }
    painter.text(
        Pos2::new(node.x + ARM_LEN + 8.0, node.y - ARM_LEN * 0.5),
        egui::Align2::LEFT_CENTER,
        format!("{angle_deg:.0} deg"),
        egui::FontId::monospace(10.0),
        dim(theme::AQUA),
    );
    // マイクノード。
    painter.circle_filled(node, 7.0, dim(theme::ACCENT));
    painter.circle_stroke(
        node,
        11.0,
        Stroke::new(1.0, dim(theme::ACCENT).gamma_multiply(0.5)),
    );

    painter.text(
        Pos2::new(rect.left() + 8.0, rect.bottom() - 10.0),
        egui::Align2::LEFT_CENTER,
        "top view - drag mic: distance, drag arms: angle",
        egui::FontId::proportional(8.5),
        theme::FAINT,
    );

    // 操作: ノード (距離) とハンドル (角度) を別々の interact で受ける。
    let mut out = MicStageResponse {
        mic_distance_m: None,
        xy_angle_deg: None,
    };
    let node_zone = Rect::from_center_size(node, Vec2::splat(28.0));
    let node_resp = ui.interact(node_zone, ui.id().with("mic_node"), Sense::drag());
    if node_resp.dragged() {
        if let Some(pos) = node_resp.interact_pointer_pos() {
            out.mic_distance_m = Some(distance_for_y(pos.y, rect));
        }
    }
    for (i, tip) in handles.iter().enumerate() {
        let zone = Rect::from_center_size(*tip, Vec2::splat(20.0));
        let resp = ui.interact(zone, ui.id().with(("mic_arm", i)), Sense::drag());
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                out.xy_angle_deg = Some(angle_for_pointer(node, pos));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 260.0))
    }

    #[test]
    fn distance_round_trips_through_the_stage() {
        let rect = stage();
        for d in [0.3f64, 1.2, 2.0, 3.0] {
            let back = distance_for_y(y_for_distance(d, rect), rect);
            assert!((back - d).abs() < 0.02, "{d} -> {back}");
        }
    }

    #[test]
    fn distance_clamps_to_its_range() {
        let rect = stage();
        assert!((distance_for_y(rect.top() - 100.0, rect) - 0.3).abs() < 1e-9);
        assert!((distance_for_y(rect.bottom() + 100.0, rect) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_angle_follows_the_pointer_and_clamps() {
        let node = Pos2::new(100.0, 100.0);
        // 真横に近いポインタは上限 135 度でクランプ。
        assert!((angle_for_pointer(node, Pos2::new(200.0, 100.0)) - 135.0).abs() < 1e-9);
        // ほぼ真上は下限 60 度でクランプ。
        assert!((angle_for_pointer(node, Pos2::new(100.5, 20.0)) - 60.0).abs() < 1e-9);
        // 45 度の対角 (半角 45) は開き 90 度。
        let a = angle_for_pointer(node, Pos2::new(160.0, 40.0));
        assert!((a - 90.0).abs() < 1.0, "{a}");
    }
}
