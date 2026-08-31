//! PhyDulcimer のエディタ GUI (egui)。
//!
//! # このクレートはウィンドウを開かない
//!
//! ここにあるのは**描画とパラメータの読み書きだけ**で、ウィンドウの生成や
//! イベントループは `phydulcimer-plugin` 側が `egui-baseview` で受け持つ。
//! 分けてあるのは 2 つの理由から (PhyPiano P-033 の踏襲):
//!
//! 1. **テストできる。** ウィンドウを立てずに `egui::Context` を直接叩けば、
//!    UI のロジック (鍵盤の色分け、座標⇔値の変換、表示の整形) を検証できる
//! 2. **依存が閉じる。** ウィンドウまわりは winit / OpenGL と 100 を超える
//!    クレートを引き込む。DSP と共有する層に持ち込まない
//!
//! # core への依存について
//!
//! 鍵盤の色分け (どの鍵がどのブリッジで鳴るか) は
//! [`Layout`](phydulcimer_core::layout::Layout) の配置表そのもの。GUI 側に
//! 表を複製すると必ずドリフトするので、core を読み取り専用で参照する
//! (core の依存は SIMD の `wide` 1 つだけで軽い)。
//!
//! # 画面に出す文字は ASCII だけ
//!
//! **egui の既定フォントに日本語のグリフが無い** (PhyPiano P-037 で実害)。
//! UI の文字列は英語にする。コメントと docs は日本語のまま (画面に出ない)。

#![forbid(unsafe_code)]

pub mod editor;
pub mod keyboard;
pub mod mic_stage;
pub mod theme;

pub use editor::Editor;

/// エディタウィンドウの既定サイズ [px]。デザイン (Artifact) の 960x640。
pub const DEFAULT_EDITOR_SIZE: (u32, u32) = (960, 640);

/// 鍵盤に描く鍵域 (両配置の合併): G2 〜 E6。
pub const KEY_MIN: u8 = 43;
pub const KEY_MAX: u8 = 88;

/// パラメータ ID。
///
/// プラグイン側 `params::id` と同じ値の**複製** (参照すると依存が逆流する)。
/// 一致はプラグイン側のテスト `gui_param_ids_match_the_plugin` が固定する。
pub mod param_id {
    pub const LEVEL: u32 = 1;
    pub const STRIKE_POSITION: u32 = 2;
    pub const ROOM: u32 = 3;
    pub const MIC_DISTANCE: u32 = 4;
    pub const XY_ANGLE: u32 = 5;
    pub const ROOM_SIZE: u32 = 6;
    pub const ABSORPTION: u32 = 7;
    pub const HAMMER_FACE: u32 = 8;
    pub const MUTE: u32 = 9;
    pub const TEMPERAMENT: u32 = 10;
    pub const LAYOUT: u32 = 11;
}

/// パラメータ 1 つの仕様。
///
/// プラグイン側の定義をそのまま参照すると依存が逆流するので、GUI が必要とする
/// ぶんだけをここで持つ (PhyPiano と同じ形)。
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDescriptor {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// 表示に付ける単位 (先頭の空白込み。例: `" m"`)
    pub unit: String,
    pub decimals: usize,
}

/// エディタからプラグインへの窓口。
///
/// **実装側がスレッド安全性を担保する。** GUI は専用のスレッドから呼ばれるので、
/// パラメータはアトミック、ノートはロックフリーのリング越しに触ること。
/// GUI は DSP の状態を直接借用しない。
pub trait EditorHost {
    /// 公開しているパラメータの一覧。
    fn params(&self) -> Vec<ParamDescriptor>;
    /// 現在値。
    fn param_value(&self, id: u32) -> f64;
    /// 値を書く。**ホストへの通知も実装側の責任。**
    fn set_param(&self, id: u32, value: f64);
    /// 鍵盤クリックの発音。`false` はリング満杯で捨てられたことを表す。
    fn note_on(&self, key: u8, velocity: f32) -> bool;
    /// その鍵が鳴らされた回数 (音声スレッドが増やすシリアル)。
    /// GUI は毎フレーム差分を見てグローを開始する。
    fn strike_serial(&self, key: u8) -> u32;
    /// エンジンに**適用済み**の配置 (0 = Diatonic 15/14, 1 = Chromatic E3-E6)。
    /// パラメータ現在値と食い違っている間は "applies on restart" を出す。
    fn active_layout(&self) -> u32;
    /// エンジンに適用済みの音律 (0 = Pure Fifth, 1 = Equal)。
    fn active_temperament(&self) -> u32;
}
