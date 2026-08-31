//! Phase 0 の疎通確認用の発振器。
//!
//! モデル本体ではない。`tools/render` → WAV → `tools/analyze` の経路が正しく
//! つながっているかを、**解析解が既知の信号**で検証するために置いている。
//!
//! 周波数と減衰時間 (T60) が厳密に分かっているので、`tools/analyze` の
//! 周波数推定と T60 推定そのものの正しさをここで先に確かめられる。Phase 1 で
//! モーダル弦を実装したとき、解析ツール側のバグと弦モデルのバグを切り分けられる。

use crate::Sample;

/// 指数減衰する正弦波。
///
/// 位相アキュムレータ方式。2次共振器 (Phase 1 の `modal`) と違い、係数量子化や
/// デノーマルの影響を受けないので、参照信号として使える。
#[derive(Debug, Clone)]
pub struct DecayingSine {
    /// 1 サンプルあたりの位相増分 [rad]
    phase_inc: f64,
    /// 現在位相 [rad]
    phase: f64,
    /// 1 サンプルあたりの振幅減衰率
    amp_decay: f64,
    /// 現在振幅
    amp: f64,
}

impl DecayingSine {
    /// # 引数
    /// - `freq_hz`: 周波数 [Hz]
    /// - `t60_sec`: 振幅が 60 dB 減衰するまでの時間 [s]。`f64::INFINITY` で減衰なし
    /// - `sample_rate`: サンプリング周波数 [Hz]
    /// - `amplitude`: 初期振幅
    pub fn new(freq_hz: f64, t60_sec: f64, sample_rate: f64, amplitude: f64) -> Self {
        // 60 dB = 振幅 1/1000。t60 の間に 10^(-3) になるような 1 サンプルあたりの率。
        let amp_decay = if t60_sec.is_finite() && t60_sec > 0.0 {
            (-3.0 * std::f64::consts::LN_10 / (t60_sec * sample_rate)).exp()
        } else {
            1.0
        };

        Self {
            phase_inc: std::f64::consts::TAU * freq_hz / sample_rate,
            phase: 0.0,
            amp_decay,
            amp: amplitude,
        }
    }

    /// 1 サンプル進める。
    #[inline]
    pub fn next_sample(&mut self) -> Sample {
        let out = self.amp * self.phase.sin();

        self.phase += self.phase_inc;
        if self.phase >= std::f64::consts::TAU {
            self.phase -= std::f64::consts::TAU;
        }
        self.amp *= self.amp_decay;

        out as Sample
    }

    /// バッファへ加算する (置き換えではない)。複数本を重ねられるようにしてある。
    pub fn add_to(&mut self, buf: &mut [Sample]) {
        for s in buf.iter_mut() {
            *s += self.next_sample();
        }
    }

    /// 現在の振幅。
    #[inline]
    pub fn amplitude(&self) -> f64 {
        self.amp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    #[test]
    fn undamped_sine_keeps_its_amplitude() {
        let mut osc = DecayingSine::new(1_000.0, f64::INFINITY, SR, 1.0);
        let mut buf = vec![0.0 as Sample; 48_000];
        osc.add_to(&mut buf);

        let peak = buf.iter().fold(0.0 as Sample, |a, b| a.max(b.abs()));
        assert_relative_eq!(peak, 1.0, epsilon = 1e-3);
        assert_relative_eq!(osc.amplitude(), 1.0, epsilon = 1e-12);
    }

    #[test]
    fn t60_decays_by_60db() {
        let t60 = 0.5;
        let mut osc = DecayingSine::new(440.0, t60, SR, 1.0);
        let n = (t60 * SR) as usize;
        let mut buf = vec![0.0 as Sample; n];
        osc.add_to(&mut buf);

        // t60 経過後の振幅は初期値の 1/1000 (= -60 dB)。
        assert_relative_eq!(osc.amplitude(), 1e-3, max_relative = 1e-3);
    }

    #[test]
    fn zero_crossings_give_the_right_frequency() {
        // 上向きゼロクロス数から周波数を数える。ここが合っていれば位相増分は正しい。
        let freq = 500.0;
        let mut osc = DecayingSine::new(freq, f64::INFINITY, SR, 1.0);
        let mut buf = vec![0.0 as Sample; SR as usize];
        osc.add_to(&mut buf);

        let crossings = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        // 1 秒間なのでゼロクロス数 ≒ 周波数。端の 1 周期分の誤差を許容する。
        assert!(
            (crossings as f64 - freq).abs() <= 1.0,
            "crossings={crossings}, expected≈{freq}"
        );
    }

    #[test]
    fn add_to_accumulates_rather_than_overwrites() {
        let mut buf = vec![0.0 as Sample; 128];
        DecayingSine::new(100.0, f64::INFINITY, SR, 1.0).add_to(&mut buf);
        let single = buf.clone();

        DecayingSine::new(100.0, f64::INFINITY, SR, 1.0).add_to(&mut buf);
        for (i, (&sum, &one)) in buf.iter().zip(single.iter()).enumerate() {
            assert_relative_eq!(sum, one * 2.0, epsilon = 1e-6, max_relative = 1e-6);
            assert!(sum.is_finite(), "non-finite at {i}");
        }
    }
}
