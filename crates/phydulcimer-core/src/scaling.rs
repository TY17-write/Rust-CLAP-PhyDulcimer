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

/// バス最低コースの発弦長 [m] (使う側 = 長い側)。
const BASS_SPEAKING_BOTTOM_M: f64 = 0.74;
/// バス最高コースの発弦長 [m]。
const BASS_SPEAKING_TOP_M: f64 = 0.30;

/// 半音階配置 (E3–E6、Phase 7) のジオメトリ。
///
/// 一次資料の実測が無いので、**端の波速 `c = 2·L·f0` が 15/14 の同音域と
/// 同程度になる**よう選んだ設計値。妥当性は published-ranges テスト
/// (応力・張力・B の文献範囲) が固定する — 参照音源を持たないプロジェクト
/// では、これが唯一の外部基準 (D-006)。
const CHROM_BASS_SPEAKING_BOTTOM_M: f64 = 0.55;
const CHROM_BASS_SPEAKING_TOP_M: f64 = 0.32;
const CHROM_TREBLE_TOTAL_BOTTOM_M: f64 = 0.72;
const CHROM_TREBLE_TOTAL_TOP_M: f64 = 0.32;

/// トレブルブリッジの置き方 = 左区間の音律 (Phase 7)。
///
/// ブリッジは弦長を分割し、左区間の周波数は**分割比から物理的に導かれる**
/// (`f_left = f_right · S/(1−S)`)。音律の切り替えとはブリッジを僅かに
/// 動かすことで、弦の設計値 (長さ・f0) が変わる → 弦バンクの再構築を伴う
/// (プラグインでは activate 時に適用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Temperament {
    /// ブリッジをちょうど 2:3 に置く。左 = 右 × 1.5 (純正5度 702 cent、
    /// 平均律 +2 cent — 実機どおり、D-017)
    #[default]
    PureFifth,
    /// ブリッジを約 0.3 mm 動かして左を平均律の完全5度 (700 cent) に
    /// 合わせる (実機の調律師が取る妥協の再現)
    Equal12,
}

impl Temperament {
    /// 左区間の周波数比 (右に対する倍率)。
    pub fn fifth_ratio(self) -> f64 {
        match self {
            Temperament::PureFifth => 1.5,
            Temperament::Equal12 => (7.0 / 12.0f64).exp2(),
        }
    }

    /// トレブル長弦側 (右) の分割比 S。`f_left = f_right · S/(1−S)` なので
    /// S = r/(1+r)。PureFifth でちょうど 0.6 (= 2:3)。
    pub fn treble_long_share(self) -> f64 {
        let r = self.fifth_ratio();
        r / (1.0 + r)
    }
}

/// 設計の文脈 — 配置由来の数値 + 音律。[`design_position_with`] に渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignContext {
    pub bass_courses: usize,
    pub treble_courses: usize,
    /// 音域位置 t (線径の補間) の範囲
    pub key_min: u8,
    pub key_max: u8,
    pub bass_speaking_bottom_m: f64,
    pub bass_speaking_top_m: f64,
    pub treble_total_bottom_m: f64,
    pub treble_total_top_m: f64,
    pub temperament: Temperament,
}

impl DesignContext {
    /// 配置表と音律から文脈を組む。
    pub fn for_layout(layout: &crate::layout::Layout, temperament: Temperament) -> Self {
        use crate::layout::LayoutKind;
        let (bass_bottom, bass_top, treble_bottom, treble_top) = match layout.kind() {
            LayoutKind::Diatonic1514 => (
                BASS_SPEAKING_BOTTOM_M,
                BASS_SPEAKING_TOP_M,
                TREBLE_TOTAL_BOTTOM_M,
                TREBLE_TOTAL_TOP_M,
            ),
            LayoutKind::ChromaticE3E6 => (
                CHROM_BASS_SPEAKING_BOTTOM_M,
                CHROM_BASS_SPEAKING_TOP_M,
                CHROM_TREBLE_TOTAL_BOTTOM_M,
                CHROM_TREBLE_TOTAL_TOP_M,
            ),
        };
        Self {
            bass_courses: layout.bass_courses(),
            treble_courses: layout.treble_courses(),
            key_min: layout.key_min(),
            key_max: layout.key_max(),
            bass_speaking_bottom_m: bass_bottom,
            bass_speaking_top_m: bass_top,
            treble_total_bottom_m: treble_bottom,
            treble_total_top_m: treble_top,
            temperament,
        }
    }
}

impl Default for DesignContext {
    /// 15/14 標準配置・純正5度 (従来の設計そのまま)。
    fn default() -> Self {
        Self::for_layout(
            &crate::layout::Layout::standard_15_14(),
            Temperament::PureFifth,
        )
    }
}

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

/// 音域バランスの補償ゲイン [dB] のアンカー表 (Phase 10 前半)。
///
/// **物理定数ではなく校正値** (D-013 / D-020 と同じ流儀)。2026-08-31 の全鍵
/// 掃引 (ff・3 s・ROOM off・LUFS モーメンタリ最大、広がり 18.3 LU) を、
/// ターゲット直線 `LUFS(A4) + 1.0 LU/oct · log2(f0/440)` へ載せる逆カーブ。
///
/// 非単調なのはモデルの構造がそのまま出ているため:
///
/// - 110 / 196 Hz の浅い谷 — 箱 (cabinet) の双峰がその鍵だけ持ち上げている
/// - 130–330 Hz の +6〜8 dB — 響板の傾き `(f/1000)^2.5` と放射効率が低音の
///   基音を削っているぶんの戻し
/// - 494 Hz 以上の −4〜−6 dB — ブリッジ出力重み `w_n = T·nπ/L` と
///   励振側の `1/M_n` がどちらも短い弦 (高音) を優遇するぶんの抑え
///
/// 440 Hz は 0 dB に固定 (A4 の校正 `CALIBRATED_GAIN` を不動に保つ)。
const COURSE_GAIN_ANCHORS_DB: &[(f64, f64)] = &[
    (98.0, 2.0),
    (110.0, 1.5),
    (128.0, 6.5),
    (165.0, 8.0),
    (196.0, 5.2),
    (220.0, 6.5),
    (330.0, 6.0),
    (370.0, 0.9),
    (392.0, 2.5),
    (440.0, 0.0),
    (494.0, -4.2),
    (587.0, -4.75),
    (740.0, -4.55),
    (784.0, -5.6),
];

/// (log2 f, dB) 折れ線の補間。範囲外は端の値で一定。
fn interp_db_anchors(anchors: &[(f64, f64)], f_hz: f64) -> f64 {
    let f = f_hz.clamp(anchors[0].0, anchors[anchors.len() - 1].0);
    anchors
        .windows(2)
        .find(|w| f <= w[1].0)
        .map(|w| {
            let t = (f / w[0].0).log2() / (w[1].0 / w[0].0).log2();
            w[0].1 + t * (w[1].1 - w[0].1)
        })
        .unwrap_or(anchors[anchors.len() - 1].1)
}

/// 半音階配置 (E3–E6) のコースゲイン補正 [dB] (Phase 7、D-022)。
///
/// **15/14 の校正表 ([`COURSE_GAIN_ANCHORS_DB`]) の上に重ねる差分。**
/// 15/14 表は系統的な傾き (響板 tilt・箱・ブリッジ重み) を近似的に写すが、
/// 半音階は弦の設計 (長さ・線密度) が違うため鍵ごとの残差が ±4.8 LU 残った
/// (2026-08-31 の掃引、ff・3 s・ROOM off)。この表はその残差の打ち消しで、
/// **アンカー = 各コースの右側 f0** (ゲインはコース周波数でしか引かれない
/// ので実質ルックアップ)。共有コース対 (75↔82 等) は残差を折半した。
const CHROMATIC_GAIN_CORRECTION_DB: &[(f64, f64)] = &[
    (164.81, 0.0),
    (174.61, -0.6),
    (185.00, -1.1),
    (196.00, -4.0),
    (207.65, -2.4),
    (220.00, -1.7),
    (233.08, -1.4),
    (246.94, -2.0),
    (261.63, -2.1),
    (277.18, -4.0),
    (293.66, -3.1),
    (311.13, -2.5),
    (329.63, -4.5),
    (349.23, 0.1),
    (369.99, 0.4),
    (392.00, 0.2),
    (415.30, -0.5),
    (440.00, 0.0),
    (466.16, 0.1),
    (493.88, 2.1),
    (523.25, 4.6),
    (554.37, 1.0),
    (587.33, 2.9),
    (622.25, 0.3),
    (659.26, -1.9),
    (698.46, -1.4),
    (739.99, -4.2),
    (783.99, -1.6),
    (830.61, -0.3),
    (880.00, -0.8),
];

/// 撥の面のラウドネス補償 [dB] — レザー面 (Phase 10 後半、D-026)。
///
/// **物理定数ではなく校正値。** 柔らかい面は接触が長く高次部分音を励振
/// できないため、同じ打撃速度でも木より静かになる (K 特性のラウドネスは
/// 高域を重く見るので、トレブルでは 10–20 LU も凹む)。実機でも起きる現象
/// だが、音源としては面の切り替えで音量が跳ばないほうが使いやすいので、
/// **打った区間の f0** で引く補償ゲインを励振に掛けて木と揃える。
///
/// アンカー = 15/14 全 27 鍵の打った区間の f0 (実質ルックアップ、D-022 と
/// 同じ流儀)。値は 2026-09-01 の掃引 (ff と vel 0.5、3 s・ROOM off・LUFS
/// モーメンタリ最大) の **2 速度平均**の木との差を打ち消す量。
///
/// 既知の近似 (→ D-026): 面ごとの速度→ラウドネス曲線は形が違うので、
/// 静的なゲインでは 1 つの強さでしか厳密に揃わない (2 速度の残差は
/// ±4 LU 程度)。左専用鍵 (555–1176 Hz) は右鍵と挙動が違うが同じ表に
/// 混ざっている — 半音階配置の中間音は隣接アンカーの補間で受ける。
const FACE_GAIN_LEATHER_DB: &[(f64, f64)] = &[
    (98.00, -0.1),
    (110.00, -2.4),
    (123.47, -1.2),
    (130.81, 0.1),
    (146.83, 3.0),
    (164.81, 2.8),
    (185.00, 1.5),
    (196.00, 0.5),
    (220.00, 3.3),
    (246.94, 5.5),
    (261.63, 5.2),
    (293.66, 7.0),
    (329.63, 7.2),
    (369.99, 9.7),
    (392.00, 6.0),
    (440.00, 8.6),
    (493.88, 16.2),
    (523.25, 11.6),
    (555.07, 5.7),
    (587.33, 10.9),
    (659.26, 9.9),
    (739.99, 12.3),
    (783.99, 14.7),
    (880.99, 18.9),
    (988.88, 18.0),
    (1109.89, 15.7),
    (1175.98, 12.5),
];

/// 撥の面のラウドネス補償 [dB] — フェルト面。[`FACE_GAIN_LEATHER_DB`] 参照。
const FACE_GAIN_FELT_DB: &[(f64, f64)] = &[
    (98.00, -0.4),
    (110.00, -3.0),
    (123.47, -1.7),
    (130.81, -0.4),
    (146.83, 2.7),
    (164.81, 2.6),
    (185.00, 1.3),
    (196.00, 0.3),
    (220.00, 3.4),
    (246.94, 5.5),
    (261.63, 7.2),
    (293.66, 7.1),
    (329.63, 6.7),
    (369.99, 10.5),
    (392.00, 9.9),
    (440.00, 12.2),
    (493.88, 16.2),
    (523.25, 11.9),
    (555.07, 12.4),
    (587.33, 17.5),
    (659.26, 19.4),
    (739.99, 18.3),
    (783.99, 19.7),
    (880.99, 18.3),
    (988.88, 17.0),
    (1109.89, 12.0),
    (1175.98, 10.3),
];

/// 撥の面のラウドネス補償ゲイン (線形)。**打った区間の f0** で引く。
///
/// 木は 1.0 (基準)。レザー・フェルトは校正表の補間。励振に掛かるだけで、
/// 撥と弦の接触の動力学 (音色) には触れない — 実装は [`crate::segment`] の
/// 打撃ゲイン (撥が見る弦変位を 1/g して注入を g 倍する等価変換)。
pub fn face_gain(face: crate::hammer::HammerFace, struck_f0_hz: f64) -> f64 {
    use crate::hammer::HammerFace;
    let db = match face {
        HammerFace::Wood => return 1.0,
        HammerFace::Leather => interp_db_anchors(FACE_GAIN_LEATHER_DB, struck_f0_hz),
        HammerFace::Felt => interp_db_anchors(FACE_GAIN_FELT_DB, struck_f0_hz),
    };
    10.0f64.powf(db / 20.0)
}

/// コースの基音に応じた出力ゲイン (線形、音域バランス用)。**15/14 の校正。**
///
/// アンカー間は (log2 f, dB) 平面の折れ線、範囲外は端の値で一定。
/// トレブルコースは**右区間の f0** で引くこと — 左区間は同じ弦で同時に
/// 鳴るので、コースに 1 つのゲインしか持てない (左専用鍵 A5〜D6 も
/// 右側 f0 のゲインを受ける。実測でこの折衷は ±1.5 LU に収まっている)。
///
/// 音色には触れない: 励振・結合・減衰はそのままで、ブリッジ出力の
/// 振幅だけが変わる。配置を指定する場合は [`course_gain_for`]。
pub fn course_gain(course_f0_hz: f64) -> f64 {
    10.0f64.powf(interp_db_anchors(COURSE_GAIN_ANCHORS_DB, course_f0_hz) / 20.0)
}

/// 配置ごとのコース出力ゲイン (線形)。
///
/// 半音階は 15/14 の校正表に [`CHROMATIC_GAIN_CORRECTION_DB`] を重ねる
/// (D-022)。どちらの配置も 440 Hz は 0 dB (A4 の校正は不動)。
pub fn course_gain_for(layout: crate::layout::LayoutKind, course_f0_hz: f64) -> f64 {
    let base_db = interp_db_anchors(COURSE_GAIN_ANCHORS_DB, course_f0_hz);
    let db = match layout {
        crate::layout::LayoutKind::Diatonic1514 => base_db,
        crate::layout::LayoutKind::ChromaticE3E6 => {
            base_db + interp_db_anchors(CHROMATIC_GAIN_CORRECTION_DB, course_f0_hz)
        }
    };
    10.0f64.powf(db / 20.0)
}

/// 1 つの発音位置の設計 (弦 + 減衰)。**15/14 標準配置・純正5度**の既定文脈。
///
/// 別配置・別音律は [`design_position_with`] を使うこと。既定文脈と違う
/// 配置の `Position` を渡すと設計がずれる (course 数の補間が合わない)。
pub fn design_position(position: &Position) -> (StringDesign, DampingParams) {
    design_position_with(position, &DesignContext::default())
}

/// 1 つの発音位置の設計 (弦 + 減衰)。文脈 (配置由来の数値 + 音律) 指定版。
///
/// 音高は `Position` の表から取る: 右側は `midi` そのもの、左側は
/// **常に右 = `midi − 7`** (配置表の構成則。共有弦なので左の周波数は
/// 分割比から導かれ、`midi` は 12 平均律の最近傍でしかない)。
pub fn design_position_with(
    position: &Position,
    ctx: &DesignContext,
) -> (StringDesign, DampingParams) {
    // 音域位置 t (0–1): 線径の補間に使う。
    let register = |midi: u8| -> f64 {
        (midi as f64 - ctx.key_min as f64) / (ctx.key_max as f64 - ctx.key_min as f64)
    };
    match position.side {
        BridgeSide::Bass => {
            let t = position.course as f64 / (ctx.bass_courses - 1) as f64;
            let speaking = lerp(ctx.bass_speaking_bottom_m, ctx.bass_speaking_top_m, t);
            let f0 = key_to_hz(position.midi);
            let design = StringDesign {
                speaking_m: speaking,
                f0_hz: f0,
                diameter_m: diameter_at(register(position.midi)),
                wrap: wrap_for(speaking, f0),
            };
            (design, damping_for_course(f0))
        }
        BridgeSide::TrebleRight | BridgeSide::TrebleLeft => {
            let t = position.course as f64 / (ctx.treble_courses - 1) as f64;
            let total = lerp(ctx.treble_total_bottom_m, ctx.treble_total_top_m, t);
            let share = ctx.temperament.treble_long_share();
            // 右側 (長い側) の設計が弦を決める。
            let right_midi = match position.side {
                BridgeSide::TrebleRight => position.midi,
                _ => position.midi - 7,
            };
            let f_right = key_to_hz(right_midi);
            let l_right = total * share;
            let diameter = diameter_at(register(right_midi));
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
                    // 左側: 同じ弦の短い区間。周波数は分割比から物理的に導かれる
                    // (PureFifth: ×1.5 = +2 cent / Equal12: ×2^(7/12) = 0 cent)。
                    StringDesign {
                        speaking_m: total * (1.0 - share),
                        f0_hz: f_right * ctx.temperament.fifth_ratio(),
                        diameter_m: diameter,
                        wrap,
                    }
                }
            };
            (design, damping)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{BridgeSide, Layout};
    use approx::assert_relative_eq;

    #[test]
    fn course_gain_is_unity_at_a4() {
        // 440 Hz = 0 dB の固定。ここが動くと A4 の校正 (CALIBRATED_GAIN と
        // a_ff_single_note_is_calibrated) が崩れる。
        assert_relative_eq!(course_gain(440.0), 1.0, epsilon = 1e-12);
    }

    #[test]
    fn course_gain_follows_the_anchor_table() {
        // アンカー点そのものは表の値、間は log2(f) の折れ線。
        for &(f, db) in COURSE_GAIN_ANCHORS_DB {
            assert_relative_eq!(20.0 * course_gain(f).log10(), db, epsilon = 1e-9);
        }
        // 128–165 Hz の中点 (log2 で) は 6.5 と 8.0 の中間。
        let mid = (128.0f64 * 165.0).sqrt();
        assert_relative_eq!(20.0 * course_gain(mid).log10(), 7.25, epsilon = 1e-9);
    }

    #[test]
    fn face_gain_is_unity_for_wood() {
        // 木は基準。どの音域でも補償しない。
        use crate::hammer::HammerFace;
        for f in [98.0, 440.0, 1176.0, 2000.0] {
            assert_relative_eq!(face_gain(HammerFace::Wood, f), 1.0, epsilon = 1e-12);
        }
    }

    #[test]
    fn face_gain_follows_the_anchor_tables() {
        // 校正の固定 (D-026)。アンカー点そのものは表の値。
        use crate::hammer::HammerFace;
        for &(f, db) in FACE_GAIN_LEATHER_DB {
            assert_relative_eq!(
                20.0 * face_gain(HammerFace::Leather, f).log10(),
                db,
                epsilon = 1e-9
            );
        }
        for &(f, db) in FACE_GAIN_FELT_DB {
            assert_relative_eq!(
                20.0 * face_gain(HammerFace::Felt, f).log10(),
                db,
                epsilon = 1e-9
            );
        }
        // 範囲外は端の値で一定 (半音階の下の拡張は掃引後に置き直す)。
        assert_relative_eq!(
            face_gain(HammerFace::Felt, 50.0),
            face_gain(HammerFace::Felt, 98.0),
            epsilon = 1e-12
        );
        assert_relative_eq!(
            face_gain(HammerFace::Felt, 4000.0),
            face_gain(HammerFace::Felt, 1175.98),
            epsilon = 1e-12
        );
    }

    #[test]
    fn course_gain_clamps_outside_the_anchor_range() {
        assert_relative_eq!(course_gain(50.0), course_gain(98.0), epsilon = 1e-12);
        assert_relative_eq!(course_gain(2_000.0), course_gain(784.0), epsilon = 1e-12);
    }

    /// **P3 の完了条件**: 導いた設計が文献の範囲に入ること。
    /// 参照音源を持たないプロジェクトでは、これが唯一の外部基準になる。
    fn assert_published_ranges(layout: &Layout, ctx: &DesignContext) {
        for p in layout.positions() {
            let (design, _) = design_position_with(p, ctx);
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
    fn every_string_is_within_published_ranges() {
        let layout = Layout::standard_15_14();
        let ctx = DesignContext::for_layout(&layout, Temperament::PureFifth);
        assert_published_ranges(&layout, &ctx);
    }

    #[test]
    fn chromatic_strings_are_within_published_ranges() {
        // P7: 半音階配置も同じ文献範囲に入ること (ジオメトリ設計値の外部基準)。
        let layout = Layout::chromatic_e3_e6();
        let ctx = DesignContext::for_layout(&layout, Temperament::PureFifth);
        assert_published_ranges(&layout, &ctx);
    }

    #[test]
    fn equal_temperament_puts_the_left_side_on_the_et_fifth() {
        // P7: ブリッジを動かすと左区間が平均律の完全5度 (700 cent) に乗る。
        let layout = Layout::standard_15_14();
        let ctx = DesignContext::for_layout(&layout, Temperament::Equal12);
        for course in 0..layout.treble_courses() {
            let find = |side: BridgeSide| {
                layout
                    .positions()
                    .iter()
                    .find(|p| p.side == side && p.course == course)
                    .map(|p| design_position_with(p, &ctx).0)
                    .unwrap()
            };
            let right = find(BridgeSide::TrebleRight);
            let left = find(BridgeSide::TrebleLeft);

            // 比はちょうど 2^(7/12)、つまり左は平均律の MIDI 音高そのもの。
            assert_relative_eq!(
                left.f0_hz / right.f0_hz,
                (7.0 / 12.0f64).exp2(),
                epsilon = 1e-12
            );
            // 同じ弦なので張力は一致したまま (物理の整合性)。
            assert_relative_eq!(
                right.segment_params().tension(),
                left.segment_params().tension(),
                max_relative = 1e-9
            );
        }
        // 平均律からのずれは 0 cent (左専用鍵が平均律に乗る)。
        let p = layout
            .positions()
            .iter()
            .find(|p| p.side == BridgeSide::TrebleLeft && p.course == 0)
            .unwrap();
        let (design, _) = design_position_with(p, &ctx);
        let cents = 1200.0 * (design.f0_hz / key_to_hz(p.midi)).log2();
        assert!(cents.abs() < 0.01, "平均律で {cents:.3} cent ずれている");
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
    fn the_temperament_shares_are_consistent() {
        // PureFifth はちょうど 2:3 (S = 0.6)、Equal12 は S/(1−S) = 2^(7/12)。
        assert_relative_eq!(
            Temperament::PureFifth.treble_long_share(),
            0.6,
            epsilon = 1e-12
        );
        let s = Temperament::Equal12.treble_long_share();
        assert_relative_eq!(s / (1.0 - s), (7.0 / 12.0f64).exp2(), epsilon = 1e-12);
        // ブリッジの移動量はごく僅か (826 mm の弦で 1 mm 未満)。
        assert!((0.6 - s).abs() * 0.826 < 1e-3);
    }
}
