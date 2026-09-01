//! 鍵盤ウィジェット — 鳴る鍵の色分けと打鍵グロー (承認済みデザイン)。
//!
//! - D#2 (39) 〜 E6 (88) のフルクロマチック 50 鍵 (白鍵 29 本) を常に描く
//!   (下端の 4 鍵は半音階の低音弦ブロックのぶん。15/14 では暗く沈む)
//! - 鍵下端のバンド色 = その鍵を鳴らすブリッジ
//!   (バス = 赤 / トレブル右 = 橙 / トレブル左 = 黄 — 暖色の類似色ランプ)
//! - 配置に無い鍵は暗く沈める (描かないのではなく「無い」ことを見せる)
//! - 打鍵中の鍵はウォームホワイトのグローで光る (減衰はエディタ側が管理)
//! - クリックで発音。**velocity は鍵の中のクリック高さ** (下ほど強い)
//!
//! ジオメトリ (どの鍵がどこか) は純粋関数に切り出してテストする。

use egui::{Color32, Pos2, Rect, Sense, Ui, Vec2};
use phydulcimer_core::layout::{BridgeSide, Layout};

use crate::theme;
use crate::{KEY_MAX, KEY_MIN};

/// 白鍵の本数 (39..=88)。
pub const WHITE_COUNT: usize = 29;

/// 黒鍵の白鍵に対する幅・高さの比。
const BLACK_W: f32 = 0.62;
const BLACK_H: f32 = 0.58;

/// 鍵盤の推奨高さ [px]。
pub const KEYBOARD_HEIGHT: f32 = 112.0;

/// クリックの結果。
pub struct KeyboardResponse {
    /// クリックで鳴らす鍵と velocity (0.2–1.0)。
    pub pressed: Option<(u8, f32)>,
}

/// 黒鍵か。
pub fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// `KEY_MIN..key` にある白鍵の本数 (= この鍵の白鍵インデックス)。
pub fn white_index(key: u8) -> usize {
    (KEY_MIN..key).filter(|&k| !is_black(k)).count()
}

/// 鍵の矩形。`rect` は鍵盤全体の領域。
pub fn key_rect(key: u8, rect: Rect) -> Rect {
    let ww = rect.width() / WHITE_COUNT as f32;
    if is_black(key) {
        // 黒鍵は直前の白鍵と直後の白鍵の境界にまたがる。
        let boundary = rect.left() + white_index(key) as f32 * ww;
        Rect::from_min_size(
            Pos2::new(boundary - ww * BLACK_W * 0.5, rect.top()),
            Vec2::new(ww * BLACK_W, rect.height() * BLACK_H),
        )
    } else {
        Rect::from_min_size(
            Pos2::new(rect.left() + white_index(key) as f32 * ww, rect.top()),
            Vec2::new(ww, rect.height()),
        )
    }
}

/// ポインタ位置 → 鍵。黒鍵が白鍵に重なるので黒鍵を先に見る。
pub fn key_at(pos: Pos2, rect: Rect) -> Option<u8> {
    if !rect.contains(pos) {
        return None;
    }
    for key in KEY_MIN..=KEY_MAX {
        if is_black(key) && key_rect(key, rect).contains(pos) {
            return Some(key);
        }
    }
    (KEY_MIN..=KEY_MAX).find(|&key| !is_black(key) && key_rect(key, rect).contains(pos))
}

/// 鍵の中のクリック高さ → velocity。上端 0.2、下端 1.0 (下ほど強い)。
pub fn velocity_at(pos: Pos2, key: u8, rect: Rect) -> f32 {
    let kr = key_rect(key, rect);
    let t = ((pos.y - kr.top()) / kr.height().max(1.0)).clamp(0.0, 1.0);
    0.2 + 0.8 * t
}

/// その鍵を鳴らすブリッジのバンド色。配置に無ければ `None`。
pub fn bank_color(layout: &Layout, key: u8) -> Option<Color32> {
    let idx = layout.preferred_index(key)?;
    Some(match layout.positions()[idx].side {
        BridgeSide::Bass => theme::BANK_BASS,
        BridgeSide::TrebleRight => theme::BANK_TREBLE_R,
        BridgeSide::TrebleLeft => theme::BANK_TREBLE_L,
    })
}

/// グロー量 (0–1) を面の色に混ぜる。
fn with_glow(base: Color32, glow: f32) -> Color32 {
    if glow <= 0.0 {
        return base;
    }
    let t = glow.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t) as u8 };
    Color32::from_rgb(
        mix(base.r(), theme::GLOW.r()),
        mix(base.g(), theme::GLOW.g()),
        mix(base.b(), theme::GLOW.b()),
    )
}

/// 鍵盤を描いてクリックを返す。`glow(key)` は 0–1 のグロー量。
pub fn keyboard(ui: &mut Ui, layout: &Layout, glow: &dyn Fn(u8) -> f32) -> KeyboardResponse {
    let width = ui.available_width();
    let (outer, response) =
        ui.allocate_exact_size(Vec2::new(width, KEYBOARD_HEIGHT), Sense::click());
    let painter = ui.painter_at(outer);

    // 背板。鍵の隙間 1px がこの色で見える。
    painter.rect_filled(outer, 3.0, theme::KEY_GAP);
    let rect = outer.shrink(2.0);

    // 白鍵 → 黒鍵の順に重ねる。
    for key in KEY_MIN..=KEY_MAX {
        if is_black(key) {
            continue;
        }
        let kr = key_rect(key, rect).shrink2(Vec2::new(0.5, 0.0));
        let bank = bank_color(layout, key);
        let base = if bank.is_some() {
            theme::KEY_WHITE
        } else {
            theme::KEY_WHITE_OFF
        };
        painter.rect_filled(kr, 2.0, with_glow(base, glow(key)));
        if let Some(color) = bank {
            let band = Rect::from_min_max(
                Pos2::new(kr.left() + 2.0, kr.bottom() - 7.0),
                Pos2::new(kr.right() - 2.0, kr.bottom() - 2.0),
            );
            painter.rect_filled(band, 1.5, color);
        }
        // C の鍵と両端だけ音名を添える。
        if key % 12 == 0 || key == KEY_MIN || key == KEY_MAX {
            painter.text(
                Pos2::new(kr.center().x, kr.bottom() - 14.0),
                egui::Align2::CENTER_CENTER,
                phydulcimer_core::layout::note_name(key),
                egui::FontId::proportional(8.0),
                if bank.is_some() {
                    theme::DIM
                } else {
                    theme::FAINT
                },
            );
        }
    }
    for key in KEY_MIN..=KEY_MAX {
        if !is_black(key) {
            continue;
        }
        let kr = key_rect(key, rect);
        let bank = bank_color(layout, key);
        let base = if bank.is_some() {
            theme::KEY_BLACK
        } else {
            theme::KEY_BLACK_OFF
        };
        painter.rect_filled(kr, 2.0, with_glow(base, glow(key)));
        painter.rect_stroke(
            kr,
            2.0,
            egui::Stroke::new(1.0, theme::KEY_GAP),
            egui::StrokeKind::Inside,
        );
        if let Some(color) = bank {
            let band = Rect::from_min_max(
                Pos2::new(kr.left() + 1.5, kr.bottom() - 5.0),
                Pos2::new(kr.right() - 1.5, kr.bottom() - 1.5),
            );
            painter.rect_filled(band, 1.0, color);
        }
    }

    let pressed = if response.clicked() {
        response.interact_pointer_pos().and_then(|pos| {
            let key = key_at(pos, rect)?;
            Some((key, velocity_at(pos, key, rect)))
        })
    } else {
        None
    };
    KeyboardResponse { pressed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phydulcimer_core::layout::LayoutKind;

    fn board() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(270.0, 100.0))
    }

    #[test]
    fn the_range_has_29_white_keys() {
        let whites = (KEY_MIN..=KEY_MAX).filter(|&k| !is_black(k)).count();
        assert_eq!(whites, WHITE_COUNT);
        // 最後の白鍵のインデックスは 28。
        assert_eq!(white_index(KEY_MAX), WHITE_COUNT - 1);
    }

    #[test]
    fn bank_colors_follow_the_layout() {
        let diatonic = Layout::of(LayoutKind::Diatonic1514);
        // G2 はバス (赤)、A4 はトレブル右 (橙)、C#6 はトレブル左 (黄)。
        assert_eq!(bank_color(&diatonic, 43), Some(theme::BANK_BASS));
        assert_eq!(bank_color(&diatonic, 69), Some(theme::BANK_TREBLE_R));
        assert_eq!(bank_color(&diatonic, 85), Some(theme::BANK_TREBLE_L));
        // 15/14 に無い半音は色なし。
        for key in [44u8, 46, 61, 68, 88] {
            assert_eq!(bank_color(&diatonic, key), None, "key {key}");
        }

        let chromatic = Layout::of(LayoutKind::Chromatic);
        // 半音階では G#4 も E6 も鳴り、低音弦ブロックの D#2/G2 も鳴る (バス色)。
        assert!(bank_color(&chromatic, 68).is_some());
        assert!(bank_color(&chromatic, 88).is_some());
        assert_eq!(bank_color(&chromatic, 39), Some(theme::BANK_BASS));
        assert_eq!(bank_color(&chromatic, 43), Some(theme::BANK_BASS));
        // 15/14 では低音弦ブロックの鍵域は沈む。
        assert_eq!(bank_color(&diatonic, 39), None);
    }

    #[test]
    fn black_keys_sit_on_white_key_boundaries() {
        let rect = board();
        // G#2 (44) は G2 と A2 の境界 (前に白鍵 3 本 = E2, F2, G2) をまたぐ。
        let g_sharp = key_rect(44, rect);
        let ww = rect.width() / WHITE_COUNT as f32;
        let boundary = rect.left() + 3.0 * ww;
        assert!((g_sharp.center().x - boundary).abs() < 0.01);
        assert!(g_sharp.height() < rect.height());
    }

    #[test]
    fn hit_testing_prefers_black_keys() {
        let rect = board();
        // 黒鍵の中心は黒鍵に当たる。
        let pos = key_rect(44, rect).center();
        assert_eq!(key_at(pos, rect), Some(44));
        // 白鍵の下部 (黒鍵の下) は白鍵に当たる。
        let low = Pos2::new(key_rect(43, rect).center().x, rect.bottom() - 5.0);
        assert_eq!(key_at(low, rect), Some(43));
        // 領域外は None。
        assert_eq!(key_at(Pos2::new(-10.0, 50.0), rect), None);
    }

    #[test]
    fn velocity_grows_toward_the_bottom_of_the_key() {
        let rect = board();
        let kr = key_rect(60, rect);
        let top = velocity_at(Pos2::new(kr.center().x, kr.top()), 60, rect);
        let bottom = velocity_at(Pos2::new(kr.center().x, kr.bottom()), 60, rect);
        assert!((top - 0.2).abs() < 1e-6);
        assert!((bottom - 1.0).abs() < 1e-6);
        assert!(bottom > top);
    }
}
