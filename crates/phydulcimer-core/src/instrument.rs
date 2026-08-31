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
//! # Phase 2 の暫定形
//!
//! いまは**単区間 × 44 弦のクロマチック配置**。以下は後のフェーズで置き換わる:
//!
//! - 弦の設計則が仮 (発弦長 × 基音 = 一定)。15/14 の配置表と音域ごとの
//!   設計は Phase 3 の `layout.rs` / `scaling.rs`
//! - コースあたり 1 本 (実機は 2 本)。ブリッジをまたぐ 2 区間の結合も無い (Phase 4)
//! - 響板・箱・ROOM が無く、ブリッジ力の和がそのまま出力 (Phase 5–6)

use crate::segment::{DampingParams, Segment, SegmentParams};
use crate::Sample;

/// 最低音の MIDI ノート番号 (G2 = 98.00 Hz)。
///
/// 15/14 のアメリカ式はバス最低音が G2 前後。正確な配置表は Phase 3。
pub const KEY_MIN: u8 = 43;

/// 弦の本数。15/14 の発音数 (トレブル 15 × 2 区間 + バス 14) に合わせてある。
pub const STRING_COUNT: usize = 44;

/// 最高音の MIDI ノート番号 (D6 = 1174.66 Hz)。
pub const KEY_MAX: u8 = KEY_MIN + STRING_COUNT as u8 - 1;

/// 仮の設計則: 発弦長 × 基音 = 一定 (treble-long の実測に合わせる)。
///
/// この規則だと波動速度・張力・応力が全弦で同一になり、どの弦も
/// music wire の実用域に収まることが構築時に保証される。実機の寸法
/// (低音弦はもっと短く・太く) からは外れるが、**内部矛盾なく 44 本を
/// 張れる**ことを優先した暫定則。Phase 3 で実機の設計に置き換える。
const SCALE_M_HZ: f64 = 0.495 * 293.66;

/// ベロシティ 0–1 → 撥の速度 [m/s]。
///
/// 実機の「drop and bounce」は 1.7–2.4 m/s、強打で 6 m/s 程度。
/// pp = 0.5 m/s から ff = 6 m/s へ線形に割り付ける。
#[inline]
fn hammer_speed(velocity: f64) -> f64 {
    0.5 + 5.5 * velocity.clamp(0.0, 1.0)
}

/// MIDI ノート番号 → 12 平均律の周波数 [Hz]。
#[inline]
fn key_to_hz(key: u8) -> f64 {
    crate::A4_HZ * (((key as f64) - crate::A4_MIDI as f64) / 12.0).exp2()
}

/// 全弦バンク。
pub struct Instrument {
    strings: Vec<Segment>,
    /// 打弦点 x/L。次の打撃から効く (鳴っている弦の係数は触らない)
    strike_ratio: f64,
}

impl Instrument {
    /// 全弦を構築する。**確保はここだけ** (メインスレッドで呼ぶこと)。
    pub fn new(sample_rate: f64) -> Self {
        let strings = (0..STRING_COUNT)
            .map(|i| {
                let f0 = key_to_hz(KEY_MIN + i as u8);
                let params = SegmentParams {
                    length_m: SCALE_M_HZ / f0,
                    f0_hz: f0,
                    ..SegmentParams::treble_low_long()
                };
                let mut seg = Segment::new(params, sample_rate);
                // 減衰のアンカーを弦ごとの基音に置く。ダンパーの無い楽器の
                // 暫定値 (基音 10 秒 / 5 kHz で 0.8 秒)。実測での置き直しは Phase 3。
                seg.set_damping(DampingParams::from_t60_anchors(f0, 10.0, 5_000.0, 0.8));
                seg
            })
            .collect();

        Self {
            strings,
            strike_ratio: 0.09,
        }
    }

    /// 弦の本数。
    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// 打弦点 x/L を設定する。**次の打撃から効く。**
    ///
    /// 鳴っている弦の係数は触らない。係数の再構築 (モード数ぶんの sin 計算) を
    /// 打撃時だけに限ることで、パラメータのオートメーションが走っていても
    /// オーディオスレッドの負荷が増えない。
    pub fn set_strike_ratio(&mut self, ratio: f64) {
        self.strike_ratio = ratio.clamp(0.005, 0.5);
    }

    /// 打弦。`velocity` は 0–1。範囲外の鍵は何もしない。
    ///
    /// **弦の状態はリセットしない。** 鳴っている弦を叩けば足し込まれる。
    pub fn note_on(&mut self, key: u8, velocity: f64) {
        let ratio = self.strike_ratio;
        let Some(seg) = self.string_for(key) else {
            return;
        };
        // 打弦点が変わっていたときだけ係数を作り直す (約 75 モード × 2 レート)。
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
        if let Some(seg) = self.string_for(key) {
            seg.reset();
        }
    }

    /// 全弦を即座に消音する (ホストの停止・シーク)。
    pub fn reset(&mut self) {
        for seg in &mut self.strings {
            seg.reset();
        }
    }

    /// 1 ブロック処理する (上書き)。返り値はブロック内のピーク絶対値。
    ///
    /// 出力はブリッジ力の和 [N]。校正と 2ch 化は呼び出し側 (プラグイン層)。
    pub fn process(&mut self, out: &mut [Sample]) -> Sample {
        out.fill(0.0);
        // 弦ごとにブロックを回す (弦の係数と状態がキャッシュに乗ったまま使える)。
        for seg in &mut self.strings {
            for s in out.iter_mut() {
                *s += seg.process_sample();
            }
        }
        out.iter().fold(0.0 as Sample, |a, &b| a.max(b.abs()))
    }

    /// いずれかの撥が飛行・接触中か。眠ってよいかの判定に使う。
    pub fn any_hammer_active(&self) -> bool {
        self.strings.iter().any(|s| s.hammer().is_active())
    }

    /// 全弦の状態が有限か。
    pub fn is_finite(&self) -> bool {
        self.strings.iter().all(|s| s.is_finite())
    }

    /// 検証用: 指定した鍵の弦。
    pub fn string_params(&self, key: u8) -> Option<&SegmentParams> {
        let idx = self.index_of(key)?;
        Some(self.strings[idx].params())
    }

    fn index_of(&self, key: u8) -> Option<usize> {
        if (KEY_MIN..=KEY_MAX).contains(&key) {
            Some((key - KEY_MIN) as usize)
        } else {
            None
        }
    }

    fn string_for(&mut self, key: u8) -> Option<&mut Segment> {
        let idx = self.index_of(key)?;
        self.strings.get_mut(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_string_is_buildable_and_plausible() {
        let inst = Instrument::new(SR);
        assert_eq!(inst.string_count(), STRING_COUNT);
        for key in KEY_MIN..=KEY_MAX {
            let p = inst.string_params(key).expect("弦があること");
            // 仮の設計則では応力が全弦で同一 (663 MPa)。music wire の実用域。
            let mpa = p.stress_pa() / 1e6;
            assert!(
                (400.0..=1200.0).contains(&mpa),
                "key {key}: 応力 {mpa:.0} MPa"
            );
            assert!(p.mode_count(SR) > 0);
        }
    }

    #[test]
    fn a_note_rings_at_its_pitch() {
        let mut inst = Instrument::new(SR);
        inst.note_on(69, 0.7); // A4
        let x = render(&mut inst, 0.5);

        let on = magnitude_at(&x, 440.0);
        let off = magnitude_at(&x, 466.16); // 半音上
        assert!(
            on > off * 10.0,
            "A4 が 440 Hz で鳴っていない: {on:.3e} vs {off:.3e}"
        );
    }

    #[test]
    fn note_off_does_not_stop_the_ring() {
        // この楽器の定義そのもの。ノートオフの前後で減衰の速さが変わらない。
        //
        // **打撃スパイクを含む窓でピークを比べてはいけない。** ブリッジ力は
        // 接触中のスパイクが支配的 (クレストファクタ 20 倍超) なので、
        // 打鍵直後の窓と持続部の窓は同じ音でも桁が違う。先に 0.3 秒捨てて、
        // 持続部どうしを比べる。
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.8);
        render(&mut inst, 0.3); // 打撃の過渡を捨てる
        let before = render(&mut inst, 0.3);
        inst.note_off(60);
        let after = render(&mut inst, 0.3);

        let p_before = before.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        let p_after = after.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(p_before > 0.0);
        // T60 ≈ 10 秒の弦は 0.3 秒でほとんど減らない。半分より上なら鳴り続けている。
        assert!(
            p_after > p_before * 0.5,
            "ノートオフで音が切れている: {p_before:.3e} → {p_after:.3e}"
        );
    }

    #[test]
    fn restriking_adds_to_the_ringing_string() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.5);
        render(&mut inst, 0.2);
        // 鳴っている最中に叩き直しても壊れない (足し込まれる)。
        inst.note_on(60, 0.5);
        let x = render(&mut inst, 0.2);
        assert!(inst.is_finite());
        assert!(x.iter().any(|&s| s.abs() > 0.0));
    }

    #[test]
    fn out_of_range_keys_are_ignored() {
        let mut inst = Instrument::new(SR);
        inst.note_on(KEY_MIN - 1, 1.0);
        inst.note_on(KEY_MAX + 1, 1.0);
        inst.note_on(0, 1.0);
        inst.note_on(127, 1.0);
        let x = render(&mut inst, 0.1);
        assert!(x.iter().all(|&s| s == 0.0), "範囲外の鍵で音が出た");
    }

    #[test]
    fn choke_silences_only_that_string() {
        let mut inst = Instrument::new(SR);
        inst.note_on(60, 0.8);
        inst.note_on(67, 0.8);
        render(&mut inst, 0.2);

        inst.choke(60);
        let x = render(&mut inst, 0.3);

        let c4 = magnitude_at(&x, key_to_hz(60));
        let g4 = magnitude_at(&x, key_to_hz(67));
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
    fn repeated_ff_strikes_converge_to_a_bounded_roll() {
        // DAW のループ再生の再現 (D-016)。ff で 0.1 秒おきに 30 回叩き続けても
        // レベルが収束すること。T60 10 秒・打撃間隔 0.1 秒なら、定常のロールは
        // 単発の鳴りの十数倍で頭打ちになるはず。
        //
        // 撥の出発位置が弦の変位を無視していたときは、鳴っている弦を叩くたびに
        // 出発の瞬間の圧縮から数千 N のスパイクが出て、ループごとに音が
        // 大きくなり発散した (実機で LUFS +66 まで上昇)。
        let mut inst = Instrument::new(SR);

        // 単発の基準: 1 回叩いて 0.5 秒後の持続部のピーク。
        inst.note_on(60, 1.0);
        render(&mut inst, 0.5);
        let single = render(&mut inst, 0.2)
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        inst.reset();

        // 30 連打。
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
    fn all_strings_at_once_stay_finite() {
        let mut inst = Instrument::new(SR);
        for key in KEY_MIN..=KEY_MAX {
            inst.note_on(key, 1.0);
        }
        let x = render(&mut inst, 0.5);
        assert!(inst.is_finite());
        assert!(x.iter().all(|s| s.is_finite()));
    }
}
