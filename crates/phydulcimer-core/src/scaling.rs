//! 弦の設計則 — 発音位置から物理パラメータを導く。
//!
//! 実際の楽器製作と同じ順序で導出する:
//!
//! 1. **幾何**からコースの発弦長を決める (台形の楽器なので上のコースほど短い)
//! 2. 目標周波数と長さから**波動速度** `c = 2·L·f0` が決まる
//! 3. **応力目標**から巻線の質量倍率を決める: `σ_core = w·ρ·c²` を
//!    目標 (650 MPa) に合わせる。`w < 1.25` なら素の鋼線 (w = 1)
//! 4. 線径は音域で補間し、張力 `T = μ·c²`・インハーモニシティ B が**導かれる**
//!
//! 導いた値が文献の範囲に入ることがテストで固定される。**参照音源を持たない
//! 本プロジェクトでは、これが唯一の外部基準** (`docs/problems.md` の D-006)。
//!
//! # トレブルの左右は同じ弦
//!
//! トレブルブリッジは弦長を **2:3** に分ける。左右は同じ弦 (同じ張力・同じ
//! 線密度) なので、左側の周波数は右側から**物理的に導かれる**:
//!
//! ```text
//! f_left = c / (2·L_left) = f_right · (L_right / L_left) = 1.5 · f_right
//! ```
//!
//! 1.5 倍 = **純正の完全5度 (702 cent)**。12 平均律の 700 cent とは 2 cent
//! 違う。実機どおりブリッジをちょうど 2:3 に置いた帰結で、バグではない
//! (→ D-017)。ブリッジ位置を動かして平均律に寄せるのは Phase 7。
//!
//! # 巻線 (wound strings)
//!
//! 低音は素の鋼線では張力が足りず緩んでしまう。巻線は**芯線に巻き付けた
//! 質量**で線密度を上げ、曲げ剛性はほぼ芯線のまま保つ。モデルでは
//! `SegmentParams::density` を `w·ρ_steel` にすることで表す:
//!
//! - 線密度 μ = w·ρ·A (重くなる)
//! - 応力 σ = T/A_core = w·ρ·c² (芯線が張力を受け持つ)
//! - B = π³·E·d_core⁴/(64·L²·T) (剛性は芯線のみ)

use crate::layout::{BridgeSide, Position};
use crate::segment::{DampingParams, SegmentParams, STEEL_DENSITY, STEEL_YOUNG};

/// トレブル最低コースの全長 [m]。Peterson の実測 (32.5 inch = 826 mm)。
const TREBLE_TOTAL_BOTTOM_M: f64 = 0.826;
/// トレブル最高コースの全長 [m]。台形の上辺側。
///
/// 一次資料の実測が無いので、**最高音 G5 の応力が music wire の実用上限
/// (約 1000 MPa) を超えない**ことから逆算した設計値。
const TREBLE_TOTAL_TOP_M: f64 = 0.36;

/// トレブルブリッジの分割: 長い側の割合 (2:3 の 3)。
const TREBLE_LONG_SHARE: f64 = 0.6;

/// バス最低コースの発弦長 [m] (使う側 = 長い側)。
const BASS_SPEAKING_BOTTOM_M: f64 = 0.74;
/// バス最高コースの発弦長 [m]。
const BASS_SPEAKING_TOP_M: f64 = 0.30;

/// 芯線の応力目標 [Pa]。
///
/// music wire の破断強度 ~2000 MPa の 3 割強。これより緩いと巻線にして
/// 質量を足し、張りを取り戻す (実機の低音弦が巻線である理由)。
const TARGET_STRESS_PA: f64 = 650.0e6;

/// これ未満の質量倍率なら巻かない (素の鋼線)。
///
/// 「1.1 倍だけ巻く」ような弦は現実には作らない。
const MIN_WRAP: f64 = 1.25;

/// 芯線の線径 [m]: 最低音コースと最高音コース。音域で線形補間する。
///
/// 実機のゲージは treble が 0.012–0.018 inch (0.30–0.46 mm)、bass の芯線が
/// それよりやや太い。ここは範囲に収まるよう置いた代表値。
const DIAMETER_LOW_M: f64 = 0.55e-3;
const DIAMETER_HIGH_M: f64 = 0.35e-3;

/// 基音の T60 アンカー [s]: 最低音 (98 Hz) と最高音コース (784 Hz)。
///
/// ダンパーの無い楽器の暫定値。低音ほど長く鳴る。対数補間する。
/// 実測での置き直しは Phase 10。
const T60_FUNDAMENTAL_LOW_S: f64 = 12.0;
const T60_FUNDAMENTAL_HIGH_S: f64 = 3.0;
/// 5 kHz での T60 [s]。全弦共通。
const T60_AT_5K_S: f64 = 0.6;

/// MIDI ノート番号 → 12 平均律の周波数 [Hz]。
pub fn key_to_hz(key: u8) -> f64 {
    crate::A4_HZ * (((key as f64) - crate::A4_MIDI as f64) / 12.0).exp2()
}

/// 0–1 の線形補間。
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// 1 本の弦 (コース) の設計。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StringDesign {
    /// 発弦長 [m]
    pub speaking_m: f64,
    /// 基音 [Hz]
    pub f0_hz: f64,
    /// 芯線の線径 [m]
    pub diameter_m: f64,
    /// 巻線の質量倍率 (1.0 = 素の鋼線)
    pub wrap: f64,
}

impl StringDesign {
    /// [`SegmentParams`] へ落とす。巻線は実効密度で表す。
    pub fn segment_params(&self) -> SegmentParams {
        SegmentParams {
            length_m: self.speaking_m,
            f0_hz: self.f0_hz,
            diameter_m: self.diameter_m,
            density: self.wrap * STEEL_DENSITY,
            young: STEEL_YOUNG,
        }
    }
}

/// 長さと目標周波数から巻線倍率を決める。
fn wrap_for(speaking_m: f64, f0_hz: f64) -> f64 {
    let c = 2.0 * speaking_m * f0_hz;
    let w = TARGET_STRESS_PA / (STEEL_DENSITY * c * c);
    if w < MIN_WRAP {
        1.0
    } else {
        w
    }
}

/// 音域による線径 [m]。`t` は 0 (最低音) 〜 1 (最高音)。
fn diameter_at(t: f64) -> f64 {
    lerp(DIAMETER_LOW_M, DIAMETER_HIGH_M, t.clamp(0.0, 1.0))
}

/// 鍵から音域位置 t (0–1) を出す。G2 (43) 〜 D6 (86)。
fn register_t(midi: u8) -> f64 {
    (midi as f64 - 43.0) / (86.0 - 43.0)
}

/// コースの基音に応じた減衰設計。
///
/// 左右の区間は同じ弦なので、**コースの (右側の) 基音**でアンカーを置き、
/// 両区間に同じ係数を使う。
pub fn damping_for_course(course_f0_hz: f64) -> DampingParams {
    // 対数補間: t60(f) = T_low · (f/98)^k、k = ln(T_high/T_low)/ln(784/98)。
    let k = (T60_FUNDAMENTAL_HIGH_S / T60_FUNDAMENTAL_LOW_S).ln() / (784.0f64 / 98.0).ln();
    let t60_low = T60_FUNDAMENTAL_LOW_S * (course_f0_hz / 98.0).powf(k);
    DampingParams::from_t60_anchors(course_f0_hz, t60_low, 5_000.0, T60_AT_5K_S)
}

/// 1 つの発音位置の設計 (弦 + 減衰)。
pub fn design_position(position: &Position) -> (StringDesign, DampingParams) {
    match position.side {
        BridgeSide::Bass => {
            let t = position.course as f64 / (crate::layout::BASS_COURSES - 1) as f64;
            let speaking = lerp(BASS_SPEAKING_BOTTOM_M, BASS_SPEAKING_TOP_M, t);
            let f0 = key_to_hz(position.midi);
            let design = StringDesign {
                speaking_m: speaking,
                f0_hz: f0,
                diameter_m: diameter_at(register_t(position.midi)),
                wrap: wrap_for(speaking, f0),
            };
            (design, damping_for_course(f0))
        }
        BridgeSide::TrebleRight | BridgeSide::TrebleLeft => {
            let t = position.course as f64 / (crate::layout::TREBLE_COURSES - 1) as f64;
            let total = lerp(TREBLE_TOTAL_BOTTOM_M, TREBLE_TOTAL_TOP_M, t);
            // 右側 (長い側) の設計が弦を決める。
            let right_midi = 55 + course_offset(position.course);
            let f_right = key_to_hz(right_midi);
            let l_right = total * TREBLE_LONG_SHARE;
            let diameter = diameter_at(register_t(right_midi));
            let wrap = wrap_for(l_right, f_right);
            let damping = damping_for_course(f_right);

            let design = match position.side {
                BridgeSide::TrebleRight => StringDesign {
                    speaking_m: l_right,
                    f0_hz: f_right,
                    diameter_m: diameter,
                    wrap,
                },
                _ => {
                    // 左側: 同じ弦の短い区間。周波数は物理から導かれる
                    // (純正5度 = 1.5 倍、12 平均律より +2 cent)。
                    let l_left = total * (1.0 - TREBLE_LONG_SHARE);
                    StringDesign {
                        speaking_m: l_left,
                        f0_hz: f_right * (TREBLE_LONG_SHARE / (1.0 - TREBLE_LONG_SHARE)),
                        diameter_m: diameter,
                        wrap,
                    }
                }
            };
            (design, damping)
        }
    }
}

/// トレブルのコース番号 → G メジャースケール上の半音オフセット。
fn course_offset(course: usize) -> u8 {
    const MAJOR: [u8; 7] = [2, 2, 1, 2, 2, 2, 1];
    (0..course).map(|i| MAJOR[i % 7]).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{BridgeSide, Layout};
    use approx::assert_relative_eq;

    /// **P3 の完了条件**: 導いた設計が文献の範囲に入ること。
    /// 参照音源を持たないプロジェクトでは、これが唯一の外部基準になる。
    #[test]
    fn every_string_is_within_published_ranges() {
        let layout = Layout::standard_15_14();
        for p in layout.positions() {
            let (design, _) = design_position(p);
            let params = design.segment_params();
            let name = crate::layout::note_name(p.midi);

            // 芯線の応力: music wire の実用域 (破断 ~2000 MPa の 15–55%)。
            let mpa = params.stress_pa() / 1e6;
            assert!(
                (300.0..=1100.0).contains(&mpa),
                "{name} ({:?}): 応力 {mpa:.0} MPa",
                p.side
            );

            // 張力: 実機は 1 本あたり 60–110 N (15–25 lbs) 程度。余裕を見て 40–220。
            let t = params.tension();
            assert!(
                (40.0..=220.0).contains(&t),
                "{name} ({:?}): 張力 {t:.0} N",
                p.side
            );

            // 芯線の線径: 0.30–0.60 mm (0.012–0.024 inch)。
            let mm = design.diameter_m * 1e3;
            assert!((0.30..=0.60).contains(&mm), "{name}: 線径 {mm:.2} mm");

            // インハーモニシティ: 実測研究の報告域 (1e-5 〜 数e-3)。
            let b = params.inharmonicity();
            assert!(
                (1.0e-5..=5.0e-3).contains(&b),
                "{name} ({:?}): B = {b:.2e}",
                p.side
            );

            // モード数が枠に収まる (48 kHz)。
            let modes = params.mode_count(48_000.0);
            assert!(modes >= 8, "{name}: モードが {modes} 本しかない");
        }
    }

    #[test]
    fn low_strings_are_wound_and_high_strings_are_plain() {
        let layout = Layout::standard_15_14();
        let design_of = |midi: u8, side: BridgeSide| {
            layout
                .positions()
                .iter()
                .find(|p| p.midi == midi && p.side == side)
                .map(|p| design_position(p).0)
                .unwrap()
        };

        // バス最低音 G2 は巻線 (実機どおり)。
        let g2 = design_of(43, BridgeSide::Bass);
        assert!(g2.wrap > 2.0, "G2 が巻線になっていない: w = {}", g2.wrap);

        // トレブル最高音 G5 は素の鋼線。
        let g5 = design_of(79, BridgeSide::TrebleRight);
        assert_eq!(g5.wrap, 1.0, "G5 が巻線になっている");

        // 巻線倍率は低音ほど大きい (単調とまでは言わないが、端で比較する)。
        assert!(g2.wrap > design_of(62, BridgeSide::Bass).wrap);
    }

    #[test]
    fn treble_left_is_a_pure_fifth_above_right() {
        let layout = Layout::standard_15_14();
        for course in 0..crate::layout::TREBLE_COURSES {
            let find = |side: BridgeSide| {
                layout
                    .positions()
                    .iter()
                    .find(|p| p.side == side && p.course == course)
                    .map(|p| design_position(p).0)
                    .unwrap()
            };
            let right = find(BridgeSide::TrebleRight);
            let left = find(BridgeSide::TrebleLeft);

            // 純正5度 (1.5 倍ちょうど)。
            assert_relative_eq!(left.f0_hz / right.f0_hz, 1.5, epsilon = 1e-12);

            // 同じ弦なので張力が一致する (物理の整合性)。
            let t_r = right.segment_params().tension();
            let t_l = left.segment_params().tension();
            assert_relative_eq!(t_r, t_l, max_relative = 1e-9);
        }
    }

    #[test]
    fn left_side_is_two_cents_above_equal_temperament() {
        // 純正5度 (702 cent) と平均律 (700 cent) の差。D-017 の記録どおり。
        let layout = Layout::standard_15_14();
        let p = layout
            .positions()
            .iter()
            .find(|p| p.side == BridgeSide::TrebleLeft && p.course == 0)
            .unwrap();
        let (design, _) = design_position(p);
        let tet = key_to_hz(p.midi); // D4 = 293.66
        let cents = 1200.0 * (design.f0_hz / tet).log2();
        assert!(
            (1.5..=2.5).contains(&cents),
            "左側の音高が平均律から {cents:.2} cent (期待 +2)"
        );
    }

    #[test]
    fn damping_is_longer_in_the_bass() {
        let low = damping_for_course(98.0);
        let high = damping_for_course(784.0);
        assert_relative_eq!(low.t60_at(98.0), T60_FUNDAMENTAL_LOW_S, max_relative = 1e-9);
        assert_relative_eq!(
            high.t60_at(784.0),
            T60_FUNDAMENTAL_HIGH_S,
            max_relative = 1e-9
        );
        assert!(low.t60_at(98.0) > high.t60_at(784.0));
        // 5 kHz のアンカーは共通。
        assert_relative_eq!(low.t60_at(5_000.0), T60_AT_5K_S, max_relative = 1e-9);
        assert_relative_eq!(high.t60_at(5_000.0), T60_AT_5K_S, max_relative = 1e-9);
    }

    #[test]
    fn course_offsets_follow_the_major_scale() {
        // G3 (55) + offset がトレブル右の音列 G3 A3 B3 C4 D4... になる。
        let expected = [0u8, 2, 4, 5, 7, 9, 11, 12, 14, 16, 17, 19, 21, 23, 24];
        for (course, &off) in expected.iter().enumerate() {
            assert_eq!(course_offset(course), off, "course {course}");
        }
    }
}
