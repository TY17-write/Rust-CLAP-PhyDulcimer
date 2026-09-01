//! エディタ本体 — ヘッダ / PLAY / ROOM / 鍵盤の 4 区画 (承認済みデザイン)。
//!
//! パラメータの値そのものは持たない (常に [`EditorHost`] から読む)。ここに
//! あるのは**画面の状態だけ** (グローの開始時刻、配置表のキャッシュ) で、
//! 閉じて開き直すと初期値に戻ってよいもの。

use std::time::Duration;

use phydulcimer_core::layout::{Layout, LayoutKind};

use crate::keyboard::{self, KeyboardResponse};
use crate::mic_stage;
use crate::{param_id, theme, EditorHost, ParamDescriptor};

/// 打鍵グローの長さ [s]。
const GLOW_SEC: f64 = 0.3;

/// エディタの状態。
pub struct Editor<H: EditorHost> {
    host: H,
    /// パラメータ仕様のキャッシュ (毎フレーム Vec を作らない)
    specs: Vec<ParamDescriptor>,
    /// 配置表のキャッシュ ([0] = Diatonic, [1] = Chromatic)
    layouts: [Layout; 2],
    /// 各鍵の打鍵シリアルの前回値
    last_serials: [u32; 128],
    /// 各鍵のグロー開始時刻 (`ui.input(|i| i.time)` 基準)。負 = 消灯
    glow_start: [f64; 128],
}

/// Layout / Temperament のパラメータ値がエンジン適用済みの値と食い違って
/// いるか (= "applies on restart" チップを出すか)。
pub fn restart_pending<H: EditorHost>(host: &H) -> bool {
    let layout = host.param_value(param_id::LAYOUT).round() as u32;
    let temperament = host.param_value(param_id::TEMPERAMENT).round() as u32;
    layout != host.active_layout() || temperament != host.active_temperament()
}

impl<H: EditorHost> Editor<H> {
    pub fn new(host: H) -> Self {
        let specs = host.params();
        let mut last_serials = [0u32; 128];
        for (key, slot) in last_serials.iter_mut().enumerate() {
            *slot = host.strike_serial(key as u8);
        }
        Self {
            host,
            specs,
            layouts: [
                Layout::of(LayoutKind::Diatonic1514),
                Layout::of(LayoutKind::Chromatic),
            ],
            last_serials,
            glow_start: [-1.0; 128],
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    fn spec(&self, id: u32) -> &ParamDescriptor {
        self.specs
            .iter()
            .find(|s| s.id == id)
            .expect("パラメータ仕様はホストが全 ID ぶん返す")
    }

    /// エンジンに適用済みの配置表。鍵盤の色分けはこちらで描く
    /// (パラメータをいじっただけではまだ鳴る配置は変わらない)。
    fn active_layout(&self) -> &Layout {
        if self.host.active_layout() >= 1 {
            &self.layouts[1]
        } else {
            &self.layouts[0]
        }
    }

    /// 打鍵シリアルを取り込み、グローを進める。返り値は「まだ光っている鍵があるか」。
    fn update_glows(&mut self, now: f64) -> bool {
        let mut any = false;
        for key in 0..128usize {
            let serial = self.host.strike_serial(key as u8);
            if serial != self.last_serials[key] {
                self.last_serials[key] = serial;
                self.glow_start[key] = now;
            }
            if self.glow_start[key] >= 0.0 && now - self.glow_start[key] < GLOW_SEC {
                any = true;
            }
        }
        any
    }

    fn glow_at(&self, key: u8, now: f64) -> f32 {
        let start = self.glow_start[key as usize];
        if start < 0.0 {
            return 0.0;
        }
        let t = (now - start) / GLOW_SEC;
        if t >= 1.0 {
            0.0
        } else {
            (1.0 - t) as f32
        }
    }

    /// 1 フレーム描画する。
    ///
    /// egui 0.35 からパネルは `&mut Ui` を取る。`egui-baseview` の更新
    /// コールバックもこの形で `Ui` を渡してくるので、そのまま呼べる。
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // egui 0.35: スタイルはテーマ別。ライト/ダークのどちらで開かれても
        // 同じ見た目になるよう両方に当てる。
        ui.ctx().all_styles_mut(theme::apply);

        let now = ui.input(|i| i.time);
        if self.update_glows(now) {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }

        egui::Panel::top("header").show(ui, |ui| {
            self.header(ui);
        });
        egui::Panel::bottom("keys").show(ui, |ui| {
            self.keys_panel(ui, now);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(280.0);
                    self.play_panel(ui);
                });
                ui.separator();
                ui.vertical(|ui| {
                    self.room_panel(ui);
                });
            });
        });
    }

    // ---- ヘッダ -----------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(
                egui::RichText::new("PhyDulcimer")
                    .color(theme::ACCENT)
                    .strong(),
            );
            ui.label(
                egui::RichText::new("hammered dulcimer")
                    .color(theme::DIM)
                    .small(),
            );
            ui.separator();

            ui.label(egui::RichText::new("LAYOUT").color(theme::DIM).small());
            self.segmented(
                ui,
                param_id::LAYOUT,
                &["Diatonic 15/14", "Chromatic D#2-E6"],
            );
            ui.label(egui::RichText::new("TUNING").color(theme::DIM).small());
            self.segmented(ui, param_id::TEMPERAMENT, &["Pure Fifth", "Equal"]);

            if restart_pending(&self.host) {
                ui.label(egui::RichText::new("pending").color(theme::RED).small());
                if ui
                    .button(egui::RichText::new("Reload").color(theme::RED))
                    .clicked()
                {
                    self.host.request_reload();
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.slider(ui, param_id::LEVEL, "Level");
            });
        });
    }

    // ---- PLAY パネル ------------------------------------------------------

    fn play_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("PLAY").color(theme::DIM).small());
        ui.add_space(2.0);

        self.strike_diagram(ui);
        self.slider(ui, param_id::STRIKE_POSITION, "Strike Position");
        self.cc_hint(ui, "CC74 - next strike");
        ui.add_space(6.0);

        ui.label("Hammer Face");
        self.segmented(ui, param_id::HAMMER_FACE, &["Wood", "Leather", "Felt"]);
        self.cc_hint(ui, "CC70 - next strike");
        ui.add_space(6.0);

        self.slider(ui, param_id::MUTE, "Mute");
        self.cc_hint(ui, "CC1 - palm on strings, immediate");
        ui.add_space(8.0);

        ui.label(
            egui::RichText::new("No dampers: notes ring after release.")
                .color(theme::FAINT)
                .small(),
        );
    }

    /// 打弦点のミニダイアグラム — ブリッジ・弦・打点マーカー。
    fn strike_diagram(&self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::Vec2::new(ui.available_width().min(260.0), 26.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        let y = rect.center().y;
        // ブリッジ (左端の木片) と弦。
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::Pos2::new(rect.left(), rect.top() + 3.0),
                egui::Vec2::new(5.0, rect.height() - 6.0),
            ),
            1.0,
            theme::WOOD_LIGHT,
        );
        painter.line_segment(
            [
                egui::Pos2::new(rect.left() + 5.0, y),
                egui::Pos2::new(rect.right(), y),
            ],
            egui::Stroke::new(1.5, theme::DIM),
        );
        // 打点マーカー: x/L 0..0.5 を弦の左半分へ写す。
        let ratio = self.host.param_value(param_id::STRIKE_POSITION) as f32;
        let x = rect.left() + 5.0 + (rect.width() - 5.0) * ratio;
        painter.line_segment(
            [
                egui::Pos2::new(x, rect.top() + 2.0),
                egui::Pos2::new(x, y - 4.0),
            ],
            egui::Stroke::new(2.0, theme::ACCENT),
        );
        painter.circle_filled(egui::Pos2::new(x, y), 4.0, theme::ACCENT);
    }

    // ---- ROOM パネル ------------------------------------------------------

    fn room_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("ROOM - X-Y STEREO")
                .color(theme::DIM)
                .small(),
        );
        ui.add_space(2.0);
        let room_on = self.host.param_value(param_id::ROOM) >= 0.5;

        ui.horizontal_top(|ui| {
            // 左: ステージ (残り幅から操作列 170px を除いた幅)。
            let stage_width = (ui.available_width() - 170.0).max(180.0);
            ui.vertical(|ui| {
                ui.set_width(stage_width);
                let distance = self.host.param_value(param_id::MIC_DISTANCE);
                let angle = self.host.param_value(param_id::XY_ANGLE);
                let resp = mic_stage::mic_stage(ui, distance, angle, room_on);
                if let Some(d) = resp.mic_distance_m {
                    self.host.set_param(param_id::MIC_DISTANCE, d);
                }
                if let Some(a) = resp.xy_angle_deg {
                    self.host.set_param(param_id::XY_ANGLE, a);
                }
            });

            // 右: 操作列。ステージと同じパラメータを別の形で。
            ui.vertical(|ui| {
                ui.set_width(160.0);
                let mut on = room_on;
                if ui.checkbox(&mut on, "Room").changed() {
                    self.host
                        .set_param(param_id::ROOM, if on { 1.0 } else { 0.0 });
                }
                ui.label(egui::RichText::new("Size").color(theme::DIM).small());
                self.segmented(ui, param_id::ROOM_SIZE, &["S", "M", "L"]);
                self.slider(ui, param_id::ABSORPTION, "Absorption");
                self.slider(ui, param_id::MIC_DISTANCE, "Mic Distance");
                self.slider(ui, param_id::XY_ANGLE, "X-Y Angle");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Turn Room off when mixing with DAW reverb.")
                        .color(theme::FAINT)
                        .small(),
                );
            });
        });
    }

    // ---- 鍵盤 -------------------------------------------------------------

    fn keys_panel(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            let (range, count) = if self.host.active_layout() >= 1 {
                ("E3 - E6", 37)
            } else {
                ("G2 - D6", 27)
            };
            ui.label(
                egui::RichText::new(format!("KEYS - {range} - {count} NOTES"))
                    .color(theme::DIM)
                    .small(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.legend(ui);
            });
        });

        let glow = |key: u8| self.glow_at(key, now);
        let KeyboardResponse { pressed } = keyboard::keyboard(ui, self.active_layout(), &glow);
        if let Some((key, velocity)) = pressed {
            // 満杯 (false) は捨てる — クリック連打で詰まるほどのレートは出ない。
            let _ = self.host.note_on(key, velocity);
        }
        ui.label(
            egui::RichText::new("click a key to strike - velocity follows click height")
                .color(theme::FAINT)
                .small(),
        );
    }

    fn legend(&self, ui: &mut egui::Ui) {
        // right_to_left なので逆順に並べる。
        let entries = [
            ("struck", theme::GLOW),
            ("treble L", theme::BANK_TREBLE_L),
            ("treble R", theme::BANK_TREBLE_R),
            ("bass", theme::BANK_BASS),
        ];
        for (label, color) in entries {
            ui.label(egui::RichText::new(label).color(theme::DIM).small());
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color);
        }
    }

    // ---- 共通ウィジェット --------------------------------------------------

    /// 仕様駆動のスライダ。変更はその場でホストへ書く。
    fn slider(&self, ui: &mut egui::Ui, id: u32, label: &str) {
        let spec = self.spec(id).clone();
        let mut value = self.host.param_value(id);
        let changed = ui
            .add(
                egui::Slider::new(&mut value, spec.min..=spec.max)
                    .text(label)
                    .suffix(&spec.unit)
                    .fixed_decimals(spec.decimals),
            )
            .changed();
        if changed {
            self.host.set_param(id, value);
        }
    }

    /// 列挙パラメータのセグメント表示 (0, 1, 2, ... に丸めた値)。
    fn segmented(&self, ui: &mut egui::Ui, id: u32, labels: &[&str]) {
        let current = self
            .host
            .param_value(id)
            .round()
            .clamp(0.0, labels.len() as f64 - 1.0) as usize;
        ui.horizontal(|ui| {
            for (i, label) in labels.iter().enumerate() {
                let text = if i == current {
                    egui::RichText::new(*label).color(theme::HIGHLIGHT)
                } else {
                    egui::RichText::new(*label).color(theme::DIM)
                };
                if ui.selectable_label(i == current, text).clicked() && i != current {
                    self.host.set_param(id, i as f64);
                }
            }
        });
    }

    fn cc_hint(&self, ui: &mut egui::Ui, text: &str) {
        ui.label(egui::RichText::new(text).color(theme::FAINT).small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// テスト用のホスト。値を素直に覚えるだけ。
    struct FakeHost {
        values: RefCell<HashMap<u32, f64>>,
        notes: RefCell<Vec<(u8, f32)>>,
        serials: RefCell<[u32; 128]>,
        active_layout: u32,
        active_temperament: u32,
        reloads: std::cell::Cell<u32>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                values: RefCell::new(HashMap::new()),
                notes: RefCell::new(Vec::new()),
                serials: RefCell::new([0; 128]),
                active_layout: 0,
                active_temperament: 0,
                reloads: std::cell::Cell::new(0),
            }
        }
    }

    fn specs() -> Vec<ParamDescriptor> {
        // プラグイン側と同じ 11 個 (値はテストに足る範囲だけ正確に)。
        let mk = |id: u32, name: &str, min: f64, max: f64, default: f64, decimals: usize| {
            ParamDescriptor {
                id,
                name: name.into(),
                min,
                max,
                default,
                unit: String::new(),
                decimals,
            }
        };
        vec![
            mk(param_id::LEVEL, "Level", 0.0, 1.0, 0.7, 2),
            mk(
                param_id::STRIKE_POSITION,
                "Strike Position",
                0.03,
                0.30,
                0.09,
                3,
            ),
            mk(param_id::ROOM, "Room", 0.0, 1.0, 1.0, 0),
            mk(param_id::MIC_DISTANCE, "Mic Distance", 0.3, 3.0, 1.2, 2),
            mk(param_id::XY_ANGLE, "X-Y Angle", 60.0, 135.0, 90.0, 0),
            mk(param_id::ROOM_SIZE, "Room Size", 0.0, 2.0, 1.0, 0),
            mk(param_id::ABSORPTION, "Wall Absorption", 0.0, 0.9, 0.35, 2),
            mk(param_id::HAMMER_FACE, "Hammer Face", 0.0, 2.0, 0.0, 0),
            mk(param_id::MUTE, "Mute", 0.0, 1.0, 0.0, 2),
            mk(param_id::TEMPERAMENT, "Temperament", 0.0, 1.0, 0.0, 0),
            mk(param_id::LAYOUT, "Layout", 0.0, 1.0, 0.0, 0),
        ]
    }

    impl EditorHost for FakeHost {
        fn params(&self) -> Vec<ParamDescriptor> {
            specs()
        }
        fn param_value(&self, id: u32) -> f64 {
            *self.values.borrow().get(&id).unwrap_or(
                &specs()
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.default)
                    .unwrap_or(0.0),
            )
        }
        fn set_param(&self, id: u32, value: f64) {
            self.values.borrow_mut().insert(id, value);
        }
        fn note_on(&self, key: u8, velocity: f32) -> bool {
            self.notes.borrow_mut().push((key, velocity));
            true
        }
        fn strike_serial(&self, key: u8) -> u32 {
            self.serials.borrow()[key as usize]
        }
        fn active_layout(&self) -> u32 {
            self.active_layout
        }
        fn active_temperament(&self) -> u32 {
            self.active_temperament
        }
        fn request_reload(&self) {
            self.reloads.set(self.reloads.get() + 1);
        }
    }

    /// ウィンドウを立てずに 1 フレーム描く。
    fn run_frame<H: EditorHost>(editor: &mut Editor<H>) {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| editor.ui(ui));
    }

    #[test]
    fn the_editor_draws_without_panicking() {
        let mut editor = Editor::new(FakeHost::new());
        run_frame(&mut editor);
        // Chromatic 適用済みの状態でも描ける (鍵盤の配置が切り替わる経路)。
        let mut host = FakeHost::new();
        host.active_layout = 1;
        let mut editor = Editor::new(host);
        run_frame(&mut editor);
    }

    #[test]
    fn restart_pending_compares_params_with_the_active_config() {
        let host = FakeHost::new();
        assert!(!restart_pending(&host), "既定では一致している");
        host.set_param(param_id::LAYOUT, 1.0);
        assert!(restart_pending(&host), "配置を変えたら保留になる");
        host.set_param(param_id::LAYOUT, 0.0);
        host.set_param(param_id::TEMPERAMENT, 1.0);
        assert!(restart_pending(&host), "音律の変更も保留になる");
    }

    #[test]
    fn a_strike_serial_change_starts_a_glow_that_decays() {
        let mut editor = Editor::new(FakeHost::new());
        editor.host.serials.borrow_mut()[69] = 1;
        assert!(editor.update_glows(10.0), "打鍵直後は光っている");
        assert!(editor.glow_at(69, 10.0) > 0.99);
        assert!(editor.glow_at(69, 10.15) < 0.6);
        assert_eq!(editor.glow_at(69, 10.4), 0.0, "0.3 秒で消える");
        assert!(!editor.update_glows(10.4), "消えたら再描画も止まる");
        // 触っていない鍵は光らない。
        assert_eq!(editor.glow_at(60, 10.0), 0.0);
    }

    #[test]
    fn editor_new_swallows_preexisting_serials() {
        // 開いた瞬間に「過去の打鍵」で全鍵が光らないこと。
        let host = FakeHost::new();
        host.serials.borrow_mut()[60] = 5;
        let mut editor = Editor::new(host);
        assert!(!editor.update_glows(0.0));
    }

    /// P-037 (PhyPiano) の回帰: 画面に出す文字列は ASCII のみ。
    /// egui の既定フォントに日本語グリフが無く、DAW 上で豆腐になる。
    /// ソースを直接見て確かめる (描画結果からは取り出せない)。
    #[test]
    fn ui_strings_are_ascii_only() {
        for source in [
            include_str!("lib.rs"),
            include_str!("editor.rs"),
            include_str!("keyboard.rs"),
            include_str!("mic_stage.rs"),
            include_str!("theme.rs"),
        ] {
            // テストモジュール以降は画面に出ない。
            let visible = source.split("#[cfg(test)]").next().unwrap_or(source);
            for (n, line) in visible.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let draws_text = [
                    "ui.label(",
                    "ui.button(",
                    "ui.checkbox(",
                    "selectable_label(",
                    "RichText::new(",
                    "painter.text(",
                    ".text(",
                    ".suffix(",
                ]
                .iter()
                .any(|c| line.contains(c));
                if draws_text {
                    assert!(
                        line.is_ascii(),
                        "{}行目に非 ASCII の表示文字列がある (DAW で豆腐になる): {}",
                        n + 1,
                        line.trim()
                    );
                }
            }
        }
    }

    #[test]
    fn every_param_id_has_a_spec_in_the_fake_host() {
        // spec() の expect が実パラメータ集合で成立することの下支え。
        let editor = Editor::new(FakeHost::new());
        for id in [
            param_id::LEVEL,
            param_id::STRIKE_POSITION,
            param_id::ROOM,
            param_id::MIC_DISTANCE,
            param_id::XY_ANGLE,
            param_id::ROOM_SIZE,
            param_id::ABSORPTION,
            param_id::HAMMER_FACE,
            param_id::MUTE,
            param_id::TEMPERAMENT,
            param_id::LAYOUT,
        ] {
            let _ = editor.spec(id);
        }
    }
}
