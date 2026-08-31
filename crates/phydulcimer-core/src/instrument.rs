//! 楽器全体 — 全弦の常時走行。
//!
//! **ボイスプールは無い。** 弦は楽器の固定資産として構築時に確保し、
//! `process` のたびに全部回す。打撃は既に振動している弦への力の注入であって、
//! ボイスの確保ではない。ダンパーの無い楽器ではこれが正しい形になる
//! (`docs/plan.html` §03)。
//!
//! - ノートオンは撥を起動するだけ。**弦の状態はリセットしない**
//!   (連打・ロールで振動に足し込まれる)
//! - ノートオフに対応する物理が存在しないので、**受け取っても捨てる**
//! - ホストの停止 (choke / reset) だけが弦を止める
//!
//! # 構成 (Phase 4)
//!
//! - 発音位置は [`Layout`](crate::layout::Layout) の 15/14 標準配置 (44 位置)
//! - **コースあたり 2 本の弦** ([`course`](crate::course))。2 本目は +1〜2 cent
//!   デチューンされ、打撃は 0–0.3 ms ばらつく → うなりと立ち上がりの厚み
//! - **トレブルの弦はブリッジをまたぐ 2 区間が結合している**
//!   ([`TrebleString`])。片側を叩くと反対側が共鳴する (5度の響き)
//! - 全音階配置なので、楽器に無い半音の MIDI 鍵は無音 (D-017)
//!
//! 区間の総数: バス 14 コース × 2 本 + トレブル 15 コース × 2 本 × 2 区間 = **88**。
//!
//! # まだ無いもの
//!
//! - 響板・箱・ROOM (Phase 5–6)。ブリッジ力の和がそのまま出力
//! - バスブリッジの未使用側の共鳴 (見送り、D-018)

use crate::course::{
    detuned, strike_pair, Strike, TrebleString, STRIKE_SPREAD_MAX_SEC, STRINGS_PER_COURSE,
};
use crate::hammer::HammerParams;
use crate::layout::{BridgeSide, Layout, Position};
use crate::scaling::design_position;
use crate::segment::Segment;
use crate::Sample;

/// 最低音の MIDI ノート番号 (G2 = 98.00 Hz)。
pub const KEY_MIN: u8 = 43;

/// 最高音の MIDI ノート番号 (D6 = 1174.66 Hz)。
pub const KEY_MAX: u8 = 86;

/// 発音位置の総数 (44)。
pub const STRING_COUNT: usize = crate::layout::POSITION_COUNT;

/// ベロシティ 0–1 → 撥の速度 [m/s]。
///
/// 実機の「drop and bounce」は 1.7–2.4 m/s、強打で 6 m/s 程度。
/// pp = 0.5 m/s から ff = 6 m/s へ線形に割り付ける。
#[inline]
fn hammer_speed(velocity: f64) -> f64 {
    0.5 + 5.5 * velocity.clamp(0.0, 1.0)
}

/// バスの 1 コース = 弦 2 本 (発音区間のみ。短い側は見送り、D-018)。
struct BassCourse {
    strings: [Segment; STRINGS_PER_COURSE],
}

/// トレブルの 1 コース = 弦 2 本、各弦が右・左の 2 区間を持つ。
struct TrebleCourse {
    strings: [TrebleString; STRINGS_PER_COURSE],
}

/// 全弦バンク。
pub struct Instrument {
    layout: Layout,
    bass: Vec<BassCourse>,
    treble: Vec<TrebleCourse>,
    /// 打弦点 x/L。次の打撃から効く
    strike_ratio: f64,
    /// 現在の撥 (Phase 7 で面の切り替えが入る)
    hammer: HammerParams,
    /// 打撃ばらつき用の PRNG (xorshift32)。オーディオスレッドで走るので自前
    rng: u32,
}

impl Instrument {
    /// 全弦を構築する。**確保はここだけ** (メインスレッドで呼ぶこと)。
    pub fn new(sample_rate: f64) -> Self {
        let layout = Layout::standard_15_14();

        let bass = (0..crate::layout::BASS_COURSES)
            .map(|course| {
                let p = position_of(&layout, BridgeSide::Bass, course);
                let (design, damping) = design_position(p);
                let base = design.segment_params();
                let cents = crate::course::unison_detune_cents(course);
                let strings = [base, detuned(base, cents)].map(|params| {
                    let mut seg = Segment::new(params, sample_rate);
                    seg.set_damping(damping);
                    seg
                });
                BassCourse { strings }
            })
            .collect();

        let treble = (0..crate::layout::TREBLE_COURSES)
            .map(|course| {
                let pr = position_of(&layout, BridgeSide::TrebleRight, course);
                let pl = position_of(&layout, BridgeSide::TrebleLeft, course);
                let (right_design, damping) = design_position(pr);
                let (left_design, _) = design_position(pl);
                let right = right_design.segment_params();
                let left = left_design.segment_params();
                let cents =
                    crate::course::unison_detune_cents(crate::layout::BASS_COURSES + course);
                let strings = [0, 1].map(|i| {
                    // 同じ弦なので右と左は同じデチューンを受ける (張り直しの残差)。
                    let c = if i == 0 { 0.0 } else { cents };
                    TrebleString::new(detuned(right, c), detuned(left, c), damping, sample_rate)
                });
                TrebleCourse { strings }
            })
            .collect();

        Self {
            layout,
            bass,
            treble,
            strike_ratio: 0.09,
            hammer: HammerParams::wood(),
            rng: 0x9E37_79B9,
        }
    }

    /// 発音位置の数 (44)。
    pub fn string_count(&self) -> usize {
        STRING_COUNT
    }

    /// 配置表。
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// 打弦点 x/L を設定する。**次の打撃から効く。**
    pub fn set_strike_ratio(&mut self, ratio: f64) {
        self.strike_ratio = ratio.clamp(0.005, 0.5);
    }

    /// ブリッジ結合の強さを変える (検証用)。0 で切断。
    pub fn set_bridge_coupling(&mut self, k: f64) {
        for course in &mut self.treble {
            for s in &mut course.strings {
                s.set_coupling(k);
            }
        }
    }

    /// 打弦。`velocity` は 0–1。楽器に無い鍵は何もしない。
    ///
    /// コースの 2 本の弦を、2 本目だけ 0–0.3 ms 遅らせて叩く。
    /// **弦の状態はリセットしない。**
    pub fn note_on(&mut self, key: u8, velocity: f64) {
        let Some(idx) = self.layout.preferred_index(key) else {
            return;
        };
        let position = self.layout.positions()[idx];

        let strike = Strike {
            velocity_mps: hammer_speed(velocity),
            second_delay_sec: self.next_spread(),
            strike_ratio: self.strike_ratio,
            hammer: self.hammer,
        };

        match position.side {
            BridgeSide::Bass => {
                let course = &mut self.bass[position.course];
                let [a, b] = &mut course.strings;
                strike_pair([a, b], &strike);
            }
            BridgeSide::TrebleRight => {
                let course = &mut self.treble[position.course];
                let [a, b] = &mut course.strings;
                strike_pair([a.right_mut(), b.right_mut()], &strike);
            }
            BridgeSide::TrebleLeft => {
                let course = &mut self.treble[position.course];
                let [a, b] = &mut course.strings;
                strike_pair([a.left_mut(), b.left_mut()], &strike);
            }
        }
    }

    /// ノートオフ。**何もしない。** ダンパーが無い。
    pub fn note_off(&mut self, _key: u8) {}

    /// その鍵のコースを即座に消音する (ホストの choke)。
    ///
    /// コース単位で止める (奏者が弦の束を掴む動作に相当)。トレブルは
    /// 反対側の区間も止める — 結合しているので、片側だけ止めても
    /// すぐ再励振されてしまう。
    pub fn choke(&mut self, key: u8) {
        let Some(idx) = self.layout.preferred_index(key) else {
            return;
        };
        let position = self.layout.positions()[idx];
        match position.side {
            BridgeSide::Bass => {
                for s in &mut self.bass[position.course].strings {
                    s.reset();
                }
            }
            BridgeSide::TrebleRight | BridgeSide::TrebleLeft => {
                for s in &mut self.treble[position.course].strings {
                    s.reset();
                }
            }
        }
    }

    /// 全弦を即座に消音する (ホストの停止・シーク)。
    pub fn reset(&mut self) {
        for c in &mut self.bass {
            for s in &mut c.strings {
                s.reset();
            }
        }
        for c in &mut self.treble {
            for s in &mut c.strings {
                s.reset();
            }
        }
    }

    /// 1 ブロック処理する (上書き)。返り値はブロック内のピーク絶対値。
    ///
    /// 出力はブリッジ力の和 [N]。校正と 2ch 化は呼び出し側 (プラグイン層)。
    pub fn process(&mut self, out: &mut [Sample]) -> Sample {
        out.fill(0.0);
        // 弦ごとにブロックを回す (係数と状態がキャッシュに乗ったまま使える)。
        // トレブルの右左の結合は TrebleString の中でサンプル単位に閉じている。
        for c in &mut self.bass {
            for seg in &mut c.strings {
                for s in out.iter_mut() {
                    *s += seg.process_sample();
                }
            }
        }
        for c in &mut self.treble {
            for string in &mut c.strings {
                for s in out.iter_mut() {
                    *s += string.process_sample();
                }
            }
        }
        out.iter().fold(0.0 as Sample, |a, &b| a.max(b.abs()))
    }

    /// 1 ブロックを**ブリッジごとの 2 バス**へ処理する (上書き)。
    ///
    /// バスブリッジとトレブルブリッジは響板の別の位置に立ち (Phase 5)、
    /// X-Y ROOM では別の角度を持つ (Phase 6)。そのため出力を混ぜずに返す。
    /// 2 つのスライスは同じ長さであること (短い方に合わせる)。
    pub fn process_buses(&mut self, bass_out: &mut [Sample], treble_out: &mut [Sample]) -> Sample {
        let len = bass_out.len().min(treble_out.len());
        let bass_out = &mut bass_out[..len];
        let treble_out = &mut treble_out[..len];
        bass_out.fill(0.0);
        treble_out.fill(0.0);

        for c in &mut self.bass {
            for seg in &mut c.strings {
                for s in bass_out.iter_mut() {
                    *s += seg.process_sample();
                }
            }
        }
        for c in &mut self.treble {
            for string in &mut c.strings {
                for s in treble_out.iter_mut() {
                    *s += string.process_sample();
                }
            }
        }

        let mut peak = 0.0 as Sample;
        for (&b, &t) in bass_out.iter().zip(treble_out.iter()) {
            peak = peak.max(b.abs()).max(t.abs());
        }
        peak
    }

    /// いずれかの撥が飛行・接触中か。眠ってよいかの判定に使う。
    pub fn any_hammer_active(&self) -> bool {
        self.bass
            .iter()
            .any(|c| c.strings.iter().any(|s| s.hammer().is_active()))
            || self
                .treble
                .iter()
                .any(|c| c.strings.iter().any(|s| s.any_hammer_active()))
    }

    /// 全弦の状態が有限か。
    pub fn is_finite(&self) -> bool {
        self.bass
            .iter()
            .all(|c| c.strings.iter().all(|s| s.is_finite()))
            && self
                .treble
                .iter()
                .all(|c| c.strings.iter().all(|s| s.is_finite()))
    }

    /// 検証用: 指定した鍵が叩く区間 (弦 1 本目) の設計値。未マップなら `None`。
    pub fn string_params(&self, key: u8) -> Option<&crate::segment::SegmentParams> {
        let idx = self.layout.preferred_index(key)?;
        let position = self.layout.positions()[idx];
        Some(match position.side {
            BridgeSide::Bass => self.bass[position.course].strings[0].params(),
            BridgeSide::TrebleRight => self.treble[position.course].strings[0].right().params(),
            BridgeSide::TrebleLeft => self.treble[position.course].strings[0].left().params(),
        })
    }

    /// 検証用: トレブルコースの弦 1 本目の左区間の変位 (結合の観測)。
    pub fn treble_left_displacement(&self, course: usize) -> Option<Sample> {
        self.treble
            .get(course)
            .map(|c| c.strings[0].left().displacement_at_strike())
    }

    /// 次の打撃ばらつき [s] (0–0.3 ms)。
    fn next_spread(&mut self) -> f64 {
        // xorshift32。品質は要らない。オーディオスレッドで確保なし。
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f64 / u32::MAX as f64) * STRIKE_SPREAD_MAX_SEC
    }
}

fn position_of(layout: &Layout, side: BridgeSide, course: usize) -> &Position {
    layout
        .positions()
        .iter()
        .find(|p| p.side == side && p.course == course)
        .expect("配置表に存在するコース")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn render(inst: &mut Instrument, seconds: f64) -> Vec<Sample> {
        let n = (SR * seconds) as usize;
        let mut out = vec![0.0 as Sample; n];
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
        assert_eq!(inst.string_count(), 44);
    }

    #[test]
    fn a_note_rings_at_its_pitch() {
        let mut inst = Instrument::new(SR);
        inst.note_on(69, 0.7); // A4 (トレブル右)
        let x = render(&mut inst, 0.5);

        let on = magnitude_at(&x, 440.0);
        let off = magnitude_at(&x, 466.16);
        assert!(
            on > off * 10.0,
            "A4 が 440 Hz で鳴っていない: {on:.3e} vs {off:.3e}"
        );
    }

    #[test]
    fn striking_the_right_side_rings_the_left_through_the_bridge() {
        // P4 の完了条件: 片側を叩いて反対側が鳴る。
        // G4 (67) はトレブル右コース 7。左区間 (D5) が結合で動き出す。
        let mut inst = Instrument::new(SR);
        assert_eq!(inst.treble_left_displacement(7), Some(0.0));
        inst.note_on(67, 0.9);
        render(&mut inst, 0.5);
        let left = inst.treble_left_displacement(7).unwrap().abs();

        // 結合を切ったときとの比で見る (デノーマル防止の DC が微小に乗るため)。
        let mut off = Instrument::new(SR);
        off.set_bridge_coupling(0.0);
        off.note_on(67, 0.9);
        render(&mut off, 0.5);
        let left_off = off.treble_left_displacement(7).unwrap().abs();

        assert!(
            left > left_off.max(1e-12) * 1e3,
            "左区間が結合で動いていない: 結合あり {left:.3e}, なし {left_off:.3e}"
        );
    }

    #[test]
    fn the_fifth_resonance_appears_in_the_spectrum() {
        // P4 の完了条件: 片側を叩いて反対側の 5 度が立つことをスペクトルで確認。
        //
        // 左区間の部分音のうち、右の部分音列と重ならないのは奇数次
        // (左 n=2m は右 n=3m と同じ場所に来る — それが 5 度の意味)。
        // 左の第 3 部分音 (~1770 Hz) は右の第 4 (1570) と第 5 (1963) の間で
        // 孤立しているので、そこにエネルギーが立つのは左が鳴っている証拠。
        // コース 7 左区間 (D5) の設計から、第 3 部分音の正確な位置を出す。
        let left_p3_hz = {
            use crate::layout::Layout;
            let layout = Layout::standard_15_14();
            let p = layout
                .positions()
                .iter()
                .find(|p| p.side == BridgeSide::TrebleLeft && p.course == 7)
                .unwrap();
            design_position(p).0.segment_params().partial_hz(3)
        };

        let level_at_left_p3 = |coupling: f64| -> f64 {
            let mut inst = Instrument::new(SR);
            inst.set_bridge_coupling(coupling);
            inst.note_on(67, 0.9); // G4 = トレブル右コース 7
            let x = render(&mut inst, 1.0);
            magnitude_at(&x[(SR * 0.3) as usize..], left_p3_hz)
        };

        let with = level_at_left_p3(crate::course::DEFAULT_BRIDGE_COUPLING);
        let without = level_at_left_p3(0.0);
        assert!(
            with > without * 10.0,
            "5 度の共鳴がスペクトルに立っていない: 結合あり {with:.3e}, なし {without:.3e}"
        );
    }

    #[test]
    fn unison_pair_beats() {
        // 2 本の弦のデチューンでうなりが出る。基音の包絡が変調される。
        // うなり周期は 1–2 cent → 0.25–0.5 Hz @ 440 Hz 程度なので、
        // 窓を粗く取って包絡の起伏を見る。
        let mut inst = Instrument::new(SR);
        inst.note_on(69, 0.8); // A4
        let x = render(&mut inst, 6.0);

        // 0.25 秒窓 × 20 点の基音包絡 (過渡の 1 秒は捨てる)。
        let win = (SR * 0.25) as usize;
        let series: Vec<f64> = (0..20)
            .map(|i| {
                let from = (SR * 1.0) as usize + i * win;
                magnitude_at(&x[from..from + win], 440.0)
            })
            .collect();

        // 指数減衰を dB 直線として除き、残差の起伏を見る。
        let db: Vec<f64> = series.iter().map(|&m| 20.0 * m.log10()).collect();
        let n = db.len() as f64;
        let mean_x = (n - 1.0) / 2.0;
        let mean_y = db.iter().sum::<f64>() / n;
        let sxx: f64 = (0..db.len()).map(|i| (i as f64 - mean_x).powi(2)).sum();
        let sxy: f64 = db
            .iter()
            .enumerate()
            .map(|(i, &y)| (i as f64 - mean_x) * (y - mean_y))
            .sum();
        let slope = sxy / sxx;
        let residual: Vec<f64> = db
            .iter()
            .enumerate()
            .map(|(i, &y)| y - (mean_y + slope * (i as f64 - mean_x)))
            .collect();
        let depth = residual.iter().cloned().fold(f64::MIN, f64::max)
            - residual.iter().cloned().fold(f64::MAX, f64::min);

        assert!(depth > 1.0, "うなりが出ていない: 包絡の起伏 {depth:.2} dB");
    }

    #[test]
    fn left_only_pitches_ring_with_the_pure_fifth_offset() {
        let mut inst = Instrument::new(SR);
        inst.note_on(85, 0.8); // C#6 (トレブル左のみ)
        let x = render(&mut inst, 0.5);

        let expected = inst.string_params(85).unwrap().f0_hz;
        let on = magnitude_at(&x, expected);
        let semitone_off = magnitude_at(&x, expected / 1.0595);
        assert!(on > semitone_off * 10.0, "C#6 が鳴っていない");
    }

    #[test]
    fn unmapped_chromatic_keys_are_silent() {
        let mut inst = Instrument::new(SR);
        for key in [44u8, 61, 68, 75, 84] {
            inst.note_on(key, 1.0);
        }
        inst.note_on(KEY_MIN - 1, 1.0);
        inst.note_on(KEY_MAX + 1, 1.0);
        let x = render(&mut inst, 0.1);
        // デノーマル防止の DC (−300 dB) が乗るので厳密 0 ではなく閾値で見る。
        assert!(x.iter().all(|&s| s.abs() < 1e-9), "無いはずの鍵で音が出た");
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
        // D-016 の回帰。弦 2 本 + 結合でも収束すること。
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
    fn choke_silences_the_whole_course() {
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
        // デノーマル防止の DC (−300 dB) が乗るので厳密 0 ではなく閾値で見る。
        assert!(x.iter().all(|&s| s.abs() < 1e-9));
        assert!(!inst.any_hammer_active());
    }

    #[test]
    fn strike_ratio_applies_to_the_next_strike() {
        let mut inst = Instrument::new(SR);
        inst.set_strike_ratio(0.125);
        inst.note_on(60, 0.8);
        let x = render(&mut inst, 0.4);

        let p = *inst.string_params(60).unwrap();
        let at = |n: usize| magnitude_at(&x, p.partial_hz(n));
        assert!(at(8) < at(7) * 0.02, "打弦点 1/8 のノッチが出ていない");
    }

    /// P4 の完了条件: 長時間の連続演奏で発散しない。
    ///
    /// 全 44 位置を ff で叩いて 60 秒回し、撥が離れた後のエネルギーが
    /// 単調に減ることを確かめる。
    #[test]
    fn a_full_strike_decays_monotonically_for_60_seconds() {
        let mut inst = Instrument::new(SR);
        for key in KEY_MIN..=KEY_MAX {
            inst.note_on(key, 1.0);
        }

        let mut rms_points = Vec::new();
        for _ in 0..12 {
            let x = render(&mut inst, 5.0);
            let rms: f64 =
                (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt();
            rms_points.push(rms);
        }
        assert!(inst.is_finite(), "60 秒で非有限値が出た");
        // 最初の窓は打撃の過渡を含むので 2 点目から見る。
        for w in rms_points.windows(2).skip(1) {
            assert!(
                w[1] < w[0] * 1.01,
                "エネルギーが減っていない: {rms_points:?}"
            );
        }
    }
}
