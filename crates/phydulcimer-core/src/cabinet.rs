//! 箱 — 音孔と空洞の低域共鳴。
//!
//! ギター・リュートと同じく、薄い響板 + 浅い箱 + 音孔の低域は
//! **ヘルムホルツ共鳴と板の最低次モードの 2 自由度結合**が作る
//! (Christensen & Vistisen 1980)。Vongsawad らはダルシマーで音孔が放射音に
//! 強く効くことを実測している。
//!
//! # 結合系をそのまま解かず、結合後の固有周波数を直接置く
//!
//! 2 自由度結合の可聴の帰結は「共鳴が 2 本に割れて双峰になる」こと。
//! 結合方程式を実行時に解く代わりに、**結合後の 2 つの固有周波数に共振器を
//! 直接置く**。線形系なので聴感上は等価で、実装は [`ModalBank`] の 2 モードで
//! 済む。結合前のパラメータ (音孔径・箱容積・板のコンプライアンス) から
//! 固有周波数を出す計算は設計時に済ませる (弦の設計則と同じ姿勢)。
//!
//! # 値の出どころ
//!
//! **実測ではない代表値。** ダルシマーの空洞は浅く (深さ ~70 mm、容積
//! ~10 L)、音孔は小さい。ギター (A0 ≈ 100 Hz / T1 ≈ 180 Hz) より容積が
//! 小さいぶんやや高めに置いた。掃引での置き直しは Phase 10。

use crate::modal::{decay_from_t60, ModalBank, ModeSpec, Rate};
use crate::Sample;

/// 箱のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CabinetParams {
    /// 結合後の低い方の固有周波数 [Hz] (ヘルムホルツ寄り)
    pub f_low_hz: f64,
    /// 結合後の高い方の固有周波数 [Hz] (板の最低次寄り)
    pub f_high_hz: f64,
    /// それぞれの T60 [s]
    pub t60_low: f64,
    pub t60_high: f64,
    /// 全体ゲイン (校正値)
    pub gain: f64,
}

impl Default for CabinetParams {
    fn default() -> Self {
        Self {
            f_low_hz: 110.0,
            f_high_hz: 195.0,
            t60_low: 0.20,
            t60_high: 0.15,
            gain: 1.0,
        }
    }
}

/// 出力の正規化 (soundboard の `OUTPUT_NORM` と同じ理由)。
///
/// IR のバンドレベルで、箱の双峰が響板の中域フロアの **+10〜15 dB** に
/// 立つよう実測で合わせた (最初 2.0e7 で置いたら +60 dB 突出した)。
const OUTPUT_NORM: f64 = 1.0e5;

/// 箱。全ブリッジ共有で 1 つ持つ。
pub struct Cabinet {
    bank: ModalBank,
}

impl Cabinet {
    pub fn new(params: CabinetParams, sample_rate: f64) -> Self {
        let mut bank = ModalBank::new();
        bank.set_active_modes(2);
        let dt = 1.0 / sample_rate;
        for (i, (f, t60)) in [
            (params.f_low_hz, params.t60_low),
            (params.f_high_hz, params.t60_high),
        ]
        .into_iter()
        .enumerate()
        {
            bank.set_mode(
                i,
                Rate::Base,
                dt,
                &ModeSpec {
                    freq_hz: f,
                    decay: decay_from_t60(t60),
                    strike_weight: 1.0,
                    modal_mass: 1.0,
                    out_weight: params.gain * OUTPUT_NORM,
                    bridge_weight: 0.0,
                },
            );
        }
        Self { bank }
    }

    /// 1 サンプル: ブリッジ力の和 [N] を受けて低域の放射を返す。
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

    #[test]
    fn the_cabinet_has_two_low_peaks() {
        let mut c = Cabinet::new(CabinetParams::default(), SR);
        let n = (SR * 1.0) as usize;
        let mut x = Vec::with_capacity(n);
        x.push(c.process_sample(1.0));
        for _ in 1..n {
            x.push(c.process_sample(0.0));
        }
        assert!(x.iter().all(|s| s.is_finite()));

        let mag = |f: f64| {
            let w = std::f64::consts::TAU * f / SR;
            let coeff = 2.0 * w.cos();
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for &v in &x {
                let s0 = v as f64 + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt()
        };

        // 双峰: 110 と 195 にピークがあり、間の谷 (150) と外 (60, 400) より強い。
        for peak in [110.0, 195.0] {
            for off in [60.0, 150.0, 400.0] {
                assert!(
                    mag(peak) > mag(off) * 2.0,
                    "{peak} Hz のピークが立っていない (vs {off} Hz)"
                );
            }
        }
    }
}
