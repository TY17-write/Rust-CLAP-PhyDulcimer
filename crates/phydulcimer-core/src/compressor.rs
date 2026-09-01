//! ビルトインコンプレッサ (Phase 10 後半、D-029)。
//!
//! ダルシマーの主奏法は両手 — 2 和音とロール。ダンパーが無いので、ロールでは
//! 持続的な鳴りが積み上がって新しい打撃を埋め、音量の天井はソフトクリップが
//! 乱暴に受けることになる。ここに置くのは**その積み上がりを押さえて
//! 個々の打撃を浮き出させる** (粒立ち) ためのバスコンプレッサ:
//!
//! - **アタックを通す**: アタックタイム (40 ms) の間、打撃の立ち上がりは
//!   ゲインリダクションに捕まらず素通りし、さらにオンセットブーストが
//!   立ち上がりだけを持ち上げる → 各打撃が持続音の上に立つ
//! - **持続を押さえる**: 鳴りの積み上がり (しきい値超過) は押し下げられ、
//!   ソフトクリップに常時もたれない
//! - **リリースはロールの間隔に合わせる** (180 ms ≈ 6–12 Hz のロールで
//!   打撃間に部分回復し、打撃ごとにリダクションが再駆動する)
//!
//! # 挿入位置と X-Y の保全
//!
//! 響板後・ROOM 前。検出は**バス + トレブルの和** (モノ) で行い、同じゲインを
//! 両系統に掛ける — L/R 間に差を作らないので X-Y の性質 (遅延差ゼロ・
//! 定位はレベル差のみ) を壊さない。部屋は「圧縮された楽器」を録る形になり、
//! 残響尾はポンピングしない。
//!
//! # 校正との関係 (D-029)
//!
//! **エンジンの既定は off (amount = 0)。** 音域バランス・面の補償・LUFS の
//! 校正体系はすべてコンプ off で測っており、これは ROOM と同じ扱い
//! (測定では切る)。プラグインのパラメータ既定が on にして、演奏体験にだけ
//! 乗せる。しきい値以下 (単発の弱音) は素通りする。

use crate::Sample;

/// しきい値 [dBFS] (検出は響板後・校正ゲイン後の和に対して)。
///
/// **校正値** (D-029)。ff の 2 鍵ロールの持続ウォッシュ (積分 −18 LUFS 前後、
/// 包絡 −20 dBFS 台) に掛かり、mf の単発 (それより 10 dB 以上下) は
/// 素通りする位置。
pub const THRESHOLD_DB: f64 = -24.0;

/// レシオ。3:1 — バスコンプの穏当な値。潰し切らずに積み上がりだけ払う。
pub const RATIO: f64 = 3.0;

/// ソフトニーの幅 [dB]。しきい値の前後 ±3 dB で滑らかに立ち上がる。
pub const KNEE_DB: f64 = 6.0;

/// アタック [s]。打撃のピーク (接触 + 響板の過渡 ≈ 5–10 ms) を**丸ごと
/// 逃がす**長さ。実測 (D-029): 8 ms ではピーク自体が捕まって粒立ちが
/// 逆に下がった (6.7 → 5.9 dB)。ゲインリダクションは打撃の後の持続部で
/// 立ち上がり、次の打撃までに部分回復する — この位相差が粒立ちを作る。
pub const ATTACK_SEC: f64 = 0.040;

/// リリース [s]。ロールの打撃間隔 (6–12 Hz = 83–167 ms) で部分回復する長さ。
pub const RELEASE_SEC: f64 = 0.18;

/// amount = 1 でのメイクアップ [dB]。押さえたぶんの半分程度を返し、
/// コンプ on/off で聴感の音量が大きく飛ばないようにする (実測で決めた校正値)。
pub const MAKEUP_DB: f64 = 2.0;

/// トランジェント強調 (粒立ち) の速い包絡のアタック/リリース [s]。
///
/// 下方向コンプだけでは周期内の max/min (粒立ちの指標) は上がらない —
/// トラフの最小値は次打の直前 = ゲインが最も回復した位相にあるため
/// (実測、D-029)。そこで**打撃のオンセットだけ**を持ち上げる:
/// 速い包絡 (1.5 ms / 30 ms) が遅い包絡 (上のアタック/リリース) を
/// 上回っている間 = 立ち上がりの数十 ms だけブースト。定常では両者が
/// 一致してブースト 0 になる。
pub const PUNCH_ATTACK_SEC: f64 = 0.0015;
pub const PUNCH_RELEASE_SEC: f64 = 0.030;

/// オンセット超過 [dB] → ブースト [dB] の傾き。
pub const PUNCH_SLOPE: f64 = 0.5;

/// ブーストの上限 [dB] (amount = 1)。無音からの初打は超過が巨大になるので
/// ここで頭打ちにする。
pub const PUNCH_MAX_DB: f64 = 4.0;

/// バスコンプレッサ本体。状態は包絡 2 つ (遅い = GR 用、速い = オンセット用)。
#[derive(Debug, Clone)]
pub struct Compressor {
    /// ピーク包絡 (リニア、遅い — ゲインリダクション用)
    env: f64,
    /// ピーク包絡 (リニア、速い — オンセット検出用)
    env_fast: f64,
    attack_coef: f64,
    release_coef: f64,
    fast_attack_coef: f64,
    fast_release_coef: f64,
}

impl Compressor {
    pub fn new(sample_rate: f64) -> Self {
        let coef = |sec: f64| 1.0 - (-1.0 / (sample_rate.max(1.0) * sec)).exp();
        Self {
            env: 0.0,
            env_fast: 0.0,
            attack_coef: coef(ATTACK_SEC),
            release_coef: coef(RELEASE_SEC),
            fast_attack_coef: coef(PUNCH_ATTACK_SEC),
            fast_release_coef: coef(PUNCH_RELEASE_SEC),
        }
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.env_fast = 0.0;
    }

    /// 1 サンプルぶんのゲイン (リニア) を返す。
    ///
    /// `level` は検出信号 (バス + トレブルの和)、`amount` は 0–1 の効き量。
    /// amount はリダクションとメイクアップの dB を線形にスケールするので、
    /// 0 で厳密に 1.0 (素通し)、1 で全量になる。
    #[inline]
    pub fn gain(&mut self, level: Sample, amount: f64) -> Sample {
        // 包絡 (ピーク追従、アタック/リリースの 1 次平滑) × 2 系統。
        let x = (level as f64).abs();
        let coef = if x > self.env {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.env += (x - self.env) * coef;
        let coef_fast = if x > self.env_fast {
            self.fast_attack_coef
        } else {
            self.fast_release_coef
        };
        self.env_fast += (x - self.env_fast) * coef_fast;

        if amount <= 0.0 {
            return 1.0;
        }

        // ソフトニー付きのゲインカーブ (下方向)。
        let env_db = 20.0 * self.env.max(1e-9).log10();
        let over = env_db - THRESHOLD_DB;
        let slope = 1.0 - 1.0 / RATIO;
        let half = KNEE_DB * 0.5;
        let reduction_db = if over <= -half {
            0.0
        } else if over < half {
            let t = over + half;
            slope * t * t / (2.0 * KNEE_DB)
        } else {
            slope * over
        };

        // トランジェント強調 (粒立ち): 速い包絡が遅い包絡を上回っている
        // 間だけブースト。定常 (ロールのウォッシュ・持続音) では 0。
        let onset_db = 20.0 * (self.env_fast.max(1e-9) / self.env.max(1e-9)).log10();
        let boost_db = (onset_db * PUNCH_SLOPE).clamp(0.0, PUNCH_MAX_DB);

        let db = amount * (MAKEUP_DB + boost_db - reduction_db);
        10.0f64.powf(db / 20.0) as Sample
    }

    /// 現在のゲインリダクション量 [dB] (検証用、amount = 1 相当)。
    pub fn reduction_db(&self) -> f64 {
        let env_db = 20.0 * self.env.max(1e-9).log10();
        let over = env_db - THRESHOLD_DB;
        let slope = 1.0 - 1.0 / RATIO;
        let half = KNEE_DB * 0.5;
        if over <= -half {
            0.0
        } else if over < half {
            let t = over + half;
            slope * t * t / (2.0 * KNEE_DB)
        } else {
            slope * over
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    #[test]
    fn below_threshold_passes_at_makeup_gain() {
        // しきい値より十分下 (−40 dBFS) の定常信号はリダクション 0、
        // ゲインはメイクアップぶんだけ。
        let mut c = Compressor::new(SR);
        let level = 10.0f32.powf(-40.0 / 20.0);
        let mut g = 1.0;
        for _ in 0..48_000 {
            g = c.gain(level, 1.0);
        }
        assert_relative_eq!(c.reduction_db(), 0.0, epsilon = 1e-9);
        assert_relative_eq!(20.0 * (g as f64).log10(), MAKEUP_DB, epsilon = 1e-3);
    }

    #[test]
    fn amount_zero_is_exact_unity() {
        // amount = 0 は厳密に素通し (校正の不変条件、D-029)。
        let mut c = Compressor::new(SR);
        for _ in 0..10_000 {
            assert_eq!(c.gain(0.8, 0.0), 1.0);
        }
    }

    #[test]
    fn the_static_curve_follows_the_ratio() {
        // しきい値 +12 dB の定常信号 → 超過はニーの外なので
        // リダクション = slope × over = (1 − 1/3) × 12 = 8 dB。
        let mut c = Compressor::new(SR);
        let level = 10.0f32.powf(((THRESHOLD_DB + 12.0) / 20.0) as f32);
        for _ in 0..96_000 {
            c.gain(level, 1.0);
        }
        assert_relative_eq!(c.reduction_db(), 8.0, epsilon = 0.05);
    }

    #[test]
    fn the_attack_lets_the_transient_through() {
        // 無音 → しきい値 +12 dB へのステップ。アタック 40 ms なので、
        // 打撃ピークの領域 (最初の 5 ms) ではリダクションがまだ浅く
        // (< 1.5 dB)、200 ms 後にはほぼ静的値 (8 dB) に達する。
        let mut c = Compressor::new(SR);
        let level = 10.0f32.powf(((THRESHOLD_DB + 12.0) / 20.0) as f32);
        for _ in 0..(SR * 0.005) as usize {
            c.gain(level, 1.0);
        }
        assert!(
            c.reduction_db() < 1.5,
            "アタックが速すぎる (打撃ピークが捕まる): 5 ms で {:.2} dB",
            c.reduction_db()
        );
        for _ in 0..(SR * 0.195) as usize {
            c.gain(level, 1.0);
        }
        assert!(
            c.reduction_db() > 7.0,
            "アタックが遅すぎる: 200 ms で {:.2} dB",
            c.reduction_db()
        );
    }

    #[test]
    fn the_punch_boosts_only_the_onset() {
        // 定常のウォッシュ → +12 dB のステップ (打撃)。直後は速い包絡が
        // 先行してブーストが立ち、定常に達するとブーストは消えて
        // リダクションが上回る。
        let mut c = Compressor::new(SR);
        let wash = 10.0f32.powf((THRESHOLD_DB / 20.0) as f32);
        for _ in 0..48_000 {
            c.gain(wash, 1.0);
        }
        let g_before = c.gain(wash, 1.0);

        let hit = wash * 4.0; // +12 dB
        let mut g_peak = 0.0f32;
        for _ in 0..(SR * 0.004) as usize {
            g_peak = g_peak.max(c.gain(hit, 1.0));
        }
        let boost = 20.0 * (g_peak as f64 / g_before as f64).log10();
        assert!(
            boost > 1.5,
            "オンセットでブーストが立っていない: {boost:.2} dB"
        );

        for _ in 0..(SR * 0.3) as usize {
            c.gain(hit, 1.0);
        }
        let g_steady = c.gain(hit, 1.0);
        assert!(
            g_steady < g_before,
            "定常でブーストが残っている: {g_steady} vs {g_before}"
        );
    }

    #[test]
    fn the_release_recovers_between_roll_strikes() {
        // 定常リダクション後に無音へ。リリース 180 ms なので、ロールの
        // 打撃間隔 (8 Hz = 125 ms) で半分前後は回復している。
        let mut c = Compressor::new(SR);
        let level = 10.0f32.powf(((THRESHOLD_DB + 12.0) / 20.0) as f32);
        for _ in 0..96_000 {
            c.gain(level, 1.0);
        }
        let full = c.reduction_db();
        for _ in 0..(SR * 0.125) as usize {
            c.gain(0.0, 1.0);
        }
        let after = c.reduction_db();
        assert!(
            after < full * 0.8 && after > 0.0,
            "リリースの速さがロールに合っていない: {full:.2} → {after:.2} dB"
        );
    }
}
