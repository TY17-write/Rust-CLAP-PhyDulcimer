//! モーダル共振器バンク。
//!
//! 弦の変位を固有モードの重ね合わせで表す:
//!
//! ```text
//! y(x, t) = Σ_n a_n(t) · sin(nπx/L)
//! ```
//!
//! 各モード `a_n` は独立した2階の減衰振動系に従う:
//!
//! ```text
//! ä_n + 2σ_n·ȧ_n + ω_n²·a_n = F(t)·φ_n / M_n
//! ```
//!
//! `φ_n = sin(nπ·x_h/L)` は打弦点でのモード形状、`M_n = μL/2` はモード質量。
//!
//! # 離散化に結合形 (coupled form) を使う理由
//!
//! 2次共振器の実装には主に2つの形式がある。
//!
//! **直接形2極**: `y[k] = a1·y[k-1] + a2·y[k-2] + b·x[k]`、`a1 = 2r·cosθ`。
//! 乗算2回で最安だが、低周波で破綻する。最低音の G2 (98 Hz) を 48 kHz で回すと
//! `θ = 0.0128 rad` なので `a1 = 2r·cosθ ≈ 1.99984` となり、**周波数の情報が
//! 仮数の下位ビットだけに乗る**。f32 では音高が可聴なレベルでずれる。
//!
//! ピアノの A0 (27.5 Hz) ほど極端ではないが、この楽器は**ダンパーが無く
//! 数十秒鳴り続ける**。わずかな音高のずれが、共鳴している他の弦とのうなりとして
//! 時間をかけて露呈する。低音側の余裕は残しておく。
//!
//! **結合形 (減衰つき回転)**: 状態を複素数 `s = re + j·im` とし、
//! `s[k+1] = r·e^{jθ}·s[k] + g·u[k]` を実部・虚部で回す。
//!
//! ```text
//! re' = r·cosθ·re − r·sinθ·im + g·u
//! im' = r·sinθ·re + r·cosθ·im
//! ```
//!
//! 乗算4回と1回多いが、`r·sinθ` に周波数の情報がそのまま入るため
//! **低周波でも f32 の相対精度がまるごと効く**。極半径も `|p| = r` として
//! 厳密に保たれる。本プロジェクトは A0 = 27.5 Hz を扱い、かつ Phase 7 で
//! f32 SIMD 化する前提なので、乗算1回分は精度に払う。
//!
//! # 入出力ゲイン
//!
//! インパルス不変変換で離散化する。連続系のインパルス応答は
//! `h(t) = e^{-σt}·sin(ω_d·t)/ω_d` なので、実部に `g = dt/ω_d` で注入して
//! **虚部を出力**すると `h_d[k] = dt·h((k−1)·dt)` になる (1サンプルの遅延を伴う)。
//!
//! # SIMD
//!
//! **実績のある SIMD 化済みの実装をそのまま移植した。**
//! 計画では「Phase 1 はスカラ、Phase 8 で SIMD」としていたが、動作実績のある
//! 実装をわざわざスカラに退化させる意味がない (書き換えでバグが入る余地と、
//! Phase 8 での二度手間が増えるだけ)。テストも一緒に移植してある。
//! → `docs/problems.md` の D-008
//!
//! **全モードが独立した再帰**なので、モード方向にそのまま並列化できる。これが
//! 導波管ではなくモーダル合成を選んだ理由のひとつでもある (導波管の遅延線は
//! 逐次依存で、こうはいかない)。
//!
//! 係数と状態を [`f32x8`] の配列で持ち、8 モードずつまとめて回す。使わない
//! レーンは**係数も状態も 0 に均して**あるので、端数の処理に分岐が要らない
//! (0 を掛けて 0 を足すだけ)。水平加算は 1 サンプルにつき最後の 1 回だけ。
//!
//! なお `wide` の `f32x8` は AVX が無い環境では SSE レジスタ 2 本に落ちる。
//! **ビルドは baseline (SSE2) のままにしてある**ので、`target-cpu` を指定しない
//! 配布バイナリでも動く。AVX が使える環境なら自動的に 8 レーンで回る。

use crate::Sample;
use wide::f32x8;

/// 1区間の弦が持てるモード数の上限。
///
/// オーディオスレッドでの確保を避けるため、全バッファをこの長さで固定確保し、
/// 実際に使う本数 (`active`) だけを変える。**ホットループの回数は `active` で
/// 決まるので、この値を大きくしてもコストは増えない** (増えるのはメモリだけで、
/// 1 バンクあたり約 22 KB)。
///
/// # なぜ 256 と大きめに取ってあるか
///
/// この楽器は**打弦点がブリッジに寄る** (`x/L = 0.05` まで)。励振重みは
/// `sin(nπ·x_h/L)` なので、最初のノッチは第 `L/x_h` = 第 20 部分音。
/// そこまでは単調に強く励振され、**モードの打ち切りが直接アタックを殺す**。
///
/// 128 で足りるかは測らないと分からない。**測れるようにするために枠を広げてある** —
/// 実際に使う本数は Phase 1 の実測で決める。
pub const MAX_MODES: usize = 256;

/// SIMD のレーン数。
pub const LANES: usize = 8;

/// デノーマル防止の微小 DC [N]。入力へ常時足す。
///
/// # なぜ要るか (D-019)
///
/// この楽器は弦を回収しない (ダンパーが無く、常時走行)。減衰の尻尾は
/// f32 のデノーマル域 (< 1.2e-38) を**何十秒もかけて通過し続け**、
/// x86 のデノーマル演算ペナルティで処理が 1 桁遅くなる。ボイス回収のある
/// シンセなら回収が先に効いて崖に到達しないが、回収の無いこの楽器では
/// 実際に踏んだ (テストスイートが 30 秒 → 10 分超)。
///
/// 微小 DC を入力へ足すと、状態は 0 ではなく正規化数の微小定常値
/// (このゲインで ~1e-16 台) に落ち着き、デノーマルに沈まない。
/// −300 dB なので聴感・測定への影響は無い。`forbid(unsafe_code)` を保ったまま
/// FTZ/DAZ と同じ効果が得られる。
const ANTI_DENORMAL: Sample = 1.0e-15;

/// [`MAX_MODES`] を収めるのに要る [`f32x8`] の本数。
const CHUNKS: usize = MAX_MODES / LANES;

/// `n` モードを収めるのに要るチャンク数 (端数は切り上げ)。
#[inline]
const fn chunks_for(n: usize) -> usize {
    n.div_ceil(LANES)
}

/// ベクタの 1 レーンだけを書き換える。
///
/// `as_array_mut` はベクタをそのままメモリとして触るので、書き込み 1 回で済む。
/// **`to_array()` で取り出して書き戻す実装にしたら `note_on` が 2 倍重くなった**
/// (ベクタ全体の読み書きが 1 レーンごとに発生するため)。係数の設定は打鍵時に
/// 最大 128 モード × 2 レート走るので、ここが効く。
#[inline]
fn set_lane(v: &mut f32x8, lane: usize, value: Sample) {
    v.as_array_mut()[lane] = value;
}

/// ベクタの 1 レーンを読む。
#[inline]
fn lane(v: &f32x8, lane: usize) -> Sample {
    v.as_array_ref()[lane]
}

/// 係数を再計算するときのレート指定。
///
/// ハンマー接触中だけオーバーサンプルするため、2組の係数を持ち分ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    /// ホストのサンプリング周波数
    Base = 0,
    /// ハンマー接触中に使うオーバーサンプルレート
    Oversampled = 1,
}

/// レート依存の係数一式。
#[derive(Debug, Clone)]
struct RateCoeffs {
    /// `r·cos θ`
    r_cos: [f32x8; CHUNKS],
    /// `r·sin θ`
    r_sin: [f32x8; CHUNKS],
    /// 力 [N] → 状態実部への注入ゲイン (`dt/ω_d · φ_n / M_n`)
    in_gain: [f32x8; CHUNKS],
    /// ブリッジ駆動 → 状態実部への注入ゲイン (`dt/ω_d · w_n / M_n`)
    ///
    /// ブリッジ点は弦の端 (変位モードの節) なので、点の力は `φ_n` では
    /// 入らない。端点が動くときの結合は**モード形状の傾き** `φ'_n(0) ∝ n` で
    /// 決まり、これは出力重み `w_n = T·nπ/L` と同じ形。つまりブリッジ駆動は
    /// **出力の転置** (ランク1 の結合、`docs/plan.html` §03)。
    bridge_gain: [f32x8; CHUNKS],
}

impl Default for RateCoeffs {
    fn default() -> Self {
        let zero = f32x8::splat(0.0);
        Self {
            r_cos: [zero; CHUNKS],
            r_sin: [zero; CHUNKS],
            in_gain: [zero; CHUNKS],
            bridge_gain: [zero; CHUNKS],
        }
    }
}

/// 1モード分の設計値。[`ModalBank::set_mode`] に渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeSpec {
    /// 部分音の周波数 [Hz]
    pub freq_hz: f64,
    /// 減衰係数 σ [1/s]。振幅が `e^{-σt}` で減る
    pub decay: f64,
    /// 打弦点でのモード形状 `φ_n = sin(nπ·x_h/L)`
    pub strike_weight: f64,
    /// モード質量 `M_n = μL/2` [kg]
    pub modal_mass: f64,
    /// 出力の重み (ブリッジ力なら `T·nπ/L`)
    pub out_weight: f64,
    /// ブリッジ駆動を受ける重み。
    ///
    /// 素の物理は `out_weight` と同じ (ランク1 の転置) だが、**高域は
    /// 落とすこと**。1 サンプル遅延の帰還は高周波モードで位相が回りきり、
    /// モードの固有減衰 (σ·dt) を超えるループゲインが乗ると発散する。
    /// 物理的にもブリッジの質量で高域の結合は落ちる。
    pub bridge_weight: f64,
}

/// 並列2次共振器バンク。
///
/// 構築時に [`MAX_MODES`] 分を確保し、以降は確保しない。`process_*` は
/// オーディオスレッドから呼べる。
#[derive(Debug, Clone)]
pub struct ModalBank {
    /// 状態の実部
    re: [f32x8; CHUNKS],
    /// 状態の虚部。これがモード変位 `a_n` にあたる
    im: [f32x8; CHUNKS],
    /// 打弦点でのモード形状 `φ_n`
    strike_weight: [f32x8; CHUNKS],
    /// 出力の重み
    out_weight: [f32x8; CHUNKS],
    /// レート別の係数 (`Rate` で添字)
    coeffs: [RateCoeffs; 2],
    /// 実際に使うモード数
    active: usize,
    /// `active` を収めるチャンク数。ホットループの回数
    chunks: usize,
    /// モードごとの追加減衰 (1 サンプルあたり)。1.0 で減衰なし
    ///
    /// **この楽器にダンパー機構は無い** ので、常用するのは 1.0 (減衰なし)。
    /// 使うのは奏者が手のひらで弦を押さえるミュート奏法 (Phase 7) で、
    /// 手はフェルトと同じく高い部分音を先に止めるため、一律ではなく
    /// モードごとに持つ。
    damping: [f32x8; CHUNKS],
}

impl Default for ModalBank {
    fn default() -> Self {
        Self::new()
    }
}

impl ModalBank {
    pub fn new() -> Self {
        let zero = f32x8::splat(0.0);
        Self {
            re: [zero; CHUNKS],
            im: [zero; CHUNKS],
            strike_weight: [zero; CHUNKS],
            out_weight: [zero; CHUNKS],
            coeffs: [RateCoeffs::default(), RateCoeffs::default()],
            active: 0,
            chunks: 0,
            damping: [f32x8::splat(1.0); CHUNKS],
        }
    }

    /// 全モードに一律の追加減衰を設定する。`1.0` で減衰なし。
    ///
    /// 極半径を `r·factor` にするのと等価。ダンパーを離すとき (`1.0`) に使う。
    #[inline]
    pub fn set_damping_all(&mut self, factor: Sample) {
        self.damping.fill(f32x8::splat(factor.clamp(0.0, 1.0)));
    }

    /// モードごとの追加減衰を設定する。
    ///
    /// 実機のダンパーは高い部分音を先に止めるので、モードごとに違う値を入れる。
    /// `index` が範囲外なら何もしない。
    #[inline]
    pub fn set_mode_damping(&mut self, index: usize, factor: Sample) {
        if index < MAX_MODES {
            set_lane(
                &mut self.damping[index / LANES],
                index % LANES,
                factor.clamp(0.0, 1.0),
            );
        }
    }

    /// モード `index` の追加減衰。
    #[inline]
    pub fn mode_damping(&self, index: usize) -> Sample {
        if index < MAX_MODES {
            lane(&self.damping[index / LANES], index % LANES)
        } else {
            1.0
        }
    }

    /// 使用中のモード数。
    #[inline]
    pub fn active_modes(&self) -> usize {
        self.active
    }

    /// 使用するモード数を設定する。[`MAX_MODES`] で頭打ちになる。
    ///
    /// **端数レーンを均す。** 最後のチャンクの余りに前の音の係数や状態が
    /// 残っていると、0 を掛けたつもりが鳴ってしまう。ここで消しておけば
    /// ホットループは端数を気にせずチャンク単位で回せる。
    pub fn set_active_modes(&mut self, n: usize) {
        self.active = n.min(MAX_MODES);
        self.chunks = chunks_for(self.active);

        for i in self.active..self.chunks * LANES {
            let (c, l) = (i / LANES, i % LANES);
            set_lane(&mut self.re[c], l, 0.0);
            set_lane(&mut self.im[c], l, 0.0);
            set_lane(&mut self.strike_weight[c], l, 0.0);
            set_lane(&mut self.out_weight[c], l, 0.0);
            set_lane(&mut self.damping[c], l, 0.0);
            for rate in 0..2 {
                set_lane(&mut self.coeffs[rate].r_cos[c], l, 0.0);
                set_lane(&mut self.coeffs[rate].r_sin[c], l, 0.0);
                set_lane(&mut self.coeffs[rate].in_gain[c], l, 0.0);
                set_lane(&mut self.coeffs[rate].bridge_gain[c], l, 0.0);
            }
        }
    }

    /// 全モードの状態を 0 に戻す。係数は保持する。
    pub fn reset(&mut self) {
        let zero = f32x8::splat(0.0);
        self.re.fill(zero);
        self.im.fill(zero);
    }

    /// 1モード分の係数を設定する。
    ///
    /// `index` が [`MAX_MODES`] 以上なら何もしない (オーディオスレッドで
    /// panic させないため)。`dt` は指定レートでの1サンプルの時間 [s]。
    pub fn set_mode(&mut self, index: usize, rate: Rate, dt: f64, spec: &ModeSpec) {
        if index >= MAX_MODES {
            return;
        }

        let omega = std::f64::consts::TAU * spec.freq_hz;
        // 減衰つき固有角周波数。ピアノでは σ ≪ ω なので実質 ω だが、
        // 過減衰の指定が来ても NaN にしないよう下限を入れる。
        let omega_d = (omega * omega - spec.decay * spec.decay).max(1e-12).sqrt();

        let r = (-spec.decay * dt).exp();
        let theta = omega_d * dt;

        let (chunk, slot) = (index / LANES, index % LANES);
        let c = &mut self.coeffs[rate as usize];
        set_lane(&mut c.r_cos[chunk], slot, (r * theta.cos()) as Sample);
        set_lane(&mut c.r_sin[chunk], slot, (r * theta.sin()) as Sample);

        // インパルス不変変換のゲイン dt/ω_d に、力からモード加速度への
        // 変換 φ_n / M_n を掛ける。
        let gain = if spec.modal_mass > 0.0 {
            dt / omega_d * spec.strike_weight / spec.modal_mass
        } else {
            0.0
        };
        set_lane(&mut c.in_gain[chunk], slot, gain as Sample);

        // ブリッジ駆動: 重みは呼び出し側が高域を落とした bridge_weight。
        let bridge = if spec.modal_mass > 0.0 {
            dt / omega_d * spec.bridge_weight / spec.modal_mass
        } else {
            0.0
        };
        set_lane(&mut c.bridge_gain[chunk], slot, bridge as Sample);

        set_lane(
            &mut self.strike_weight[chunk],
            slot,
            spec.strike_weight as Sample,
        );
        set_lane(&mut self.out_weight[chunk], slot, spec.out_weight as Sample);
    }

    /// 1サンプル進めて出力 (重み付きモード和) を返す。
    ///
    /// `force` は打弦点に加わる力 [N]。
    ///
    /// 8 モードずつまとめて回す。端数レーンは係数も状態も 0 なので、そのまま
    /// 掛けて足しても結果に影響しない ([`Self::set_active_modes`] を参照)。
    /// 水平加算はループを抜けてから 1 回だけ。
    ///
    /// **このループに累算器を足さないこと。** `f32x8` は AVX が無いと SSE
    /// レジスタ 2 本を消費するので、baseline ビルドでは 16 本の xmm がすぐ尽きる。
    /// 別の量が要るなら別ループで回すほうが速い (→ `docs/problems.md` の D-009)。
    #[inline]
    pub fn process_sample(&mut self, force: Sample, rate: Rate) -> Sample {
        self.process_sample_bridged(force, 0.0, rate)
    }

    /// [`Self::process_sample`] に加えて、ブリッジ駆動 `bridge_drive` を
    /// `w_n` の重みで注入する (出力の転置、Phase 4 のブリッジ結合)。
    ///
    /// `bridge_drive` はブリッジ点の動きに相当する量。結合の強さと符号は
    /// 呼び出し側 ([`course`](crate::course)) が決める。
    #[inline]
    pub fn process_sample_bridged(
        &mut self,
        force: Sample,
        bridge_drive: Sample,
        rate: Rate,
    ) -> Sample {
        let c = &self.coeffs[rate as usize];
        // デノーマル防止の DC を両方の入力路へ (上記 ANTI_DENORMAL 参照)。
        // strike_weight が節で 0 のモードにも bridge_weight 側から届く。
        let drive = f32x8::splat(force + ANTI_DENORMAL);
        let bridge = f32x8::splat(bridge_drive + ANTI_DENORMAL);
        let mut out = f32x8::splat(0.0);

        for i in 0..self.chunks {
            let re = self.re[i];
            let im = self.im[i];
            let damp = self.damping[i];
            // 回転させてから追加減衰を掛ける。極半径を r·damp にするのと等価。
            let next_re = (c.r_cos[i] * re - c.r_sin[i] * im) * damp
                + c.in_gain[i] * drive
                + c.bridge_gain[i] * bridge;
            let next_im = (c.r_sin[i] * re + c.r_cos[i] * im) * damp;
            self.re[i] = next_re;
            self.im[i] = next_im;
            out += self.out_weight[i] * next_im;
        }

        out.reduce_add()
    }

    /// 打弦点での弦の変位 `y(x_h) = Σ a_n·φ_n` [m]。
    ///
    /// ハンマーとの結合に使う。状態を進めずに現在値を読むだけ。
    #[inline]
    pub fn displacement_at_strike(&self) -> Sample {
        let mut y = f32x8::splat(0.0);
        for i in 0..self.chunks {
            y += self.im[i] * self.strike_weight[i];
        }
        y.reduce_add()
    }

    /// モード `index` の現在の変位 `a_n` [m]。検証用。
    pub fn mode_displacement(&self, index: usize) -> Sample {
        if index < MAX_MODES {
            lane(&self.im[index / LANES], index % LANES)
        } else {
            0.0
        }
    }

    /// 全モードの状態が有限か。数値的な破綻を検知する。
    pub fn is_finite(&self) -> bool {
        let finite = |v: &[f32x8]| {
            v[..self.chunks]
                .iter()
                .all(|c| c.to_array().iter().all(|x| x.is_finite()))
        };
        finite(&self.re) && finite(&self.im)
    }
}

/// 減衰係数 σ を `-60 dB` 到達時間 (T60) から求める。
///
/// 振幅は `e^{-σt}` で減るので `T60 = ln(1000)/σ`。
#[inline]
pub fn decay_from_t60(t60_sec: f64) -> f64 {
    if t60_sec > 0.0 {
        3.0 * std::f64::consts::LN_10 / t60_sec
    } else {
        f64::INFINITY
    }
}

/// 減衰係数 σ から T60 [s] を求める。[`decay_from_t60`] の逆。
#[inline]
pub fn t60_from_decay(decay: f64) -> f64 {
    if decay > 0.0 {
        3.0 * std::f64::consts::LN_10 / decay
    } else {
        f64::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    fn single_mode(freq_hz: f64, t60_sec: f64) -> ModalBank {
        let mut bank = ModalBank::new();
        bank.set_active_modes(1);
        bank.set_mode(
            0,
            Rate::Base,
            1.0 / SR,
            &ModeSpec {
                freq_hz,
                decay: decay_from_t60(t60_sec),
                strike_weight: 1.0,
                modal_mass: 1.0,
                out_weight: 1.0,
                bridge_weight: 0.0,
            },
        );
        bank
    }

    /// インパルスを1発入れて `n` サンプル走らせる。
    fn impulse_response(bank: &mut ModalBank, n: usize) -> Vec<Sample> {
        let mut out = Vec::with_capacity(n);
        out.push(bank.process_sample(1.0, Rate::Base));
        for _ in 1..n {
            out.push(bank.process_sample(0.0, Rate::Base));
        }
        out
    }

    /// 指定周波数の振幅 (Goertzel 法)。`tools/analyze` と同じ原理の簡易版。
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
    fn pole_radius_and_angle_are_exact() {
        let freq = 440.0;
        let t60 = 2.0;
        let bank = single_mode(freq, t60);
        let c = &bank.coeffs[Rate::Base as usize];

        let r = (lane(&c.r_cos[0], 0) as f64).hypot(lane(&c.r_sin[0], 0) as f64);
        let theta = (lane(&c.r_sin[0], 0) as f64).atan2(lane(&c.r_cos[0], 0) as f64);

        let expected_r = (-decay_from_t60(t60) / SR).exp();
        let expected_theta = std::f64::consts::TAU * freq / SR;

        assert_relative_eq!(r, expected_r, max_relative = 1e-6);
        assert_relative_eq!(theta, expected_theta, max_relative = 1e-5);
    }

    #[test]
    fn low_frequency_pitch_survives_f32() {
        // 結合形を選んだ理由そのもののテスト。この楽器の最低音 G2 = 98 Hz を
        // f32 係数で回して音高が保たれることを確認する。直接形2極ならここで
        // 有意にずれる。ダンパーが無く長く鳴る楽器なので、わずかなずれも
        // 他の弦とのうなりとして露呈する。
        let freq = 98.0;
        let mut bank = single_mode(freq, 20.0);
        let x = impulse_response(&mut bank, 96_000); // 2 秒

        // ±0.5 Hz ずらした点より、狙いの周波数で最大になること。
        let on = magnitude_at(&x, freq);
        assert!(on > magnitude_at(&x, freq - 0.5));
        assert!(on > magnitude_at(&x, freq + 0.5));

        // 係数から復元した周波数が 1 cent 以内。
        let c = &bank.coeffs[Rate::Base as usize];
        let theta = (lane(&c.r_sin[0], 0) as f64).atan2(lane(&c.r_cos[0], 0) as f64);
        let recovered = theta * SR / std::f64::consts::TAU;
        let cents = 1200.0 * (recovered / freq).log2();
        assert!(cents.abs() < 1.0, "音高のずれ {cents} cent");
    }

    #[test]
    fn impulse_response_has_the_requested_frequency() {
        for freq in [55.0, 220.0, 440.0, 3_000.0] {
            let mut bank = single_mode(freq, 5.0);
            let x = impulse_response(&mut bank, 48_000);

            let on = magnitude_at(&x, freq);
            // 半音上下より狙いの周波数が強いこと。
            let up = magnitude_at(&x, freq * 1.0595);
            let down = magnitude_at(&x, freq / 1.0595);
            assert!(on > up * 10.0, "freq={freq}: on={on}, up={up}");
            assert!(on > down * 10.0, "freq={freq}: on={on}, down={down}");
        }
    }

    #[test]
    fn decay_matches_the_requested_t60() {
        let t60 = 1.0;
        let mut bank = single_mode(1_000.0, t60);
        let n = (SR * t60) as usize;
        let x = impulse_response(&mut bank, n * 2);

        // 前半 (0 → t60) と後半 (t60 → 2·t60) のピーク比が 1/1000 になるはず。
        let peak_first = x[..n].iter().fold(0.0f32, |a, &b| a.max(b.abs())) as f64;
        let peak_second = x[n..].iter().fold(0.0f32, |a, &b| a.max(b.abs())) as f64;
        assert_relative_eq!(peak_second / peak_first, 1e-3, max_relative = 0.02);
    }

    #[test]
    fn t60_conversion_round_trips() {
        for t60 in [0.1, 1.0, 12.0, 60.0] {
            assert_relative_eq!(t60_from_decay(decay_from_t60(t60)), t60, epsilon = 1e-9);
        }
        assert!(decay_from_t60(0.0).is_infinite());
        assert!(t60_from_decay(0.0).is_infinite());
    }

    #[test]
    fn oversampled_coefficients_give_the_same_decay_envelope() {
        // 4x のレートで 4 倍のステップを踏めば、同じ実時間で同じだけ減衰する。
        let t60 = 0.5;
        let freq = 440.0;
        let os = 4;

        let mut a = single_mode(freq, t60);
        let mut b = single_mode(freq, t60);
        b.set_mode(
            0,
            Rate::Oversampled,
            1.0 / (SR * os as f64),
            &ModeSpec {
                freq_hz: freq,
                decay: decay_from_t60(t60),
                strike_weight: 1.0,
                modal_mass: 1.0,
                out_weight: 1.0,
                bridge_weight: 0.0,
            },
        );

        // 同じ実時間 (0.25 秒) を、片方は Base、片方は 4x で回す。
        let steps = (SR * 0.25) as usize;
        a.process_sample(1.0, Rate::Base);
        for _ in 1..steps {
            a.process_sample(0.0, Rate::Base);
        }
        b.process_sample(1.0, Rate::Oversampled);
        for _ in 1..steps * os {
            b.process_sample(0.0, Rate::Oversampled);
        }

        // 包絡 (状態ベクトルの長さ) が一致すること。位相は一致しなくてよい。
        let env_a = (lane(&a.re[0], 0) as f64).hypot(lane(&a.im[0], 0) as f64);
        let env_b = (lane(&b.re[0], 0) as f64).hypot(lane(&b.im[0], 0) as f64);
        // 4x では in_gain が 1/4 になるので、注入量も 1/4 になっている。
        assert_relative_eq!(env_b * os as f64, env_a, max_relative = 0.01);
    }

    #[test]
    fn reset_clears_the_state_but_keeps_coefficients() {
        let mut bank = single_mode(440.0, 2.0);
        impulse_response(&mut bank, 1_000);
        assert!(bank.mode_displacement(0).abs() > 0.0);

        bank.reset();
        assert_eq!(bank.mode_displacement(0), 0.0);
        assert_eq!(bank.displacement_at_strike(), 0.0);

        // 係数は残っているので、また鳴らせる。
        let x = impulse_response(&mut bank, 1_000);
        assert!(x.iter().any(|&v| v.abs() > 0.0));
    }

    #[test]
    fn out_of_range_mode_index_is_ignored() {
        let mut bank = ModalBank::new();
        // panic せず、何も起きない。
        bank.set_mode(
            MAX_MODES,
            Rate::Base,
            1.0 / SR,
            &ModeSpec {
                freq_hz: 440.0,
                decay: 1.0,
                strike_weight: 1.0,
                modal_mass: 1.0,
                out_weight: 1.0,
                bridge_weight: 0.0,
            },
        );
        assert_eq!(bank.mode_displacement(MAX_MODES), 0.0);
    }

    #[test]
    fn active_modes_are_capped() {
        let mut bank = ModalBank::new();
        bank.set_active_modes(MAX_MODES * 10);
        assert_eq!(bank.active_modes(), MAX_MODES);
    }

    #[test]
    fn multiple_modes_sum_independently() {
        let mut bank = ModalBank::new();
        bank.set_active_modes(3);
        let freqs = [200.0, 401.0, 613.0];
        for (i, &f) in freqs.iter().enumerate() {
            bank.set_mode(
                i,
                Rate::Base,
                1.0 / SR,
                &ModeSpec {
                    freq_hz: f,
                    decay: decay_from_t60(4.0),
                    strike_weight: 1.0,
                    modal_mass: 1.0,
                    out_weight: 1.0,
                    bridge_weight: 0.0,
                },
            );
        }

        let x = impulse_response(&mut bank, 48_000);
        assert!(bank.is_finite());
        for &f in &freqs {
            // どの成分も存在し、間の周波数にはエネルギーがない。
            assert!(magnitude_at(&x, f) > magnitude_at(&x, f + 40.0) * 5.0);
        }
    }

    #[test]
    fn state_stays_finite_under_sustained_drive() {
        let mut bank = single_mode(440.0, 30.0);
        // 共振周波数で連続的に加振し続けても発散しない (r < 1 のため)。
        for k in 0..480_000 {
            let phase = std::f64::consts::TAU * 440.0 * k as f64 / SR;
            bank.process_sample(phase.sin() as Sample, Rate::Base);
        }
        assert!(bank.is_finite());
    }
}
