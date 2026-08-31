//! ROOM — X-Y ステレオ録音の模倣。
//!
//! 楽器の幾何と部屋を同じ座標系に置き、後段の汎用リバーブでは失われる
//! 「楽器の各部が空間のどこにあるか」を保つ (`docs/plan.html` §04)。
//!
//! # X-Y の 3 つの性質と、その実装上の現れ
//!
//! X-Y は 2 本の指向性マイクを**同一点**に置き、軸を開いて構える方式。
//!
//! 1. **L/R に時間差が生じない** → 直接音・初期反射・後部残響のどこにも
//!    L/R 間の遅延差を作らない。反射タップは 1 本の遅延線を読み、L/R には
//!    **ゲインだけ**を分ける
//! 2. **定位はレベル差だけ** → カーディオイド `p(θ) = 0.5 + 0.5·cos θ` を
//!    ±開き角/2 の 2 軸で評価する
//! 3. **幅は控えめ** → つまみで開き角は変えられるが、既定は実物どおり 90°
//!
//! # 3 層
//!
//! - **直接音**: 音源 (バスブリッジ / トレブルブリッジ) ごとに方位と距離。
//!   カーディオイド × 1/d
//! - **初期反射**: 靴箱の部屋の鏡像法で音源ごとに 12 タップ。タップごとに
//!   遅延・減衰・**到来方位**を持ち、同じカーディオイド行列を通す。
//!   X-Y らしさが一番出る層
//! - **後部残響**: FDN 8 ライン。**L/R の相関を高く保つ** (既定 0.7) —
//!   同一点収音だから。無相関にすると AB (スペースドペア) になってしまう。
//!   相関は 2 つの出力ミクスベクトルの内積で設計する (遅延では作らない)
//!
//! # 検証 (P6 の完了条件)
//!
//! ROOM は**参照音源なしで数値が閉じる**数少ない部分:
//! L/R 相互相関のピークが遅延 0 / モノ和が櫛にならない / レベル差が
//! カーディオイドの理論値と一致 / RT60・ITDG が部屋の設定と整合。

use crate::Sample;

/// 音速 [m/s]。
const SPEED_OF_SOUND: f64 = 343.0;

/// 遅延線の長さ (2 の冪)。170 ms @ 48 kHz — 最大の部屋の反射に足りる。
const LINE_LEN: usize = 8192;
const LINE_MASK: usize = LINE_LEN - 1;

/// 音源ごとの初期反射タップ数。
const TAPS_PER_SOURCE: usize = 12;

/// FDN のライン数。
const FDN_LINES: usize = 8;

/// 後部残響の L/R 相関の設計値。
///
/// 同一点収音の X-Y は残響でも相関が高い (0.6–0.8)。市販リバーブの
/// 「無相関で広げる」は離して置いた 2 本のマイクの音であって X-Y ではない。
const TAIL_CORRELATION: f64 = 0.7;

/// 部屋の大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomSize {
    Small,
    Medium,
    Large,
}

impl RoomSize {
    /// (幅 x, 奥行き y, 高さ z) [m] と RT60 の基準値 [s] (absorption = 0.35 のとき)。
    fn dims(self) -> ([f64; 3], f64) {
        match self {
            RoomSize::Small => ([3.5, 2.8, 2.4], 0.45),
            RoomSize::Medium => ([5.0, 4.0, 2.8], 0.80),
            RoomSize::Large => ([8.0, 6.0, 3.5], 1.50),
        }
    }
}

/// ROOM のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomParams {
    /// マイクから楽器の中心までの距離 [m]
    pub mic_distance_m: f64,
    /// X-Y の開き角 [deg] (2 軸の間の角)
    pub xy_angle_deg: f64,
    pub size: RoomSize,
    /// 壁の吸音率 (0–1)。RT60 と高域の落ち方を決める
    pub absorption: f64,
}

impl Default for RoomParams {
    fn default() -> Self {
        Self {
            mic_distance_m: 1.2,
            xy_angle_deg: 90.0,
            size: RoomSize::Medium,
            absorption: 0.35,
        }
    }
}

/// カーディオイドの感度。`theta_rel` は軸からの角度 [rad]。
#[inline]
fn cardioid(theta_rel: f64) -> f64 {
    0.5 + 0.5 * theta_rel.cos()
}

/// 方位 `azimuth` (正面 = 0、右が正) の音に対する (L, R) のゲイン。
///
/// L 軸は −半開き角、R 軸は +半開き角。**時間差は作らない — これが X-Y。**
pub fn xy_gains(azimuth_rad: f64, xy_angle_deg: f64) -> (f64, f64) {
    let half = xy_angle_deg.to_radians() * 0.5;
    (cardioid(azimuth_rad + half), cardioid(azimuth_rad - half))
}

/// 初期反射の 1 タップ。
#[derive(Debug, Clone, Copy, Default)]
struct Tap {
    delay: usize,
    gain_l: Sample,
    gain_r: Sample,
}

/// 1 音源ぶんの直接音 + 初期反射。
struct SourcePath {
    line: Vec<Sample>,
    pos: usize,
    direct_l: Sample,
    direct_r: Sample,
    taps: [Tap; TAPS_PER_SOURCE],
}

impl SourcePath {
    fn new() -> Self {
        Self {
            line: vec![0.0; LINE_LEN],
            pos: 0,
            direct_l: 0.0,
            direct_r: 0.0,
            taps: [Tap::default(); TAPS_PER_SOURCE],
        }
    }
}

/// FDN (後部残響)。
struct Fdn {
    lines: Vec<Vec<Sample>>,
    pos: [usize; FDN_LINES],
    delays: [usize; FDN_LINES],
    /// ラインごとのフィードバックゲイン (RT60 から)
    fb: [Sample; FDN_LINES],
    /// 高域減衰の 1 次 LP 状態と係数
    lp: [Sample; FDN_LINES],
    lp_a: Sample,
    /// 出力ミクス (相関を設計する 2 本のベクトル)
    mix_l: [Sample; FDN_LINES],
    mix_r: [Sample; FDN_LINES],
    wet: Sample,
}

impl Fdn {
    fn new() -> Self {
        Self {
            lines: (0..FDN_LINES).map(|_| vec![0.0; LINE_LEN]).collect(),
            pos: [0; FDN_LINES],
            delays: [1; FDN_LINES],
            fb: [0.0; FDN_LINES],
            lp: [0.0; FDN_LINES],
            lp_a: 0.5,
            mix_l: [0.0; FDN_LINES],
            mix_r: [0.0; FDN_LINES],
            wet: 0.0,
        }
    }
}

/// ROOM 本体。
pub struct Room {
    sample_rate: f64,
    params: RoomParams,
    sources: [SourcePath; 2],
    fdn: Fdn,
}

/// 楽器の幾何: マイク正面から見た音源の横位置 [m]。
///
/// ダルシマーは横に約 1 m。バスブリッジは奏者から見て右 (聴き手からは左)、
/// トレブルブリッジは中央やや左 (聴き手からは右) に立つ。
const SOURCE_OFFSETS_M: [f64; 2] = [-0.35, 0.25];

impl Room {
    pub fn new(params: RoomParams, sample_rate: f64) -> Self {
        let mut room = Self {
            sample_rate,
            params,
            sources: [SourcePath::new(), SourcePath::new()],
            fdn: Fdn::new(),
        };
        room.rebuild();
        room
    }

    pub fn params(&self) -> &RoomParams {
        &self.params
    }

    /// パラメータを差し替える。**確保しない** (タップと係数の再計算だけ)。
    pub fn set_params(&mut self, params: RoomParams) {
        if self.params != params {
            self.params = params;
            self.rebuild();
        }
    }

    pub fn reset(&mut self) {
        for s in &mut self.sources {
            s.line.fill(0.0);
        }
        for l in &mut self.fdn.lines {
            l.fill(0.0);
        }
        self.fdn.lp = [0.0; FDN_LINES];
    }

    /// 幾何からタップと係数を作り直す。
    fn rebuild(&mut self) {
        let p = self.params;
        let ([w, l, h], rt60_base) = p.size.dims();
        let d = p.mic_distance_m.clamp(0.3, 3.0);

        // マイクは部屋の中央やや後ろ、高さ 1.2 m。楽器はその正面 d [m]。
        let mic = [w * 0.5, l * 0.35, 1.2];
        let src_y = mic[1] + d;
        let src_h = 0.9;

        // 反射係数 (振幅)。吸音率 α の壁で 1 回反射するたび √(1−α)。
        let refl = (1.0 - p.absorption.clamp(0.0, 0.95)).sqrt();

        for (si, source) in self.sources.iter_mut().enumerate() {
            let sx = mic[0] + SOURCE_OFFSETS_M[si];
            let src = [sx, src_y, src_h];

            // 直接音。
            let (az, dist) = azimuth_and_distance(&mic, &src);
            let (gl, gr) = xy_gains(az, p.xy_angle_deg);
            let g = 1.0 / dist.max(0.3);
            source.direct_l = (gl * g) as Sample;
            source.direct_r = (gr * g) as Sample;

            // 鏡像法: 1 次 6 面 + 横壁どうしの 2 次 4 + 床と横壁の 2 次 2。
            let images = [
                // (x 反転壁, y 反転壁, z 反転壁) — 0 = しない, -1 = 原点側, +1 = 遠い側
                [-1, 0, 0],
                [1, 0, 0],
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, -1, 0],
                [1, -1, 0],
                [-1, 1, 0],
                [1, 1, 0],
                [-1, 0, -1],
                [1, 0, -1],
            ];
            for (ti, refl_spec) in images.iter().enumerate() {
                let mut pos = src;
                let mut order = 0;
                let dims = [w, l, h];
                for (axis, &side) in refl_spec.iter().enumerate() {
                    match side {
                        -1 => {
                            pos[axis] = -pos[axis];
                            order += 1;
                        }
                        1 => {
                            pos[axis] = 2.0 * dims[axis] - pos[axis];
                            order += 1;
                        }
                        _ => {}
                    }
                }
                let (az, dist) = azimuth_and_distance(&mic, &pos);
                let (gl, gr) = xy_gains(az, p.xy_angle_deg);
                let g = refl.powi(order) / dist.max(0.3);
                let delay = ((dist / SPEED_OF_SOUND) * self.sample_rate).round() as usize;
                source.taps[ti] = Tap {
                    delay: delay.min(LINE_MASK),
                    gain_l: (gl * g) as Sample,
                    gain_r: (gr * g) as Sample,
                };
            }
        }

        // FDN。遅延は互いに素な近傍の値 × 部屋の大きさ。
        let scale = (l / 4.0).clamp(0.5, 2.5);
        let base_delays = [1123usize, 1361, 1499, 1667, 1801, 2053, 2251, 2399];
        let rt60 = (rt60_base * (1.5 - p.absorption) / 1.15).max(0.1);
        // 並行する複数の配列 (delays / fb) を同じ添字で埋めるので range で回す。
        #[allow(clippy::needless_range_loop)]
        for i in 0..FDN_LINES {
            let delay = ((base_delays[i] as f64 * scale) as usize).min(LINE_MASK);
            self.fdn.delays[i] = delay.max(1);
            // このラインを 1 周するたびの減衰が RT60 に合うように。
            let seconds = self.fdn.delays[i] as f64 / self.sample_rate;
            self.fdn.fb[i] = 10.0f64.powf(-3.0 * seconds / rt60) as Sample;
        }
        // 高域の減衰: 吸音率が高いほど強く落とす。
        let fc = 6_000.0 * (1.0 - p.absorption * 0.7);
        self.fdn.lp_a = (1.0 - (-std::f64::consts::TAU * fc / self.sample_rate).exp()) as Sample;

        // 出力ミクス: 相関 ρ を内積で設計する (遅延では作らない — X-Y)。
        // a = 全部 +、b = 交互 ± (直交)。R = ρ·a + √(1−ρ²)·b。
        let rho = TAIL_CORRELATION;
        let orth = (1.0 - rho * rho).sqrt();
        let norm = 1.0 / (FDN_LINES as f64).sqrt();
        for i in 0..FDN_LINES {
            let a = norm;
            let b = if i % 2 == 0 { norm } else { -norm };
            self.fdn.mix_l[i] = a as Sample;
            self.fdn.mix_r[i] = (rho * a + orth * b) as Sample;
        }

        // 残響の量: マイクが遠いほど直接音 (1/d) が下がり、残響は部屋のもの
        // なので変わらない。wet は距離に依存させない。
        self.fdn.wet = 0.06;
    }

    /// 1 サンプル: 2 音源 (バス系統, トレブル系統) → (L, R)。
    #[inline]
    pub fn process_sample(&mut self, bass: Sample, treble: Sample) -> (Sample, Sample) {
        let mut l = 0.0 as Sample;
        let mut r = 0.0 as Sample;

        for (source, &input) in self.sources.iter_mut().zip([bass, treble].iter()) {
            // 遅延線へ書く。
            source.pos = (source.pos + 1) & LINE_MASK;
            source.line[source.pos] = input;

            // 直接音 (遅延なし — 全体の共通遅延は省く)。
            l += input * source.direct_l;
            r += input * source.direct_r;

            // 初期反射: 1 本の遅延線を読み、L/R にはゲインだけを分ける。
            for tap in &source.taps {
                let v = source.line[(source.pos.wrapping_sub(tap.delay)) & LINE_MASK];
                l += v * tap.gain_l;
                r += v * tap.gain_r;
            }
        }

        // FDN。入力は両系統の和。
        let send = bass + treble;
        let mut outs = [0.0 as Sample; FDN_LINES];
        // 並行配列の添字アクセスなので range で回す (以下の 2 ループも同じ)。
        #[allow(clippy::needless_range_loop)]
        for i in 0..FDN_LINES {
            let p = self.fdn.pos[i];
            outs[i] = self.fdn.lines[i][(p.wrapping_sub(self.fdn.delays[i])) & LINE_MASK];
        }
        // Hadamard 8×8 (バタフライ 3 段)。
        let m = hadamard8(outs);
        for i in 0..FDN_LINES {
            // 高域減衰 → フィードバックゲイン → 入力を足して書き戻す。
            self.fdn.lp[i] += self.fdn.lp_a * (m[i] - self.fdn.lp[i]);
            let v = self.fdn.lp[i] * self.fdn.fb[i] + send * if i % 2 == 0 { 0.25 } else { -0.25 };
            self.fdn.pos[i] = (self.fdn.pos[i] + 1) & LINE_MASK;
            let p = self.fdn.pos[i];
            self.fdn.lines[i][p] = v;

            l += outs[i] * self.fdn.mix_l[i] * self.fdn.wet;
            r += outs[i] * self.fdn.mix_r[i] * self.fdn.wet;
        }

        (l, r)
    }
}

/// マイクから見た方位 [rad] (正面 = +y、右が正) と 3D 距離 [m]。
fn azimuth_and_distance(mic: &[f64; 3], src: &[f64; 3]) -> (f64, f64) {
    let dx = src[0] - mic[0];
    let dy = src[1] - mic[1];
    let dz = src[2] - mic[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    (dx.atan2(dy), dist)
}

/// 8 点 Hadamard 変換 (正規化つき)。
#[inline]
fn hadamard8(x: [Sample; 8]) -> [Sample; 8] {
    let mut y = x;
    // 3 段のバタフライ。
    for stride in [1usize, 2, 4] {
        let mut out = [0.0 as Sample; 8];
        for i in 0..8 {
            let partner = i ^ stride;
            out[i] = if i & stride == 0 {
                y[i] + y[partner]
            } else {
                y[partner] - y[i]
            };
        }
        y = out;
    }
    let norm = 1.0 / (8.0f32).sqrt();
    y.map(|v| v * norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// 2 音源に信号を流して L/R を集める。
    fn run(
        room: &mut Room,
        n: usize,
        mut input: impl FnMut(usize) -> (Sample, Sample),
    ) -> (Vec<Sample>, Vec<Sample>) {
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let (b, t) = input(i);
            let (yl, yr) = room.process_sample(b, t);
            l.push(yl);
            r.push(yr);
        }
        (l, r)
    }

    /// 正規化相互相関。返り値は (最大の係数, そのラグ, ラグ 0 の係数)。
    fn cross_correlation(l: &[Sample], r: &[Sample], max_lag: usize) -> (f64, i64, f64) {
        let energy = |x: &[Sample]| x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
        let denom = (energy(l) * energy(r)).sqrt();
        if denom <= 0.0 {
            return (0.0, 0, 0.0);
        }
        let mut best = (f64::MIN, 0i64);
        let mut at_zero = 0.0;
        for lag in -(max_lag as i64)..=(max_lag as i64) {
            let mut acc = 0.0;
            // ラグつきの 2 配列参照なので range で回す。
            #[allow(clippy::needless_range_loop)]
            for i in 0..l.len() {
                let j = i as i64 + lag;
                if j >= 0 && (j as usize) < r.len() {
                    acc += l[i] as f64 * r[j as usize] as f64;
                }
            }
            let c = acc / denom;
            if lag == 0 {
                at_zero = c;
            }
            if c > best.0 {
                best = (c, lag);
            }
        }
        (best.0, best.1, at_zero)
    }

    /// 決定的なノイズ。
    fn noise(seed: u32, n: usize) -> Vec<Sample> {
        let mut x = seed | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f64 / u32::MAX as f64 - 0.5) as Sample
            })
            .collect()
    }

    #[test]
    fn cardioid_gains_match_the_formula() {
        // P6 の完了条件: レベル差が理論値と一致 (構造そのものの検証)。
        for az_deg in [-60.0f64, -30.0, 0.0, 15.0, 45.0] {
            let (l, r) = xy_gains(az_deg.to_radians(), 90.0);
            let expect_l = 0.5 + 0.5 * (az_deg + 45.0).to_radians().cos();
            let expect_r = 0.5 + 0.5 * (az_deg - 45.0).to_radians().cos();
            assert!((l - expect_l).abs() < 1e-12);
            assert!((r - expect_r).abs() < 1e-12);
        }
        // 左の音源は L が大きい。
        let (l, r) = xy_gains((-40.0f64).to_radians(), 90.0);
        assert!(l > r);
    }

    #[test]
    fn the_cross_correlation_peaks_at_lag_zero() {
        // P6 の完了条件: X-Y の定義そのもの。どこにも L/R 間の遅延差が無い。
        let mut room = Room::new(RoomParams::default(), SR);
        let nb = noise(0xAAAA, (SR * 1.0) as usize);
        let nt = noise(0x5555, (SR * 1.0) as usize);
        let (l, r) = run(&mut room, nb.len(), |i| (nb[i], nt[i]));

        let (best, lag, _) = cross_correlation(&l, &r, 24);
        assert!(best > 0.0);
        assert_eq!(
            lag, 0,
            "相互相関のピークが遅延 {lag} にある (X-Y が壊れている)"
        );
    }

    #[test]
    fn the_tail_correlation_is_in_the_xy_range() {
        // P6 の完了条件: 残響の相関 0.5–0.8 (同一点収音)。
        // インパルスの後部残響だけを見る。
        let mut room = Room::new(RoomParams::default(), SR);
        let n = (SR * 1.2) as usize;
        let (l, r) = run(
            &mut room,
            n,
            |i| {
                if i == 0 {
                    (1.0, 1.0)
                } else {
                    (0.0, 0.0)
                }
            },
        );

        // 初期反射が終わった後 (0.25 秒以降) が後部残響。
        let from = (SR * 0.25) as usize;
        let (_, _, rho) = cross_correlation(&l[from..], &r[from..], 0);
        assert!(
            (0.4..=0.85).contains(&rho),
            "残響の相関 {rho:.2} が X-Y の範囲 (0.5–0.8 目標) から外れている"
        );
    }

    #[test]
    fn direct_sound_correlation_is_high() {
        // 直接音 (+初期反射前) はほぼ完全相関。
        let mut room = Room::new(RoomParams::default(), SR);
        let nb = noise(0x1234, 256);
        let (l, r) = run(&mut room, 200, |i| (nb[i], nb[i] * 0.7));
        // 最初の反射 (ITDG ~ 数 ms) より前だけを見る。
        let (_, _, rho) = cross_correlation(&l[..150], &r[..150], 0);
        assert!(rho > 0.9, "直接音の相関 {rho:.3} が低すぎる");
    }

    #[test]
    fn the_first_reflection_matches_the_geometry() {
        // ITDG: 最初の反射の遅れが部屋の幾何と整合する。
        // Medium (5×4×2.8)、マイク (2.5, 1.4, 1.2)、音源はその正面 1.2 m。
        // 最も近い経路は床反射: 直接 1.24 m に対し床経由 ~2.4 m → 差 ~1.2 m
        // ≈ 3.4 ms ≈ 165 サンプル。横壁は ~2.9 m 差。
        let mut room = Room::new(RoomParams::default(), SR);
        let (l, _) = run(&mut room, 2000, |i| {
            if i == 0 {
                (1.0, 0.0)
            } else {
                (0.0, 0.0)
            }
        });

        // 直接音はサンプル 0。次に振幅が立つ場所が最初の反射。
        let first_reflection = l
            .iter()
            .enumerate()
            .skip(8)
            .find(|(_, &v)| v.abs() > 1e-4)
            .map(|(i, _)| i)
            .expect("反射が見つからない");
        assert!(
            (100..400).contains(&first_reflection),
            "最初の反射が {first_reflection} サンプル (期待 100–400 = 幾何の床反射)"
        );
    }

    #[test]
    fn rt60_tracks_the_absorption_knob() {
        // 残響時間が設定に応じて動く。正確な RT60 一致ではなく、
        // つまみの方向が正しく効くことを固定する。
        let tail_level = |absorption: f64| -> f64 {
            let mut room = Room::new(
                RoomParams {
                    absorption,
                    ..RoomParams::default()
                },
                SR,
            );
            let n = (SR * 1.0) as usize;
            let (l, _) = run(
                &mut room,
                n,
                |i| {
                    if i == 0 {
                        (1.0, 1.0)
                    } else {
                        (0.0, 0.0)
                    }
                },
            );
            let seg = &l[(SR * 0.8) as usize..];
            (seg.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / seg.len() as f64).sqrt()
        };
        let dead = tail_level(0.8);
        let live = tail_level(0.1);
        assert!(
            live > dead * 3.0,
            "absorption が効いていない: live {live:.3e}, dead {dead:.3e}"
        );
    }

    #[test]
    fn mic_distance_changes_the_direct_to_reverb_ratio() {
        // マイクを離すと直接音だけが下がる (これ 1 本でタイト ⇔ アンビエント)。
        let direct_level = |d: f64| -> f64 {
            let mut room = Room::new(
                RoomParams {
                    mic_distance_m: d,
                    ..RoomParams::default()
                },
                SR,
            );
            let (l, _) = run(
                &mut room,
                64,
                |i| if i == 0 { (1.0, 1.0) } else { (0.0, 0.0) },
            );
            l[0].abs() as f64
        };
        let near = direct_level(0.4);
        let far = direct_level(2.5);
        assert!(near > far * 3.0, "距離が直接音に効いていない");
    }

    #[test]
    fn wider_angle_widens_the_image() {
        // 開き角を広げると、横の音源の L/R 差が増える。
        let side_ratio = |angle: f64| -> f64 {
            let (l, r) = xy_gains((-30.0f64).to_radians(), angle);
            l / r
        };
        assert!(side_ratio(120.0) > side_ratio(60.0));
    }

    #[test]
    fn the_room_is_stable_and_finite() {
        let mut room = Room::new(RoomParams::default(), SR);
        let nb = noise(0x77, (SR * 3.0) as usize);
        let (l, r) = run(&mut room, nb.len(), |i| (nb[i], -nb[i]));
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        // 入力を止めれば減衰する。
        let (l2, _) = run(&mut room, (SR * 2.0) as usize, |_| (0.0, 0.0));
        let early = l2[..4800].iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        let late = l2[l2.len() - 4800..]
            .iter()
            .fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(late < early, "残響が減衰していない");
    }

    #[test]
    fn set_params_rebuilds_without_breaking() {
        let mut room = Room::new(RoomParams::default(), SR);
        run(&mut room, 1000, |i| {
            if i == 0 {
                (1.0, 1.0)
            } else {
                (0.0, 0.0)
            }
        });
        room.set_params(RoomParams {
            mic_distance_m: 2.0,
            xy_angle_deg: 120.0,
            size: RoomSize::Large,
            absorption: 0.2,
        });
        let (l, r) = run(&mut room, 1000, |_| (0.0, 0.0));
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
    }
}
