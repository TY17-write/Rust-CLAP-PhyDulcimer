//! vim-hybrid ベースのダークテーマ + 木のアクセント (承認済みデザイン)。
//!
//! パレットは w0ng/vim-hybrid の色をそのまま使う。木・真鍮の連想は
//! 橙 (#de935f) と黄 (#f0c674) が受け持つ。

use egui::Color32;

/// 背景 (最深部)。
pub const BG: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x21);
/// パネル面。
pub const PANEL: Color32 = Color32::from_rgb(0x26, 0x28, 0x2c);
/// 罫線・非アクティブ面。
pub const BORDER: Color32 = Color32::from_rgb(0x37, 0x3b, 0x41);
/// 本文。
pub const TEXT: Color32 = Color32::from_rgb(0xc5, 0xc8, 0xc6);
/// 補助テキスト。
pub const DIM: Color32 = Color32::from_rgb(0x70, 0x78, 0x80);
/// さらに沈んだ注記。
pub const FAINT: Color32 = Color32::from_rgb(0x4a, 0x4f, 0x55);
/// 木のアクセント (橙)。
pub const ACCENT: Color32 = Color32::from_rgb(0xde, 0x93, 0x5f);
/// 強調 (黄)。選択中のセグメント文字など。
pub const HIGHLIGHT: Color32 = Color32::from_rgb(0xf0, 0xc6, 0x74);
/// 警告 (赤)。"applies on restart" チップ。
pub const RED: Color32 = Color32::from_rgb(0xcc, 0x66, 0x66);
/// 距離・吸音などの水系アクセント。
pub const AQUA: Color32 = Color32::from_rgb(0x8a, 0xbe, 0xb7);
/// 打鍵グロー (ウォームホワイト)。バンク色と紛れない「光」。
pub const GLOW: Color32 = Color32::from_rgb(0xf7, 0xf3, 0xe8);

/// 鍵盤のバンク色 — 暖色の類似色ランプ (低音 = 赤 → 高音 = 黄)。
pub const BANK_BASS: Color32 = RED;
pub const BANK_TREBLE_R: Color32 = ACCENT;
pub const BANK_TREBLE_L: Color32 = HIGHLIGHT;

/// 鍵の面。
pub const KEY_WHITE: Color32 = Color32::from_rgb(0x2f, 0x32, 0x37);
pub const KEY_WHITE_OFF: Color32 = Color32::from_rgb(0x23, 0x25, 0x27);
pub const KEY_BLACK: Color32 = Color32::from_rgb(0x1b, 0x1d, 0x20);
pub const KEY_BLACK_OFF: Color32 = Color32::from_rgb(0x10, 0x12, 0x14);
/// 鍵盤の背板 (鍵の隙間に見える色)。
pub const KEY_GAP: Color32 = Color32::from_rgb(0x16, 0x18, 0x1a);

/// 楽器俯瞰図の木の色 (上端 / 下端)。
pub const WOOD_LIGHT: Color32 = Color32::from_rgb(0x8f, 0x5a, 0x36);
pub const WOOD_DARK: Color32 = Color32::from_rgb(0x6b, 0x42, 0x26);

/// egui の Style をこのテーマに合わせる。
pub fn apply(style: &mut egui::Style) {
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = KEY_GAP;
    v.faint_bg_color = PANEL;

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = BORDER;
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.hovered.bg_fill = BORDER;
    v.widgets.hovered.weak_bg_fill = BORDER;
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = BORDER;

    v.selection.bg_fill = BORDER;
    v.selection.stroke = egui::Stroke::new(1.0, HIGHLIGHT);
    v.slider_trailing_fill = true;

    style.spacing.slider_width = 140.0;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
}
