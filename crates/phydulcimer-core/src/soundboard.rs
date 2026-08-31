//! 響板 — 並列 2 次共振器バンクによるフィルタ近似。
//!
//! 板の方程式は解かない。板の伝達関数を
//! 多数の共振器の並列和で近似する。実装は [`ModalBank`] をそのまま使う —
//! 響板のモードも弦のモードも「周波数・減衰・入出力重みを持つ 2 次共振器」で、
//! SIMD もタダで付いてくる。
//!
//! # 2 系統 (ゾーン)
//!
//! バスブリッジとトレブルブリッジは響板の**別の位置**に立つ。同じ板でも
//! 駆動点が違えばモードへの結合が違う。さらに
//! Phase 6 の X-Y ROOM では 2 つのブリッジに**別の角度**を与えて定位を
//! 幾何から出すので、出力も分かれている必要がある。
//!
//! そこで響板はゾーンごとに独立したバンクとして 2 つ持つ
//! ([`crate::engine`])。「物理的には 1 枚の板」から外れる近似だが、
//! モードの重みが駆動点で変わることの表現としては同等で、
//! 出力の分離がタダで手に入る。
//!
//! # モードの設計
//!
//! - 周波数: 対数等間隔 + ジッタ。**等間隔のままだと櫛になる**
//!   (モードの帯域幅より間隔が広い帯域では、部分音が谷に落ちると 20–30 dB 沈む)
//! - 減衰: 周波数依存 (低域ほど長い) × モードごとの散らし。**散らしの上限は
//!   クランプする** (クランプしないと鳴り止まないモードができ、ボイス回収や
//!   チョークを壊す)
//! - 入力重み: 駆動点でのモード振幅 (符号つき乱数)
//! - 出力重み: 放射先でのモード振幅 × **放射効率** (低域は板の表裏が打ち
//!   消して音にならない、P-039 の radiation) × 高域ロールオフ
//!
//! 乱数はゾーンの種から決定的に引く。ビルドごとに音が変わってはいけない。

use crate::modal::{decay_from_t60, ModalBank, ModeSpec, Rate};
use crate::Sample;

/// 1 ゾーンのモード数。
///
/// 2 ゾーン合計で 400 本。楽器が小さく実際のモード密度は低いが、
/// 櫛を避けるには本数で埋めるほうが効く。
pub const SOUNDBOARD_MODES: usize = 200;

/// 響板のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundboardParams {
    /// モードを置く下限 [Hz]。箱 (cabinet) がこの下を受け持つ
    pub f_min_hz: f64,
    /// モードを置く上限 [Hz]
    pub f_max_hz: f64,
    /// 低域側の T60 [s] (f_min でのアンカー)
    pub t60_low: f64,
    /// 高域側の T60 [s] (f_max でのアンカー)
    pub t60_high: f64,
    /// 放射効率の肩 [Hz]。波長が板より長い帯域は表裏が打ち消して音にならない
    pub radiation_hz: f64,
    /// 高域ロールオフの肩 [Hz]
    pub rolloff_hz: f64,
    /// 出力のスペクトル傾き指数 (`(f/1kHz)^tilt` を出力重みに掛ける)。
    ///
    /// 1.0 = 変位→速度の物理換算のみ。**それだけでは足りない** — IR の
    /// 1 秒平均で測ると −13 dB/oct 傾いた (短い T60 の高域は平均で沈む、
    /// 共振ピークが σ に反比例する、の重ね掛け)。バンドレベルがおおむね
    /// 平坦 + 緩い高域ロールオフになるよう実測で合わせた校正値。
    pub tilt_exp: f64,
    /// 全体ゲイン (校正値)
    pub gain: f64,
}

impl Default for SoundboardParams {
    /// ダルシマーの薄い響板の暫定値。
    ///
    /// **実測ではない。** 実機の板厚 ~6 mm・面積 ~0.3 m² から、ピアノ響板の
    /// 校正値 (低域 0.30 s / 高域 0.05 s 程度) を上限の目安として置いた。
    /// 掃引での置き直しは Phase 10。
    fn default() -> Self {
        Self {
            f_min_hz: 90.0,
            t60_low: 0.30,
            f_max_hz: 11_000.0,
            t60_high: 0.04,
            radiation_hz: 160.0,
            rolloff_hz: 4_500.0,
            tilt_exp: 2.5,
            gain: 1.0,
        }
    }
}

/// 出力の正規化。
///
/// [`ModalBank`] の入力ゲインはインパルス不変変換の `dt/ω_d` を含む
/// (弦では力 [N] → 変位 [m] の物理換算)。響板では入出力とも無次元の
/// 校正量なので、この物理スケールを打ち消して「1 N のインパルスで
/// O(0.1) の出力」に揃える。最終的な音量は [`SoundboardParams::gain`] と
/// エンジンの校正ゲインが決める。
const OUTPUT_NORM: f64 = 1.5e8;

/// 響板の 1 ゾーン。
pub struct Soundboard {
    bank: ModalBank,
}

/// 決定的な乱数 (xorshift32)。ビルドごとに音が変わってはいけない。
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// [0, 1)
    fn unit(&mut self) -> f64 {
        self.next() as f64 / u32::MAX as f64
    }

    /// [-1, 1) から 0 近傍を避けた符号つき振幅
    fn signed_amp(&mut self) -> f64 {
        let sign = if self.next() & 1 == 0 { 1.0 } else { -1.0 };
        sign * (0.3 + 0.7 * self.unit())
    }
}

impl Soundboard {
    /// `seed` はゾーンごとに変える (バス / トレブルで別のモード配置にする)。
    pub fn new(params: SoundboardParams, seed: u32, sample_rate: f64) -> Self {
        let mut bank = ModalBank::new();
        bank.set_active_modes(SOUNDBOARD_MODES);
        let mut rng = Rng(seed | 1);

        let dt = 1.0 / sample_rate;
        let nyquist_guard = sample_rate * 0.5 * 0.95;
        let count = SOUNDBOARD_MODES as f64;

        for i in 0..SOUNDBOARD_MODES {
            // 対数等間隔 + ±40% ジッタ (間隔に対して)。
            let t = (i as f64 + 0.5) / count;
            let base = params.f_min_hz * (params.f_max_hz / params.f_min_hz).powf(t);
            let spacing = base * ((params.f_max_hz / params.f_min_hz).ln() / count);
            let freq = (base + spacing * (rng.unit() - 0.5) * 0.8).min(nyquist_guard);

            // 減衰: 対数補間 + 散らし [0.7, 1.3]。上限はクランプ (P-039)。
            let k =
                (params.t60_high / params.t60_low).ln() / (params.f_max_hz / params.f_min_hz).ln();
            let t60_base = params.t60_low * (freq / params.f_min_hz).powf(k);
            let t60 = (t60_base * (0.7 + 0.6 * rng.unit())).min(params.t60_low * 1.2);

            // 放射効率 (低域を落とす) と高域ロールオフ。
            let radiation = freq * freq / (freq * freq + params.radiation_hz * params.radiation_hz);
            let rolloff = 1.0 / (1.0 + (freq / params.rolloff_hz).powf(1.3));

            let in_gain = rng.signed_amp();
            // スペクトル傾きの補正 (tilt_exp のコメント参照)。
            let tilt = (freq / 1_000.0).powf(params.tilt_exp);
            let out_gain =
                rng.signed_amp() * tilt * radiation * rolloff * params.gain * OUTPUT_NORM / count;

            bank.set_mode(
                i,
                Rate::Base,
                dt,
                &ModeSpec {
                    freq_hz: freq,
                    decay: decay_from_t60(t60),
                    strike_weight: in_gain,
                    modal_mass: 1.0,
                    out_weight: out_gain,
                    bridge_weight: 0.0,
                },
            );
        }

        Self { bank }
    }

    /// 1 サンプル: ブリッジ力 [N] を受けて放射音 (無次元、校正前) を返す。
    #[inline]
    pub fn process_sample(&mut self, bridge_force: Sample) -> Sample {
        self.bank.process_sample(bridge_force, Rate::Base)
    }

    pub fn reset(&mut self) {
        self.bank.reset();
    }

    pub fn is_finite(&self) -> bool {
        self.bank.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn impulse_response(sb: &mut Soundboard, n: usize) -> Vec<Sample> {
        let mut out = Vec::with_capacity(n);
        out.push(sb.process_sample(1.0));
        for _ in 1..n {
            out.push(sb.process_sample(0.0));
        }
        out
    }

    #[test]
    fn the_ir_rings_and_dies() {
        let mut sb = Soundboard::new(SoundboardParams::default(), 0xB0A2D, SR);
        let x = impulse_response(&mut sb, (SR * 1.0) as usize);
        assert!(x.iter().all(|s| s.is_finite()));

        let early = x[..4800].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        let late = x[x.len() - 4800..]
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(early > 0.0, "IR が出ていない");
        // 上限クランプが効いていれば、1 秒後には 60 dB 以上落ちている
        // (t60_low 0.30 × 1.2 = 0.36 s が最長)。
        assert!(
            late < early * 1e-3,
            "響板が鳴り止まない: {early:.3e} → {late:.3e} (P-039 のクランプ漏れ?)"
        );
    }

    #[test]
    fn different_seeds_give_different_boards() {
        let mut a = Soundboard::new(SoundboardParams::default(), 1, SR);
        let mut b = Soundboard::new(SoundboardParams::default(), 2, SR);
        let xa = impulse_response(&mut a, 4800);
        let xb = impulse_response(&mut b, 4800);
        assert_ne!(xa, xb, "ゾーンの種が効いていない");
    }

    #[test]
    fn the_same_seed_is_deterministic() {
        let mut a = Soundboard::new(SoundboardParams::default(), 7, SR);
        let mut b = Soundboard::new(SoundboardParams::default(), 7, SR);
        assert_eq!(
            impulse_response(&mut a, 4800),
            impulse_response(&mut b, 4800)
        );
    }

    #[test]
    fn the_radiation_knob_shapes_the_low_end() {
        // 放射効率の肩を動かすと低域のレベルが動くこと (つまみの直接検証)。
        let level_100hz = |radiation_hz: f64| -> f64 {
            let params = SoundboardParams {
                radiation_hz,
                ..SoundboardParams::default()
            };
            let mut sb = Soundboard::new(params, 0xB0A2D, SR);
            let x = impulse_response(&mut sb, (SR * 1.0) as usize);
            // Hann 窓を掛けること。掛けないと高域の過渡の漏れ込みが 100 Hz の
            // 読みを支配する (窓なしで書いて一度このテストを無意味にした)。
            let n = x.len();
            let w = std::f64::consts::TAU * 100.0 / SR;
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
        };
        let open = level_100hz(20.0); // ほぼ全部放射する
        let choked = level_100hz(500.0); // 100 Hz は肩の下
        assert!(
            open > choked * 3.0,
            "radiation_hz が効いていない: open {open:.3e}, choked {choked:.3e}"
        );
    }
}
