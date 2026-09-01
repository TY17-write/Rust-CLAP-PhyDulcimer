//! コース — 同じ音位置に張られた弦の束と、ブリッジをまたぐ結合。
//!
//! 実機のコースは 2 本の弦で、トレブルコースの弦はブリッジをまたいで
//! **右 (長い側) と左 (5度上の短い側) の 2 区間**が同じ 1 本の弦に属する。
//!
//! # ブリッジ結合 (Phase 4 の中身)
//!
//! ブリッジは接着されておらず、弦の張力だけで押さえられた**有限インピーダンス
//! の支持点**。左右の区間はブリッジ点の動きを介して結合する。
//!
//! - 各区間がブリッジへ返す力は `F = Σ w_n·a_n` (`w_n = T·nπ/L`) — これは
//!   既に出力として計算している
//! - ブリッジ点の動きは両区間の力の和で決まり、各モードへ **同じ `w_n` の
//!   重みで** 戻る (出力の転置 = ランク1 の結合)
//!
//! 実装は**撥の接触力の順方向注入**: 片側を撥が押している間、その接触力を
//! ブリッジ重み (`w_n` × 高域テーパ) で反対区間へも注入する。
//!
//! ## 遅延帰還型を捨てた経緯 (実測、→ D-018)
//!
//! 「鳴っている両区間の力からブリッジ速度を作って戻す」形を 2 種類試し、
//! **どちらも使える強さで発散した**:
//!
//! 1. 変位比例 (バネ性支持点): k = 2e-5 で 0.05 秒後に 1e18
//! 2. 速度比例 (抵抗性支持点、LP 1.5 kHz + モード重みの高域テーパ付き):
//!    k = 1e-3 でも 1 秒以内に発散
//!
//! 原因は位相ではなく**弦の減衰の軽さ**。モードの固有減衰は 1 サンプルあたり
//! `σ·dt ≈ 1e-5` しかなく、1 サンプル遅延の帰還が持つ位相誤差の反減衰成分は、
//! 可聴レベルの結合強度で必ずこれを上回る。**明示的な (遅延つき) 帰還では、
//! この楽器の弦は結合できない。**
//!
//! 真の双方向は、注入が同サンプルの出力に現れる形へ組み替えた上での
//! **1×1 の陰的解**が必要 (計画どおり)。それは将来の仕事として D-018 に
//! 仕様を残し、Phase 4 では**閉ループを持たない順方向注入**を採る:
//!
//! - 撥の接触力は撥自身の動力学で決まり、駆動され続けない → 環は閉じない
//!   (両側の撥が同時に接触している 1–4 ms だけ弱く閉じるが、有界)
//! - 聴感上の主要な現象 —「片側を叩くと反対側が鳴り出す」— は接触中の
//!   エネルギー伝達そのもので、これで出る
//! - 失われるのは打撃後の弱い持続的交換 (二段減衰の微細構造)
//!
//! # 弦 2 本 (ユニゾン)
//!
//! - 2 本目は **+1〜2 cent デチューン** (コースごとに決まった量)。和がうなる
//! - 撥は 1 本の手で 2 本同時に叩くが、完全同時ではない。**0–0.3 ms の
//!   ばらつき**を打撃ごとに与える (実機のハンマーの傾き)
//!
//! うなりとダブルデケイの語彙で言うと: うなりはデチューンした 2 本の和として
//! 出る。弦間の結合 (Weinreich のダブルデケイ) はブリッジ経由の共有駆動が
//! 同じコースの 4 区間すべてに掛かることで弱く生じる。

use crate::hammer::HammerParams;
use crate::segment::{DampingParams, Segment, SegmentParams};
use crate::Sample;

/// ブリッジ結合の既定の強さ。
///
/// **物理定数ではなく校正値** (D-018)。接触力 [N] をブリッジ点の変位相当へ
/// 換算する係数で、逆数はブリッジの接触剛性 (1/1.5e-6 ≈ 0.7 N/µm — 木の
/// ブリッジキャップの接触として妥当なオーダー)。
///
/// 掃引 (右を v=3 m/s で打鍵、1 秒後の左右の変位レベル比):
///
/// | k | 左/右 |
/// |---|---|
/// | 1e-2 | +53 dB (非物理) |
/// | **1.5e-6 (採用)** | **−24 dB** |
/// | 1.5e-7 | −44 dB |
///
/// 反対区間の共鳴が主音の 20–30 dB 下、という聴感目標に合わせた。
pub const DEFAULT_BRIDGE_COUPLING: f64 = 1.5e-6;

/// コースあたりの弦の本数。実機は 2 本 (機種により 3–4 本)。
pub const STRINGS_PER_COURSE: usize = 2;

/// 2 本目の弦のデチューン [cent]。コース番号から決める。
///
/// 実機の調律残差は ±1–2 cent。コースごとに違う値にして、うなりの速さが
/// 音によって変わるようにする (全部同じだと機械的に聞こえる)。
pub fn unison_detune_cents(course_index: usize) -> f64 {
    // 0.8〜1.8 cent を決定的に散らす (黄金比の小数部)。
    let phase = (course_index as f64 * 0.618_033_988_749_895).fract();
    0.8 + phase
}

/// デチューンを周波数比へ。
fn cents_to_ratio(cents: f64) -> f64 {
    (cents / 1200.0).exp2()
}

/// トレブルの 1 本の弦 = ブリッジをまたぐ右・左の 2 区間 + 結合。
pub struct TrebleString {
    right: Segment,
    left: Segment,
    /// 結合の強さ = 接触力の透過率 (0 で切断)
    coupling: f64,
}

impl TrebleString {
    pub fn new(
        right_params: SegmentParams,
        left_params: SegmentParams,
        damping: DampingParams,
        sample_rate: f64,
    ) -> Self {
        let mut right = Segment::new(right_params, sample_rate);
        right.set_damping(damping);
        let mut left = Segment::new(left_params, sample_rate);
        left.set_damping(damping);
        Self {
            right,
            left,
            coupling: DEFAULT_BRIDGE_COUPLING,
        }
    }

    pub fn set_coupling(&mut self, k: f64) {
        self.coupling = k.max(0.0);
    }

    pub fn right(&self) -> &Segment {
        &self.right
    }

    pub fn left(&self) -> &Segment {
        &self.left
    }

    pub fn right_mut(&mut self) -> &mut Segment {
        &mut self.right
    }

    pub fn left_mut(&mut self) -> &mut Segment {
        &mut self.left
    }

    /// 1 サンプル進めて、ブリッジに加わる力の和 [N] を返す。
    ///
    /// 撥が片側を押している間、その接触力の一部が**同じサンプル内で**反対区間へ
    /// 注入される (前サンプルの接触力を使う。撥の力は駆動され続けないので
    /// 環は閉じない)。
    #[inline]
    pub fn process_sample(&mut self) -> Sample {
        // 反対側の撥の接触力 (前サンプル) を透過させる。符号は負 —
        // ブリッジは梃子で、片側を押し下げる力は反対側の端を押し下げる向きに
        // 働き、モード座標では逆符号になる (聴感上は位相の違いでしかない)。
        let drive_r = (-self.coupling * self.left.last_hammer_force() as f64) as Sample;
        let drive_l = (-self.coupling * self.right.last_hammer_force() as f64) as Sample;
        let f_right = self.right.process_sample_coupled(drive_r);
        let f_left = self.left.process_sample_coupled(drive_l);
        f_right + f_left
    }

    /// パームミュート量 0–1。手は弦 1 本を押さえれば両区間に効く。
    pub fn set_mute(&mut self, amount: f64) {
        self.right.set_mute(amount);
        self.left.set_mute(amount);
    }

    pub fn reset(&mut self) {
        self.right.reset();
        self.left.reset();
    }

    pub fn is_finite(&self) -> bool {
        self.right.is_finite() && self.left.is_finite()
    }

    pub fn any_hammer_active(&self) -> bool {
        self.right.hammer().is_active() || self.left.hammer().is_active()
    }

    /// 両区間とも眠っているか (P8)。
    ///
    /// 眠っている間は互いへのブリッジ駆動も 0 なので、弦ごとブロックを
    /// 飛ばしてよい (起きる契機は打弦だけで、それはブロック処理の外で起きる)。
    pub fn is_asleep(&self) -> bool {
        self.right.is_asleep() && self.left.is_asleep()
    }
}

/// 打撃時刻のばらつきの上限 [s]。実機のハンマーは 2 本を完全同時には叩かない。
pub const STRIKE_SPREAD_MAX_SEC: f64 = 0.3e-3;

/// 撥・打弦点を 2 本の弦に配る打撃の指示。
#[derive(Debug, Clone, Copy)]
pub struct Strike {
    /// 撥の速度 [m/s]
    pub velocity_mps: f64,
    /// 2 本目の遅れ [s] (0–[`STRIKE_SPREAD_MAX_SEC`])
    pub second_delay_sec: f64,
    /// 打弦点 x/L
    pub strike_ratio: f64,
    /// 撥の面
    pub hammer: HammerParams,
    /// 打撃ゲイン (面のラウドネス補償、[`crate::scaling::face_gain`])。1.0 で素通し
    pub gain: f64,
}

/// 2 本の弦それぞれの区間へ打撃を配る。
///
/// `segments` は同じ側 (右か左) の弦 1・弦 2 の区間。
pub fn strike_pair(segments: [&mut Segment; STRINGS_PER_COURSE], strike: &Strike) {
    for (i, seg) in segments.into_iter().enumerate() {
        if (seg.strike_ratio() - strike.strike_ratio).abs() > 1e-9 {
            seg.set_strike_ratio(strike.strike_ratio);
        }
        if seg.hammer().params() != &strike.hammer {
            seg.set_hammer_params(strike.hammer);
        }
        seg.set_strike_gain(strike.gain);
        let delay = if i == 0 { 0.0 } else { strike.second_delay_sec };
        seg.strike_delayed(strike.velocity_mps, delay);
    }
}

/// 2 本目の弦のパラメータ (デチューン済み) を作る。
pub fn detuned(params: SegmentParams, cents: f64) -> SegmentParams {
    SegmentParams {
        f0_hz: params.f0_hz * cents_to_ratio(cents),
        ..params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaling::{damping_for_course, design_position};
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    /// トレブルコース 7 (G4 / D5) の弦 1 本を作る。
    fn g4_string(coupling: f64) -> TrebleString {
        use crate::layout::{BridgeSide, Layout};
        let layout = Layout::standard_15_14();
        let find = |side: BridgeSide| {
            layout
                .positions()
                .iter()
                .find(|p| p.side == side && p.course == 7)
                .map(|p| design_position(p).0.segment_params())
                .unwrap()
        };
        let right = find(BridgeSide::TrebleRight);
        let left = find(BridgeSide::TrebleLeft);
        let mut s = TrebleString::new(right, left, damping_for_course(right.f0_hz), SR);
        s.set_coupling(coupling);
        s
    }

    fn run(s: &mut TrebleString, seconds: f64) -> Vec<Sample> {
        (0..(SR * seconds) as usize)
            .map(|_| s.process_sample())
            .collect()
    }

    /// 区間のモード変位のエネルギー的な量 (検証用)。
    fn segment_level(seg: &Segment) -> f64 {
        // 打弦点変位で代用する (状態が動いていれば非ゼロ)。
        seg.displacement_at_strike().abs() as f64
    }

    #[test]
    fn the_default_coupling_is_calibrated_to_minus_20_to_30_db() {
        // DEFAULT_BRIDGE_COUPLING の校正の固定。右を叩いて 1 秒後、
        // 左の変位が右の −20〜−30 dB に収まること。
        let mut s = g4_string(DEFAULT_BRIDGE_COUPLING);
        s.right_mut().strike(3.0);
        run(&mut s, 1.0);
        let ratio_db = 20.0 * (segment_level(s.left()) / segment_level(s.right())).log10();
        assert!(
            (-30.0..=-18.0).contains(&ratio_db),
            "共鳴レベルが校正から外れた: {ratio_db:.1} dB"
        );
    }

    #[test]
    fn striking_one_side_excites_the_other_through_the_bridge() {
        let mut s = g4_string(DEFAULT_BRIDGE_COUPLING);
        s.right_mut().strike(3.0);
        run(&mut s, 0.5);

        let left = segment_level(s.left());
        assert!(left > 0.0, "結合しているのに左区間が動いていない");

        // 結合を切ると左は完全に無音のまま。
        let mut off = g4_string(0.0);
        off.right_mut().strike(3.0);
        run(&mut off, 0.5);
        // デノーマル防止の DC (−300 dB) が乗るので閾値で見る。
        assert!(
            segment_level(off.left()) < 1e-12,
            "結合を切ったのに左区間が動いた: {:.3e}",
            segment_level(off.left())
        );
    }

    #[test]
    fn coupling_preserves_the_designed_decay() {
        // 結合は共鳴を足すためのもので、叩いた区間の減衰を大きく変えては
        // いけない (T60 の変化 < 10% が P4 の校正基準)。
        let t60_of = |coupling: f64| -> f64 {
            let mut s = g4_string(coupling);
            s.right_mut().strike(2.0);
            let x = run(&mut s, 2.5);
            let f0 = s.right().params().f0_hz;

            let mag = |from: usize, len: usize| {
                let seg = &x[from..from + len];
                let w = std::f64::consts::TAU * f0 / SR;
                let coeff = 2.0 * w.cos();
                let (mut s1, mut s2, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
                for (i, &v) in seg.iter().enumerate() {
                    let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
                    wsum += win;
                    let s0 = v as f64 * win + coeff * s1 - s2;
                    s2 = s1;
                    s1 = s0;
                }
                let re = s1 - s2 * w.cos();
                let im = s2 * w.sin();
                2.0 * (re * re + im * im).sqrt() / wsum
            };
            let early = mag((SR * 0.2) as usize, (SR * 0.2) as usize);
            let late = mag((SR * 2.0) as usize, (SR * 0.2) as usize);
            3.0 * std::f64::consts::LN_10 * 1.8 / (early / late).ln()
        };

        let free = t60_of(0.0);
        let coupled = t60_of(DEFAULT_BRIDGE_COUPLING);
        let change = (coupled - free).abs() / free;
        assert!(
            change < 0.10,
            "結合が減衰を変えすぎ: 自由 {free:.2} s → 結合 {coupled:.2} s ({:.0}%)",
            change * 100.0
        );
    }

    #[test]
    fn coupled_string_does_not_diverge_over_a_long_ring() {
        // P4 の完了条件: 発散しない。ここでは 20 秒 (楽器全体の 60 秒テストは
        // instrument 側)。撥が離れた後はエネルギーが単調に減ること。
        let mut s = g4_string(DEFAULT_BRIDGE_COUPLING * 4.0); // 余裕を見て強めで
        s.right_mut().strike(6.0);
        s.left_mut().strike(6.0);

        let mut rms_points = Vec::new();
        for _ in 0..10 {
            let x = run(&mut s, 2.0);
            let rms: f64 =
                (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt();
            rms_points.push(rms);
        }
        assert!(s.is_finite(), "非有限値が出た");
        // P8: 減衰し切って眠った後は厳密に 0 なので、等号 (0 == 0) を許す。
        for w in rms_points.windows(2).skip(1) {
            assert!(
                w[1] < w[0] * 1.02 || w[1] == 0.0,
                "エネルギーが減っていない: {rms_points:?}"
            );
        }
    }

    #[test]
    fn strike_spread_delays_the_second_string() {
        let layout_params = g4_string(0.0);
        let mut a = Segment::new(*layout_params.right().params(), SR);
        let mut b = Segment::new(*layout_params.right().params(), SR);

        let strike = Strike {
            velocity_mps: 2.0,
            second_delay_sec: 0.25e-3,
            strike_ratio: 0.09,
            hammer: HammerParams::wood(),
            gain: 1.0,
        };
        strike_pair([&mut a, &mut b], &strike);

        // 弦 1 は即座に接触し、弦 2 は遅れて接触する。
        let first_contact = |seg: &mut Segment| -> usize {
            for i in 0..200 {
                seg.process_sample();
                if seg.hammer().contact_duration() > 0.0 {
                    return i;
                }
            }
            usize::MAX
        };
        let ca = first_contact(&mut a);
        let cb = first_contact(&mut b);
        assert!(ca < 3, "弦 1 の接触が遅い: {ca}");
        let expected = (0.25e-3 * SR) as usize;
        assert!(
            cb >= expected.saturating_sub(2) && cb <= expected + 4,
            "弦 2 の遅れが {cb} サンプル (期待 {expected})"
        );
    }

    #[test]
    fn detune_shifts_the_fundamental_by_the_requested_cents() {
        let base = *g4_string(0.0).right().params();
        let det = detuned(base, 1.5);
        assert_relative_eq!(
            1200.0 * (det.f0_hz / base.f0_hz).log2(),
            1.5,
            epsilon = 1e-9
        );
        // 張力はわずかに変わる (同じ弦を張り直す) が、線密度・長さは同じ。
        assert_eq!(det.length_m, base.length_m);
        assert_eq!(det.diameter_m, base.diameter_m);
    }

    #[test]
    fn unison_detune_varies_by_course() {
        let a = unison_detune_cents(0);
        let b = unison_detune_cents(1);
        assert!((0.8..=1.8).contains(&a));
        assert!((0.8..=1.8).contains(&b));
        assert!((a - b).abs() > 0.05, "コースごとに違う値になっていない");
    }
}
