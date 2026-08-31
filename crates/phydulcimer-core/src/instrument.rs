//! 楽器全体 — 全弦の常時走行。
//!
//! **ボイスプールは無い。** 弦は楽器の固定資産として構築時に確保し、
//! `process` のたびに全部回す。打撃は既に振動している弦への力の注入であって、
//! ボイスの確保ではない。ダンパーの無い楽器ではこれが正しい形になる
//! (`docs/plan.html` §03)。
//!
//! - ノートオンは撥を 1 本起動するだけ。**弦の状態はリセットしない**
//!   (連打・ロールで振動に足し込まれる)
//! - ノートオフに対応する物理が存在しないので、**受け取っても捨てる**
//! - ホストの停止 (choke / reset) だけが弦を止める
//!
//! # 配置と設計 (Phase 3)
//!
//! 発音位置は [`Layout`](crate::layout::Layout) の 15/14 標準配置 (44 位置)、
//! 各位置の物理は [`scaling`](crate::scaling) の設計則から導く。
//! **全音階配置なので、楽器に無い半音の MIDI 鍵は無音** (D-017)。
//! 同じ音高が複数の位置にあるときは最も長い区間を叩く (バス → トレブル右 →
//! トレブル左の順)。
//!
//! # まだ無いもの
//!
//! - コースあたり 2 本の弦 (実機は 2 本、いまは 1 本)
//! - ブリッジをまたぐ 2 区間の結合 (Phase 4)。トレブル左右はいまは独立した
//!   区間として鳴る
//! - 響板・箱・ROOM (Phase 5–6)。ブリッジ力の和がそのまま出力

use crate::layout::Layout;
use crate::scaling::design_position;
use crate::segment::Segment;
use crate::Sample;

/// 最低音の MIDI ノート番号 (G2 = 98.00 Hz)。
pub const KEY_MIN: u8 = 43;

/// 最高音の MIDI ノート番号 (D6 = 1174.66 Hz)。
pub const KEY_MAX: u8 = 86;

/// 発音位置 (= 走らせる区間) の総数。
pub const STRING_COUNT: usize = crate::layout::POSITION_COUNT;

/// ベロシティ 0–1 → 撥の速度 [m/s]。
///
/// 実機の「drop and bounce」は 1.7–2.4 m/s、強打で 6 m/s 程度。
/// pp = 0.5 m/s から ff = 6 m/s へ線形に割り付ける。
#[inline]
fn hammer_speed(velocity: f64) -> f64 {
    0.5 + 5.5 * velocity.clamp(0.0, 1.0)
}

/// 全弦バンク。
pub struct Instrument {
    layout: Layout,
    /// [`Layout::positions`] と同じ並びの区間。
    segments: Vec<Segment>,
    /// 打弦点 x/L。次の打撃から効く (鳴っている弦の係数は触らない)
    strike_ratio: f64,
}

impl Instrument {
    /// 全弦を構築する。**確保はここだけ** (メインスレッドで呼ぶこと)。
    pub fn new(sample_rate: f64) -> Self {
        let layout = Layout::standard_15_14();
        let segments = layout
            .positions()
            .iter()
            .map(|p| {
                let (design, damping) = design_position(p);
                let mut seg = Segment::new(design.segment_params(), sample_rate);
                seg.set_damping(damping);
                seg
            })
            .collect();

        Self {
            layout,
            segments,
            strike_ratio: 0.09,
        }
    }

    /// 発音位置の数。
    pub fn string_count(&self) -> usize {
        self.segments.len()
    }

    /// 配置表。
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// 打弦点 x/L を設定する。**次の打撃から効く。**
    ///
    /// 鳴っている弦の係数は触らない。係数の再構築 (モード数ぶんの sin 計算) を
    /// 打撃時だけに限ることで、パラメータのオートメーションが走っていても
    /// オーディオスレッドの負荷が増えない。
    pub fn set_strike_ratio(&mut self, ratio: f64) {
        self.strike_ratio = ratio.clamp(0.005, 0.5);
    }

    /// 打弦。`velocity` は 0–1。楽器に無い鍵 (半音の欠落・範囲外) は何もしない。
    ///
    /// **弦の状態はリセットしない。** 鳴っている弦を叩けば足し込まれる。
    pub fn note_on(&mut self, key: u8, velocity: f64) {
        let ratio = self.strike_ratio;
        let Some(idx) = self.layout.preferred_index(key) else {
            return;
        };
        let Some(seg) = self.segments.get_mut(idx) else {
            return;
        };
        // 打弦点が変わっていたときだけ係数を作り直す (モード数 × 2 レート)。
        if (seg.strike_ratio() - ratio).abs() > 1e-9 {
            seg.set_strike_ratio(ratio);
        }
        seg.strike(hammer_speed(velocity));
    }

    /// ノートオフ。**何もしない。** ダンパーが無い。
    ///
    /// 明示的に置いてあるのは「実装し忘れ」と区別するため。
    pub fn note_off(&mut self, _key: u8) {}

    /// 1 本の弦を即座に消音する (ホストの choke)。
    ///
    /// 演奏上の操作ではない。奏者のミュート (手のひら) は Phase 7 で
    /// モードごとの減衰として入る。
    pub fn choke(&mut self, key: u8) {
        if let Some(idx) = self.layout.preferred_index(key) {
            if let Some(seg) = self.segments.get_mut(idx) {
                seg.reset();
            }
        }
    }

    /// 全弦を即座に消音する (ホストの停止・シーク)。
    pub fn reset(&mut self) {
        for seg in &mut self.segments {
            seg.reset();
        }
    }

    /// 1 ブロック処理する (上書き)。返り値はブロック内のピーク絶対値。
    ///
    /// 出力はブリッジ力の和 [N]。校正と 2ch 化は呼び出し側 (プラグイン層)。
    pub fn process(&mut self, out: &mut [Sample]) -> Sample {
        out.fill(0.0);
        // 弦ごとにブロックを回す (弦の係数と状態がキャッシュに乗ったまま使える)。
        for seg in &mut self.segments {
            for s in out.iter_mut() {
                *s += seg.process_sample();
            }
        }
        out.iter().fold(0.0 as Sample, |a, &b| a.max(b.abs()))
    }

    /// いずれかの撥が飛行・接触中か。眠ってよいかの判定に使う。
    pub fn any_hammer_active(&self) -> bool {
        self.segments.iter().any(|s| s.hammer().is_active())
    }

    /// 全弦の状態が有限か。
    pub fn is_finite(&self) -> bool {
        self.segments.iter().all(|s| s.is_finite())
    }

    /// 検証用: 指定した鍵が叩く区間の設計値。未マップなら `None`。
    pub fn string_params(&self, key: u8) -> Option<&crate::segment::SegmentParams> {
        let idx = self.layout.preferred_index(key)?;
        Some(self.segments[idx].params())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::BridgeSide;

    const SR: f64 = 48_000.0;

    fn render(inst: &mut Instrument, seconds: f64) -> Vec<Sample> {
        let n = (SR * seconds) as usize;
        let mut out = vec![0.0 as Sample; n];
        // 64 サンプルずつ (実際のホストのブロックに似せる)。
        for chunk in out.chunks_mut(64) {
            inst.process(chunk);
        }
        out
    }

    fn magnitude_at(x: &[Sample], freq_hz: f64) -> f64 {
        let n = x.len();
        let w = std::f64::consts::TAU * freq_hz / SR;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            wsum += win;
            let s0 = v as f64 * win + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let re = s1 - s2 * w.cos();
        let im = s2 * w.sin();
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    #[test]
    fn the_instrument_has_44_speaking_positions() {
        let inst = Instrument::new(SR);
        assert_eq!(inst.string_count(), STRING_COUNT);
        assert_eq!(STRING_COUNT, 44);
    }

    #[test]
    fn a_note_rings_at_its_pitch() {
        let mut inst = Instrument::new(SR);
        inst.note_on(69, 0.7); // A4 (トレブル右)
        let x = render(&mut inst, 0.5);

        let on = magnitude_at(&x, 440.0);
        let off = magnitude_at(&x, 466.16); // 半音上
        assert!(
            on > off * 10.0,
            "A4 が 440 Hz で鳴っていない: {on:.3e} vs {off:.3e}"
        );
    }

    #[test]
    fn left_only_pitches_ring_with_the_pure_fifth_offset() {
        // C#6 (85) はトレブル左にしか無い。純正5度の帰結で 12 平均律より
        // +2 cent 高く鳴る (D-017)。ずれた音高で実在することを確認する。
        let mut inst = Instrument::new(SR);
        inst.note_on(85, 0.8);
        let x = render(&mut inst, 0.5);

        let expected = inst.string_params(85).unwrap().f0_hz;
        let on = magnitude_at(&x, expected);
        let semitone_off = magnitude_at(&x, expected / 1.0595);
        assert!(on > semitone_off * 10.0, "C#6 が鳴っていない");

        let tet = crate::scaling::key_to_hz(85);
        let cents = 1200.0 * (expected / tet).log2();
        assert!((1.5..=2.5).contains(&cents), "左側のずれが {cents:.2} cent");
    }

    #[test]
    fn unmapped_chromatic_keys_are_silent() {
        // 全音階配置なので G#4 (68) などの半音は楽器に無い (D-017)。
        let mut inst = Instrument::new(SR);
        for key in [44u8, 61, 68, 75, 84] {
            inst.note_on(key, 1.0);
        }
        // 範囲外も。
        inst.note_on(KEY_MIN - 1, 1.0);
        inst.note_on(KEY_MAX + 1, 1.0);
        let x = render(&mut inst, 0.1);
        assert!(x.iter().all(|&s| s == 0.0), "無いはずの鍵で音が出た");
    }

    #[test]
    fn duplicated_pitches_strike_the_bass_segment() {
        // D4 (62) は 3 箇所にあるが、既定はバス (最も長い区間)。
        let inst = Instrument::new(SR);
        let p = inst.string_params(62).unwrap();
        // バス D4 の発弦長は約 0.44 m。トレブル右 D4 は 0.42 m、左は 0.24 m。
        // 添字で確かめるほうが確実:
        let idx = inst.layout().preferred_index(62).unwrap();
        assert_eq!(inst.layout().positions()[idx].side, BridgeSide::Bass);
        assert!(p.f0_hz > 293.0 && p.f0_hz < 294.0);
    }

    #[test]
    fn restriking_adds_to_the_ringing_string() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.5);
        render(&mut inst, 0.2);
        inst.note_on(60, 0.5);
        let x = render(&mut inst, 0.2);
        assert!(inst.is_finite());
        assert!(x.iter().any(|&s| s.abs() > 0.0));
    }

    #[test]
    fn repeated_ff_strikes_converge_to_a_bounded_roll() {
        // DAW のループ再生の再現 (D-016)。ff で 0.1 秒おきに 30 回叩き続けても
        // レベルが収束すること。
        let mut inst = Instrument::new(SR);

        inst.note_on(60, 1.0);
        render(&mut inst, 0.5);
        let single = render(&mut inst, 0.2)
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        inst.reset();

        let mut windows = Vec::new();
        for _ in 0..30 {
            inst.note_on(60, 1.0);
            let x = render(&mut inst, 0.1);
            windows.push(x.iter().fold(0.0f32, |a, &b| a.max(b.abs())));
        }

        assert!(inst.is_finite(), "連打で非有限値が出た");
        let late = windows[29];
        let mid = windows[19];
        assert!(
            late < single * 50.0,
            "ロールが収束していない: 単発 {single:.3e} → 30 打目 {late:.3e}"
        );
        assert!(
            late < mid * 1.5,
            "連打でレベルが増え続けている: 20 打目 {mid:.3e} → 30 打目 {late:.3e}"
        );
    }

    #[test]
    fn choke_silences_only_that_string() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.8);
        inst.note_on(67, 0.8);
        render(&mut inst, 0.2);

        inst.choke(60);
        let x = render(&mut inst, 0.3);

        let c4 = magnitude_at(&x, 261.63);
        let g4 = magnitude_at(&x, 392.0);
        assert!(
            g4 > c4 * 10.0,
            "choke が効いていない: C4 {c4:.3e}, G4 {g4:.3e}"
        );
    }

    #[test]
    fn reset_silences_everything() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.8);
        inst.note_on(72, 0.8);
        render(&mut inst, 0.1);
        inst.reset();
        let x = render(&mut inst, 0.1);
        assert!(x.iter().all(|&s| s == 0.0));
        assert!(!inst.any_hammer_active());
    }

    #[test]
    fn strike_ratio_applies_to_the_next_strike() {
        let mut inst = Instrument::new(SR);
        inst.set_strike_ratio(0.125);
        inst.note_on(60, 0.8);
        let x = render(&mut inst, 0.4);

        // 1/8 で叩いたので第 8 部分音が欠ける。
        let p = *inst.string_params(60).unwrap();
        let at = |n: usize| magnitude_at(&x, p.partial_hz(n));
        assert!(at(8) < at(7) * 0.02, "打弦点 1/8 のノッチが出ていない");
    }

    #[test]
    fn process_returns_the_block_peak() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.9);
        let mut out = vec![0.0 as Sample; 4_800];
        let peak = inst.process(&mut out);
        let expected = out.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert_eq!(peak, expected);
        assert!(peak > 0.0);
    }

    #[test]
    fn all_mapped_keys_at_once_stay_finite() {
        let mut inst = Instrument::new(SR);
        for key in KEY_MIN..=KEY_MAX {
            inst.note_on(key, 1.0); // 未マップは無視される
        }
        let x = render(&mut inst, 0.5);
        assert!(inst.is_finite());
        assert!(x.iter().all(|s| s.is_finite()));
    }
}
