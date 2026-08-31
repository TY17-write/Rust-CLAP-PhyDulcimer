//! 弦の 1 区間。
//!
//! ハンマーダルシマーの弦は**ブリッジをまたいで 2 つの発音区間を持つ**が、
//! Phase 1 で扱うのは 1 区間だけ。区間どうしの結合は Phase 4 で入れる。
//!
//! # 弦の設計則
//!
//! 発弦長 `L` と目標周波数 `f0` と線径 `d` から、残りを導く。実際の楽器製作と
//! 同じ順序なので、内部矛盾が起きない。
//!
//! ```text
//! μ = ρ·πd²/4                    線密度 [kg/m]
//! T = 4·μ·L²·f0²                 張力 [N]
//! B = π³·E·d⁴ / (64·L²·T)        インハーモニシティ係数
//! f_n = n·f0·√(1 + B·n²)         第 n 部分音 [Hz]
//! M_n = μL/2                     モード質量 [kg]
//! ```
//!
//! # 打弦点
//!
//! **この楽器では奏者が打弦点を選ぶ。** 励振の重みは `φ_n = sin(nπ·x_h/L)` で、
//! `x_h/L = 1/8` なら第 8・16・24… 部分音が節に当たって励振されない。
//!
//! 実機の奏者はブリッジから 25–50 mm を叩く。区間長 330–495 mm に対して
//! `x/L = 0.05–0.15` で、**ピアノの 1/8 (0.125) よりブリッジ寄りまで動く**。
//! 比が小さいほど最初のノッチが高次に移り、`1/0.05 = 第 20 部分音`まで
//! 単調に強く励振される。
//!
//! # 出力
//!
//! ブリッジに加わる横向きの力 [N]:
//!
//! ```text
//! F_bridge = T·∂y/∂x|_{x=0} = T·Σ a_n·(nπ/L)
//! ```
//!
//! `nπ/L` の重みがあるので**高次ほどブリッジをよく駆動する**。これは
//! Peterson が観察した「接触中は打弦点とブリッジに挟まれた短い区間が
//! 基音よりはるかに高い周波数でブリッジを叩く」と同じ向きの効果。

use crate::hammer::{Hammer, HammerParams};
use crate::modal::{decay_from_t60, ModalBank, ModeSpec, Rate, MAX_MODES};
use crate::Sample;

/// 鋼 (music wire) の密度 [kg/m³]。
pub const STEEL_DENSITY: f64 = 7_850.0;
/// 鋼のヤング率 [Pa]。
pub const STEEL_YOUNG: f64 = 2.0e11;

/// 1 区間の弦の設計値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentParams {
    /// 発弦長 [m]
    pub length_m: f64,
    /// 基音 [Hz]
    pub f0_hz: f64,
    /// 線径 [m]
    pub diameter_m: f64,
    /// 材質の密度 [kg/m³]
    pub density: f64,
    /// 材質のヤング率 [Pa]
    pub young: f64,
}

impl SegmentParams {
    /// トレブル最低コースの長い側 (D4)。
    ///
    /// Peterson の実測 (全長 826 mm が 330 : 495 に分割される) の長い側。
    /// **線径は設計則から選んだ暫定値**で、特定の個体の実測ではない。
    /// 音域ごとの設計表は Phase 3 で作る。
    pub fn treble_low_long() -> Self {
        Self {
            length_m: 0.495,
            f0_hz: 293.66,
            diameter_m: 0.5e-3,
            density: STEEL_DENSITY,
            young: STEEL_YOUNG,
        }
    }

    /// トレブル最低コースの短い側 (A4)。長い側のちょうど 5 度上。
    pub fn treble_low_short() -> Self {
        Self {
            length_m: 0.330,
            f0_hz: 440.0,
            ..Self::treble_low_long()
        }
    }

    /// 断面積 [m²]。
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.diameter_m * self.diameter_m * 0.25
    }

    /// 線密度 μ [kg/m]。
    pub fn linear_density(&self) -> f64 {
        self.density * self.area()
    }

    /// 張力 T [N]。`f0 = (1/2L)·√(T/μ)` を T について解いたもの。
    pub fn tension(&self) -> f64 {
        4.0 * self.linear_density() * self.length_m * self.length_m * self.f0_hz * self.f0_hz
    }

    /// 引張応力 [Pa]。**設計の妥当性はここで見る。**
    ///
    /// music wire の破断強度は 2000 MPa 前後で、実用は 30–50%。
    /// これを外れた線径は物理的にありえない。
    pub fn stress_pa(&self) -> f64 {
        let a = self.area();
        if a > 0.0 {
            self.tension() / a
        } else {
            0.0
        }
    }

    /// 横波の速度 [m/s] (= `2·L·f0`)。
    pub fn wave_speed(&self) -> f64 {
        2.0 * self.length_m * self.f0_hz
    }

    /// インハーモニシティ係数 `B = π³·E·d⁴ / (64·L²·T)`。
    pub fn inharmonicity(&self) -> f64 {
        let t = self.tension();
        if t <= 0.0 || self.length_m <= 0.0 {
            return 0.0;
        }
        std::f64::consts::PI.powi(3) * self.young * self.diameter_m.powi(4)
            / (64.0 * self.length_m * self.length_m * t)
    }

    /// モード質量 `M_n = μL/2` [kg]。すべてのモードで同じ。
    pub fn modal_mass(&self) -> f64 {
        self.linear_density() * self.length_m * 0.5
    }

    /// 第 `n` 部分音 [Hz] (n は 1 始まり)。
    pub fn partial_hz(&self, n: usize) -> f64 {
        let nf = n as f64;
        let b = self.inharmonicity();
        nf * self.f0_hz * (1.0 + b * nf * nf).sqrt()
    }

    /// ナイキストに収まる部分音の本数。[`MAX_MODES`] で頭打ちになる。
    pub fn mode_count(&self, sample_rate: f64) -> usize {
        // 0.98 は折り返し際の余裕。ここに部分音を置くと、わずかな係数誤差でも
        // ナイキストを跨いで折り返す。
        let limit = sample_rate * 0.5 * 0.98;
        (1..=MAX_MODES)
            .take_while(|&n| self.partial_hz(n) < limit)
            .count()
    }
}

/// 周波数依存の減衰設計 `σ(f) = c1 + c3·f²`。
///
/// `c1` は空気抵抗 (低域が支配的)、`c3` は内部粘弾性とヒステリシス (高域が支配的)。
/// Chaigne & Askenfelt 系の定式化に対応する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DampingParams {
    /// 周波数によらない項 [1/s]
    pub c1: f64,
    /// `f²` に比例する項 [1/(s·Hz²)]
    pub c3: f64,
}

impl DampingParams {
    /// 2 点の T60 から係数を解く。
    ///
    /// 減衰を「係数」ではなく**「この周波数で何秒鳴るか」**で指定できるようにする。
    /// PhyPiano は響板で Q 指定から減衰時間指定へ変えて改善した経緯があるので、
    /// こちらは最初から時間で持つ。
    pub fn from_t60_anchors(f_low: f64, t60_low: f64, f_high: f64, t60_high: f64) -> Self {
        let (s_low, s_high) = (decay_from_t60(t60_low), decay_from_t60(t60_high));
        let (fl2, fh2) = (f_low * f_low, f_high * f_high);
        if (fh2 - fl2).abs() < f64::EPSILON {
            return Self { c1: s_low, c3: 0.0 };
        }
        let c3 = (s_high - s_low) / (fh2 - fl2);
        Self {
            c1: s_low - c3 * fl2,
            c3,
        }
    }

    /// 周波数 `f` での減衰係数 σ [1/s]。
    pub fn decay_at(&self, f: f64) -> f64 {
        (self.c1 + self.c3 * f * f).max(0.0)
    }

    /// 周波数 `f` での T60 [s]。
    pub fn t60_at(&self, f: f64) -> f64 {
        crate::modal::t60_from_decay(self.decay_at(f))
    }
}

impl Default for DampingParams {
    /// ダンパーの無い楽器の暫定値。
    ///
    /// **実測ではない。** Peterson は「製作者が減衰を短くしようと苦心している」と
    /// 記しており、実機はかなり長く鳴る。ここでは基音 10 秒 / 5 kHz で 0.8 秒と
    /// 置いた。音域ごとの設計は Phase 3。
    fn default() -> Self {
        Self::from_t60_anchors(293.66, 10.0, 5_000.0, 0.8)
    }
}

/// オーバーサンプルしたレートから元のレートへ落とすときの方式。
///
/// **Phase 1 で測って決めた: 既定は [`Decimation::Drop`] で、フィルタは要らない。**
/// 木の撥 (os=16) で両方式の部分音レベルは 0.01 dB まで一致し、部分音の無い
/// 周波数のフロアはどちらも −129 dB 以下だった。共振器バンクの状態は狭帯域で、
/// ナイキストより上のエネルギーをそもそも持たないため。PhyPiano で未測のまま
/// 残った P-011 に、この楽器での答えを出した (→ D-010)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decimation {
    /// 最後のサブサンプルだけを採る (フィルタなし)
    #[default]
    Drop,
    /// サブサンプルの平均を採る (移動平均 1 段ぶんの緩いローパス)
    Average,
}

/// 1 区間の弦 + それを叩く撥。
#[derive(Debug, Clone)]
pub struct Segment {
    params: SegmentParams,
    damping: DampingParams,
    bank: ModalBank,
    hammer: Hammer,
    sample_rate: f64,
    /// 接触中のオーバーサンプル倍率
    oversample: usize,
    decimation: Decimation,
    /// 打弦点 `x_h / L`
    strike_ratio: f64,
    /// 使う部分音の本数の上限 (測定用に絞れる。0 で制限なし)
    mode_limit: usize,
    /// 実際に使っている本数
    modes: usize,
    /// 前サブサンプルの打弦点変位 [m]。速度を差分で作るために持つ
    prev_displacement: f64,
}

impl Segment {
    /// 既定は「木の撥・打弦点 0.09・オーバーサンプル 16 倍」。
    ///
    /// # 16 倍の根拠 (Phase 1 の実測、→ D-010)
    ///
    /// 部分音レベルが os=64 と比べて:
    ///
    /// | os | 木 | フェルト |
    /// |---|---|---|
    /// | 4 | +10 dB 以上ずれる | 数 dB 明るすぎる |
    /// | 8 | 高次で最大 20 dB ずれる | ≤ 1.8 dB |
    /// | **16** | **≤ 1.5 dB** | **≤ 0.5 dB** |
    ///
    /// 木の撥は接触がチャタリング (数十 µs の接触の繰り返し) になるため、
    /// 接触時間よりスペクトルの収束で判定した。
    pub fn new(params: SegmentParams, sample_rate: f64) -> Self {
        let mut s = Self {
            params,
            damping: DampingParams::default(),
            bank: ModalBank::new(),
            hammer: Hammer::new(HammerParams::wood()),
            sample_rate,
            oversample: 16,
            decimation: Decimation::default(),
            strike_ratio: 0.09,
            mode_limit: 0,
            modes: 0,
            prev_displacement: 0.0,
        };
        s.rebuild();
        s
    }

    pub fn params(&self) -> &SegmentParams {
        &self.params
    }

    pub fn damping(&self) -> &DampingParams {
        &self.damping
    }

    pub fn hammer(&self) -> &Hammer {
        &self.hammer
    }

    /// 使っている部分音の本数。
    pub fn partial_count(&self) -> usize {
        self.modes
    }

    pub fn strike_ratio(&self) -> f64 {
        self.strike_ratio
    }

    pub fn oversample(&self) -> usize {
        self.oversample
    }

    pub fn set_damping(&mut self, damping: DampingParams) {
        self.damping = damping;
        self.rebuild();
    }

    pub fn set_hammer_params(&mut self, params: HammerParams) {
        self.hammer.set_params(params);
        // 撥の幅が励振重みに入るので作り直す。
        self.rebuild();
    }

    /// 接触中のオーバーサンプル倍率を変える。1 以上。
    pub fn set_oversample(&mut self, factor: usize) {
        self.oversample = factor.max(1);
        self.rebuild();
    }

    pub fn set_decimation(&mut self, mode: Decimation) {
        self.decimation = mode;
    }

    /// 部分音の本数を上から絞る。`0` で制限なし (ナイキストまで)。
    ///
    /// **モードの打ち切りがアタックをどれだけ殺すかを測る**ために置いてある。
    ///
    /// Phase 1 の実測 (木・打弦点 0.05): 60 本で全数と 0.3 dB 以内、40 本で
    /// 最大 3.5 dB、20 本では**低次の部分音まで 1–4 dB 動く**。打ち切りは
    /// 高域を消すだけでなく、接触のチャタリングを介して低次にも波及する。
    /// **演奏用の経路では絞らないこと** (この区間は全数でも 75 本)。→ D-010
    pub fn set_mode_limit(&mut self, limit: usize) {
        self.mode_limit = limit;
        self.rebuild();
    }

    /// 打弦点 `x_h / L` を変える。
    ///
    /// 実機の奏者はブリッジから 25–50 mm を叩く。区間長で割ると 0.05–0.15。
    /// 端 (0 と 0.5) に寄りすぎると励振が消えるのでクランプする。
    pub fn set_strike_ratio(&mut self, ratio: f64) {
        self.strike_ratio = ratio.clamp(0.005, 0.5);
        self.rebuild();
    }

    /// 打弦。`velocity_mps` は弦に当たる直前の撥の速度 [m/s]。
    ///
    /// 実機の「drop and bounce」は落下高さ 6–12 inch で 1.7–2.4 m/s。
    ///
    /// **弦の状態はリセットしない。** ダンパーが無いので、鳴っている振動に
    /// 力を足すのが正しい (ロール奏法がこれで自然に出る)。
    ///
    /// 撥は**弦の現在位置**から出発させる。静止位置 0 から出発させると、
    /// 鳴っている弦を叩いたとき出発の瞬間に非物理的な圧縮スパイクが出て、
    /// ループ再生の再打弦で発散する (D-016)。
    pub fn strike(&mut self, velocity_mps: f64) {
        let displacement = self.bank.displacement_at_strike() as f64;
        self.hammer.strike_at(velocity_mps, displacement);
        self.prev_displacement = displacement;
    }

    /// 弦の状態を消す。テストと初期化のためのもので、演奏では使わない。
    pub fn reset(&mut self) {
        self.bank.reset();
        self.hammer.reset();
        self.prev_displacement = 0.0;
    }

    /// 1 サンプル進めて、ブリッジに加わる力 [N] を返す。
    ///
    /// 撥が接触している間だけ [`Self::oversample`] 倍のレートで回す。接触は
    /// 1 ms 未満なので、全体のコストにはほぼ効かない。
    #[inline]
    pub fn process_sample(&mut self) -> Sample {
        if !self.hammer.is_active() {
            return self.bank.process_sample(0.0, Rate::Base);
        }

        let dt_os = 1.0 / (self.sample_rate * self.oversample as f64);
        let mut last = 0.0 as Sample;
        let mut acc = 0.0 as Sample;

        for _ in 0..self.oversample {
            let displacement = self.bank.displacement_at_strike() as f64;
            // 速度は差分で作る。結合形の実部から作る手もあるが、
            // 差分のほうが「いま撥が見ている弦の動き」に素直に対応する。
            let velocity = (displacement - self.prev_displacement) / dt_os;
            self.prev_displacement = displacement;

            let force = self.hammer.step(displacement, velocity, dt_os);
            last = self.bank.process_sample(force as Sample, Rate::Oversampled);
            acc += last;
        }

        match self.decimation {
            Decimation::Drop => last,
            Decimation::Average => acc / self.oversample as Sample,
        }
    }

    /// ブロックを埋める (加算ではなく上書き)。
    pub fn process_block(&mut self, out: &mut [Sample]) {
        for s in out.iter_mut() {
            *s = self.process_sample();
        }
    }

    /// 打弦点での弦の変位 [m]。検証用。
    pub fn displacement_at_strike(&self) -> Sample {
        self.bank.displacement_at_strike()
    }

    /// 状態が有限か。
    pub fn is_finite(&self) -> bool {
        self.bank.is_finite()
    }

    /// 撥の幅による励振の重み。
    ///
    /// **撥は点ではなく面で弦を押す。** 幅より短い波長の部分音は接触面の中で
    /// 打ち消し合う。矩形の接触面なら `sinc` になるが、それは符号が反転して
    /// 櫛を作る。実際の面は端ほど圧力が低いので、正で単調に落ちるガウス形で
    /// 近似する。肩の位置 (第 `2L/(πw)` 部分音) は矩形と同じ。
    fn width_weight(&self, nf: f64) -> f64 {
        let w = self.hammer.params().hammer_width_m;
        if w <= 0.0 || self.params.length_m <= 0.0 {
            return 1.0;
        }
        let u = std::f64::consts::PI * nf * w / (2.0 * self.params.length_m);
        (-0.5 * u * u).exp()
    }

    /// モードの係数を作り直す。
    ///
    /// 打弦点・撥・減衰・倍率のどれかが変わったら呼ぶ。**オーディオスレッドから
    /// 呼ばれる** (打弦点は演奏中に動く) ので、確保はしない。
    fn rebuild(&mut self) {
        let mut modes = self.params.mode_count(self.sample_rate);
        if self.mode_limit > 0 {
            modes = modes.min(self.mode_limit);
        }
        self.modes = modes;
        self.bank.set_active_modes(modes);

        let modal_mass = self.params.modal_mass();
        let tension = self.params.tension();
        let length = self.params.length_m;
        if modal_mass <= 0.0 || length <= 0.0 {
            return;
        }

        let dt_base = 1.0 / self.sample_rate;
        let dt_os = dt_base / self.oversample as f64;

        for i in 0..modes {
            let nf = (i + 1) as f64;
            let freq = self.params.partial_hz(i + 1);

            // 励振の重み: 打弦点のモード形状 × 撥の幅。
            let shape =
                (std::f64::consts::PI * nf * self.strike_ratio).sin() * self.width_weight(nf);

            // ブリッジ力 F = T·Σ a_n·(nπ/L)。高次ほど強く駆動する。
            let out_weight = tension * nf * std::f64::consts::PI / length;

            let spec = ModeSpec {
                freq_hz: freq,
                decay: self.damping.decay_at(freq),
                strike_weight: shape,
                modal_mass,
                out_weight,
            };
            self.bank.set_mode(i, Rate::Base, dt_base, &spec);
            self.bank.set_mode(i, Rate::Oversampled, dt_os, &spec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    fn segment() -> Segment {
        Segment::new(SegmentParams::treble_low_long(), SR)
    }

    /// 打弦して `seconds` 秒ぶんレンダリングする。
    fn render(seg: &mut Segment, velocity: f64, seconds: f64) -> Vec<Sample> {
        seg.strike(velocity);
        let n = (SR * seconds) as usize;
        let mut out = vec![0.0 as Sample; n];
        seg.process_block(&mut out);
        out
    }

    /// 指定周波数の振幅 (Goertzel + Hann 窓)。
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

    // ---- 設計則 ------------------------------------------------------------

    #[test]
    fn design_quantities_are_physically_plausible() {
        let p = SegmentParams::treble_low_long();

        // 波動速度は 2·L·f0 に一致する (定義そのもの)。
        assert_relative_eq!(p.wave_speed(), 2.0 * 0.495 * 293.66, max_relative = 1e-12);

        // 張力から逆算した f0 が元に戻る。
        let f0 = (p.tension() / p.linear_density()).sqrt() / (2.0 * p.length_m);
        assert_relative_eq!(f0, p.f0_hz, max_relative = 1e-12);

        // 応力が music wire の実用範囲 (破断 2000 MPa の 20–60%) に入る。
        let mpa = p.stress_pa() / 1e6;
        assert!(
            (400.0..=1200.0).contains(&mpa),
            "応力 {mpa:.0} MPa は music wire として不自然"
        );

        // インハーモニシティはピアノの中音域と同オーダー (1e-4 〜 1e-3)。
        let b = p.inharmonicity();
        assert!((1e-4..=1e-3).contains(&b), "B = {b:.3e} が想定外");
    }

    #[test]
    fn partial_frequencies_follow_the_stiff_string_law() {
        // Phase 1 の完了条件: f_n = n·f0·√(1+B·n²) と一致 (相対誤差 < 1e-6)。
        let p = SegmentParams::treble_low_long();
        let b = p.inharmonicity();
        for n in [1usize, 2, 5, 10, 20, 40] {
            let nf = n as f64;
            let expected = nf * p.f0_hz * (1.0 + b * nf * nf).sqrt();
            assert_relative_eq!(p.partial_hz(n), expected, max_relative = 1e-12);
        }
        // 単調増加。
        for n in 1..40 {
            assert!(p.partial_hz(n + 1) > p.partial_hz(n));
        }
    }

    #[test]
    fn mode_count_stops_below_nyquist() {
        let p = SegmentParams::treble_low_long();
        let n = p.mode_count(SR);
        assert!(n > 0);
        assert!(p.partial_hz(n) < SR * 0.5);
        // 次の 1 本はナイキストを超える (= ぎりぎりまで使っている)。
        assert!(p.partial_hz(n + 1) >= SR * 0.5 * 0.98);
    }

    #[test]
    fn higher_sample_rate_gives_more_modes() {
        let p = SegmentParams::treble_low_long();
        assert!(p.mode_count(96_000.0) > p.mode_count(48_000.0));
        // 96 kHz でも MAX_MODES に収まる (枠を 256 にした根拠)。
        assert!(p.mode_count(96_000.0) < MAX_MODES);
    }

    // ---- 減衰 --------------------------------------------------------------

    #[test]
    fn damping_anchors_are_reproduced() {
        let d = DampingParams::from_t60_anchors(293.66, 10.0, 5_000.0, 0.8);
        assert_relative_eq!(d.t60_at(293.66), 10.0, max_relative = 1e-9);
        assert_relative_eq!(d.t60_at(5_000.0), 0.8, max_relative = 1e-9);
        // 高い部分音ほど速く減衰する。
        assert!(d.t60_at(1_000.0) < d.t60_at(293.66));
        assert!(d.t60_at(10_000.0) < d.t60_at(5_000.0));
    }

    #[test]
    fn rendered_decay_matches_the_design() {
        // Phase 1 の完了条件: 設計 T60 と実測が 5% 以内。
        // 減衰の速い部分音を短時間で測る (基音の 10 秒は測定に時間がかかりすぎる)。
        let mut seg = segment();
        let damping = DampingParams::from_t60_anchors(300.0, 1.0, 3_000.0, 0.3);
        seg.set_damping(damping);

        let x = render(&mut seg, 2.0, 1.2);
        let f1 = seg.params().partial_hz(1);
        let design = damping.t60_at(f1);

        // 0.1 秒窓を 2 つ取り、振幅比から T60 を出す。
        let w = (SR * 0.1) as usize;
        let early = magnitude_at(&x[(SR * 0.1) as usize..(SR * 0.1) as usize + w], f1);
        let late = magnitude_at(&x[(SR * 0.6) as usize..(SR * 0.6) as usize + w], f1);
        let measured = 3.0 * std::f64::consts::LN_10 * 0.5 / (early / late).ln();

        assert_relative_eq!(measured, design, max_relative = 0.05);
    }

    // ---- 打弦点 ------------------------------------------------------------

    #[test]
    fn strike_point_puts_a_notch_on_the_eighth_partial() {
        // Phase 1 の完了条件: x/L = 1/8 で第 8 部分音にノッチ。
        let mut seg = segment();
        seg.set_strike_ratio(0.125);
        let x = render(&mut seg, 2.0, 0.5);

        let p = seg.params();
        let at = |n: usize| magnitude_at(&x, p.partial_hz(n));

        // 第 8 部分音が両隣より桁で小さい。
        assert!(
            at(8) < at(7) * 0.01,
            "第 8 部分音 {:.3e} が第 7 {:.3e} に対して落ちていない",
            at(8),
            at(7)
        );
        assert!(at(8) < at(9) * 0.01);
        // 第 16 部分音も節に当たる。
        assert!(at(16) < at(15) * 0.05);
    }

    #[test]
    fn strike_point_moves_the_notch() {
        // 1/4 で叩けばノッチは第 4 部分音に移る。
        let mut seg = segment();
        seg.set_strike_ratio(0.25);
        let x = render(&mut seg, 2.0, 0.5);
        let p = *seg.params();
        let at = |n: usize| magnitude_at(&x, p.partial_hz(n));

        assert!(at(4) < at(3) * 0.01);
        assert!(at(8) < at(7) * 0.05);
    }

    #[test]
    fn striking_nearer_the_bridge_is_brighter() {
        // 奏者がブリッジ寄りを叩くと明るくなる。この楽器の主要な音色操作。
        let brightness = |ratio: f64| -> f64 {
            let mut seg = segment();
            seg.set_strike_ratio(ratio);
            let x = render(&mut seg, 2.0, 0.4);
            let p = *seg.params();
            let low: f64 = (1..=3).map(|n| magnitude_at(&x, p.partial_hz(n))).sum();
            let high: f64 = (8..=20).map(|n| magnitude_at(&x, p.partial_hz(n))).sum();
            high / low
        };

        let near = brightness(0.05);
        let far = brightness(0.25);
        assert!(
            near > far,
            "ブリッジ寄り {near:.4} が中央寄り {far:.4} より明るくない"
        );
    }

    #[test]
    fn strike_ratio_is_clamped() {
        let mut seg = segment();
        seg.set_strike_ratio(-1.0);
        assert!(seg.strike_ratio() > 0.0);
        seg.set_strike_ratio(10.0);
        assert!(seg.strike_ratio() <= 0.5);
    }

    // ---- 撥と出力 ----------------------------------------------------------

    #[test]
    fn wood_hammer_contact_is_brief_and_ends() {
        // 木の撥の弦上の接触は数十 µs の接触の繰り返し (チャタリング) になり、
        // 「接触時間の合計」は倍率にも速度にも滑らかに依存しない (v=6 で単調性が
        // 崩れることを実測済み、→ D-012)。ここでは頑健な性質だけを固定する:
        // 接触が起きること、合計が実機のオーダー (0.2–2 ms) に収まること、
        // 撥が最終的に弦から離れること。
        // 撥そのものの接触時間の単調減少は剛体壁で検証している (hammer::tests)。
        for v in [0.5, 2.0, 6.0] {
            let mut seg = segment();
            render(&mut seg, v, 0.05);
            let ms = seg.hammer().contact_duration() * 1000.0;
            assert!(
                (0.2..=2.0).contains(&ms),
                "v={v} m/s で接触時間の合計 {ms:.3} ms が想定外"
            );
            assert!(
                !seg.hammer().is_active(),
                "v={v} m/s で撥が 50 ms 経っても離れていない"
            );
        }
    }

    #[test]
    fn felt_hammer_contact_falls_with_velocity() {
        // フェルト面は接触が 1 回で閉じるので、接触時間が収束した観測量になる。
        // 実測 (os=16): 4.96 → 4.37 ms で単調減少。ピアノのフェルトの範囲。
        let mut durations = Vec::new();
        for v in [0.5, 1.0, 2.0, 4.0, 6.0] {
            let mut seg = segment();
            seg.set_hammer_params(HammerParams::felt());
            render(&mut seg, v, 0.05);
            let ms = seg.hammer().contact_duration() * 1000.0;
            assert!(
                (1.0..=8.0).contains(&ms),
                "v={v} m/s でフェルトの接触時間 {ms:.3} ms が想定外"
            );
            durations.push(ms);
        }
        for w in durations.windows(2) {
            assert!(w[1] < w[0], "接触時間が単調減少していない: {durations:?}");
        }
    }

    #[test]
    fn output_grows_with_velocity() {
        let peak = |v: f64| -> f32 {
            let mut seg = segment();
            render(&mut seg, v, 0.2)
                .iter()
                .fold(0.0f32, |a, &b| a.max(b.abs()))
        };
        let (soft, loud) = (peak(0.5), peak(4.0));
        assert!(loud > soft * 2.0, "soft={soft:.3} loud={loud:.3}");
    }

    #[test]
    fn harder_strikes_are_brighter() {
        // 接触が短くなるぶん高次まで励振される。撥の非線形性の現れ。
        let ratio = |v: f64| -> f64 {
            let mut seg = segment();
            let x = render(&mut seg, v, 0.3);
            let p = *seg.params();
            let low: f64 = (1..=3).map(|n| magnitude_at(&x, p.partial_hz(n))).sum();
            let high: f64 = (10..=25).map(|n| magnitude_at(&x, p.partial_hz(n))).sum();
            high / low
        };
        assert!(ratio(6.0) > ratio(0.5));
    }

    #[test]
    fn output_is_finite_and_settles() {
        let mut seg = segment();
        let x = render(&mut seg, 6.0, 2.0);
        assert!(seg.is_finite());
        assert!(x.iter().all(|s| s.is_finite()));
        // 後半のほうが静か (減衰している)。
        let head = x[..4_800].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        let tail = x[x.len() - 4_800..]
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(tail < head);
    }

    #[test]
    fn a_second_strike_adds_to_the_ringing_string() {
        // ダンパーが無いので、鳴っている弦を叩き直すと足し込まれる。
        // ロール奏法がこれで自然に出る。Phase 3 の全弦常時走行の前提でもある。
        let mut seg = segment();
        seg.strike(2.0);
        let mut buf = vec![0.0 as Sample; (SR * 0.2) as usize];
        seg.process_block(&mut buf);

        let before = seg.displacement_at_strike().abs();
        assert!(before > 0.0, "1 打目が鳴っていない");

        // 状態を消さずに 2 打目。
        seg.strike(2.0);
        let mut buf2 = vec![0.0 as Sample; 32];
        seg.process_block(&mut buf2);
        assert!(seg.is_finite());
    }

    #[test]
    fn reset_silences_the_segment() {
        let mut seg = segment();
        render(&mut seg, 2.0, 0.1);
        seg.reset();
        assert_eq!(seg.displacement_at_strike(), 0.0);
        let mut buf = vec![0.0 as Sample; 128];
        seg.process_block(&mut buf);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    // ---- モード数と倍率 ----------------------------------------------------

    #[test]
    fn mode_limit_truncates_the_top() {
        let mut seg = segment();
        let full = seg.partial_count();
        seg.set_mode_limit(16);
        assert_eq!(seg.partial_count(), 16);
        seg.set_mode_limit(0);
        assert_eq!(seg.partial_count(), full);
    }

    #[test]
    fn truncating_modes_removes_the_top_partials() {
        let spectrum = |limit: usize| -> f64 {
            let mut seg = segment();
            seg.set_strike_ratio(0.05);
            seg.set_mode_limit(limit);
            let x = render(&mut seg, 3.0, 0.3);
            let p = *seg.params();
            (25..=40).map(|n| magnitude_at(&x, p.partial_hz(n))).sum()
        };
        // 20 本に絞れば第 25 部分音から上は消える。
        assert!(spectrum(20) < spectrum(60) * 0.01);
    }

    #[test]
    fn oversampling_is_converged_at_the_default() {
        // 既定の 16 倍が収束域にあることを固定する。64 倍と比べて低次の部分音が
        // 一致しなくなったら、オーバーサンプルの入り口か出口で何かを壊している。
        //
        // os=4 や 8 と比べては**いけない**。そちらは収束しておらず (木の高次で
        // 最大 20 dB ずれる)、一致しないのが正しい。→ D-010
        let low_partials = |os: usize| -> Vec<f64> {
            let mut seg = segment();
            seg.set_oversample(os);
            let x = render(&mut seg, 2.0, 0.3);
            let p = *seg.params();
            (1..=5).map(|n| magnitude_at(&x, p.partial_hz(n))).collect()
        };

        let a = low_partials(16);
        let b = low_partials(64);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_relative_eq!(x, y, max_relative = 0.15);
        }
    }

    #[test]
    fn decimation_modes_both_run() {
        for mode in [Decimation::Drop, Decimation::Average] {
            let mut seg = segment();
            seg.set_decimation(mode);
            let x = render(&mut seg, 2.0, 0.2);
            assert!(x.iter().all(|s| s.is_finite()));
            assert!(x.iter().any(|&s| s.abs() > 0.0));
        }
    }
}
