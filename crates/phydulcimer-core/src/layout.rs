//! 15/14 の配置表 — どの発音位置がどの音高か。
//!
//! アメリカ式ハンマーダルシマーの標準的な「5度間隔チューニング」:
//!
//! - **バスブリッジ 14 コース**: G メジャースケール、G2 から (G2–F#4)
//! - **トレブルブリッジ右側 15 コース**: G メジャースケール、G3 から (G3–G5)
//! - **トレブルブリッジ左側**: 右側と同じ弦の短い区間。ブリッジが弦長を 2:3 に
//!   分けるので**ちょうど完全5度上** (D4–D6、D メジャースケールになる)
//!
//! 発音位置は 14 + 15 + 15 = **44**。音高の種類は重複を除くと 27 で、
//! 同じ音高が最大 3 箇所にある (例: D4 はバス・トレブル右・トレブル左の全部)。
//!
//! # 半音の欠落は再現する
//!
//! 全音階配置なので、**楽器に無い半音 (G#, Bb, D#, F など) は鳴らない**。
//! MIDI の未マップ鍵は無音になる。これは実機の性質そのもので、埋めるための
//! 仮想弦は作らない (→ `docs/problems.md` の D-017)。
//! 例外的に C# だけは C#5 と C#6 がトレブル左側に存在する (D メジャーの導音)。
//!
//! # 出典について
//!
//! 15/14 の音の並びは製作者により差がある。ここでの表は Dusty Strings 系の
//! 資料にある標準的な 5度間隔チューニングの**代表値**で、特定の個体ではない。
//! 表駆動なので、別の配置 (ツィンバロム等) はこの表の差し替えで乗る。

/// ブリッジと側。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeSide {
    /// バスブリッジ (長い側だけが発音する)
    Bass,
    /// トレブルブリッジの右側 (長い側、低い音)
    TrebleRight,
    /// トレブルブリッジの左側 (短い側、5度上)
    TrebleLeft,
}

/// 1 つの発音位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub side: BridgeSide,
    /// コース番号 (0 = 最低音側)
    pub course: usize,
    /// 割り当てる MIDI ノート番号 (12 平均律の最近傍)
    pub midi: u8,
}

/// バスブリッジのコース数 (**15/14 標準配置の値**。配置ごとの値は
/// [`Layout::bass_courses`] を使うこと)。
pub const BASS_COURSES: usize = 14;
/// トレブルブリッジのコース数 (**15/14 標準配置の値**)。
pub const TREBLE_COURSES: usize = 15;
/// 発音位置の総数 (**15/14 標準配置の値**)。
pub const POSITION_COUNT: usize = BASS_COURSES + TREBLE_COURSES * 2;

/// 配置の種類 (Phase 7)。
///
/// 弦バンクの構築時に決まり、途中では切り替えられない (プラグインでは
/// activate 時に適用する — 弦バンクの再構築 = 確保を伴うため)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutKind {
    /// 15/14 全音階 (5度間隔チューニング)。44 位置・27 音高、G2–D6
    #[default]
    Diatonic1514,
    /// E3–E6 半音階 37 音 (クロマチックダルシマー)。48 位置
    ChromaticE3E6,
}

/// メジャースケールの隣接音程 [半音]。
const MAJOR_STEPS: [u8; 7] = [2, 2, 1, 2, 2, 2, 1];

/// `root` からメジャースケールを `count` 音ぶん並べる。
fn diatonic_from(root: u8, count: usize) -> Vec<u8> {
    let mut keys = Vec::with_capacity(count);
    let mut key = root;
    for i in 0..count {
        keys.push(key);
        key += MAJOR_STEPS[i % 7];
    }
    keys
}

/// 配置表。
///
/// 位置の並び順は **バス → トレブル右 → トレブル左** (コース昇順)。
/// [`Instrument`](crate::instrument::Instrument) はこの順で弦 (区間) を持つ。
#[derive(Debug, Clone)]
pub struct Layout {
    kind: LayoutKind,
    positions: Vec<Position>,
    /// MIDI 鍵 → 優先する位置の添字。未マップは `None`
    preferred: [Option<u16>; 128],
    bass_courses: usize,
    treble_courses: usize,
    key_min: u8,
    key_max: u8,
}

impl Layout {
    /// バス・トレブル右の音列から配置表を組む。左は常に右 +7 半音
    /// (同じ弦の短い区間 — 共有弦の物理制約。周波数の精密値は
    /// `scaling` の分割比が決める)。
    fn from_tables(kind: LayoutKind, bass_keys: &[u8], right_keys: &[u8]) -> Self {
        let mut positions = Vec::with_capacity(bass_keys.len() + right_keys.len() * 2);
        for (course, &midi) in bass_keys.iter().enumerate() {
            positions.push(Position {
                side: BridgeSide::Bass,
                course,
                midi,
            });
        }
        for (course, &midi) in right_keys.iter().enumerate() {
            positions.push(Position {
                side: BridgeSide::TrebleRight,
                course,
                midi,
            });
        }
        for (course, &midi) in right_keys.iter().enumerate() {
            positions.push(Position {
                side: BridgeSide::TrebleLeft,
                course,
                midi: midi + 7,
            });
        }

        // 優先順位: バス → トレブル右 → トレブル左。
        // 「最も長い (低い) 区間を選ぶ」という既定。positions の並びが
        // ちょうどこの順なので、**先勝ち**でよい。
        let mut preferred = [None; 128];
        let (mut key_min, mut key_max) = (u8::MAX, u8::MIN);
        for (i, p) in positions.iter().enumerate() {
            let slot = &mut preferred[p.midi as usize];
            if slot.is_none() {
                *slot = Some(i as u16);
            }
            key_min = key_min.min(p.midi);
            key_max = key_max.max(p.midi);
        }

        Self {
            kind,
            positions,
            preferred,
            bass_courses: bass_keys.len(),
            treble_courses: right_keys.len(),
            key_min,
            key_max,
        }
    }

    /// 種類から配置表を作る。
    pub fn of(kind: LayoutKind) -> Self {
        match kind {
            LayoutKind::Diatonic1514 => Self::standard_15_14(),
            LayoutKind::ChromaticE3E6 => Self::chromatic_e3_e6(),
        }
    }

    /// 15/14 の標準配置 (全音階)。
    ///
    /// - バス: G2 (43) から G メジャー 14 音 → F#4 (66)
    /// - トレブル右: G3 (55) から G メジャー 15 音 → G5 (79)
    /// - トレブル左: +7 半音 → D4 (62)–D6 (86)
    pub fn standard_15_14() -> Self {
        Self::from_tables(
            LayoutKind::Diatonic1514,
            &diatonic_from(43, BASS_COURSES),
            &diatonic_from(55, TREBLE_COURSES),
        )
    }

    /// E3–E6 の半音階配置 (37 音、クロマチックダルシマー)。
    ///
    /// - バス: E3 (52)–D#4 (63) を半音間隔 12 コース
    /// - トレブル右: E4 (64)–A5 (81) を半音間隔 18 コース
    /// - トレブル左: +7 半音 → B4 (71)–E6 (88)
    ///
    /// **半音列の +7 半音は半音列のまま**なので、共有弦の制約 (左 = 右の
    /// 5 度上) を保ったまま E3–E6 の 37 音が隙間なく埋まる。重複域
    /// (B4–A5) は先勝ち規則で右側 (長い区間) が優先される。
    ///
    /// 実機のクロマチック (Dusty Strings Chromatic 系) はブリッジの追加で
    /// 半音を得るが、本モデルは plan.html §配置のとおり「同じ設計則で
    /// 3 バンクに割り付ける」理想化 (D-017 の解消)。
    pub fn chromatic_e3_e6() -> Self {
        let bass: Vec<u8> = (52..=63).collect();
        let right: Vec<u8> = (64..=81).collect();
        Self::from_tables(LayoutKind::ChromaticE3E6, &bass, &right)
    }

    /// 配置の種類。
    pub fn kind(&self) -> LayoutKind {
        self.kind
    }

    /// 全発音位置。
    pub fn positions(&self) -> &[Position] {
        &self.positions
    }

    /// バスブリッジのコース数。
    pub fn bass_courses(&self) -> usize {
        self.bass_courses
    }

    /// トレブルブリッジのコース数。
    pub fn treble_courses(&self) -> usize {
        self.treble_courses
    }

    /// マップされている最低の鍵。
    pub fn key_min(&self) -> u8 {
        self.key_min
    }

    /// マップされている最高の鍵。
    pub fn key_max(&self) -> u8 {
        self.key_max
    }

    /// MIDI 鍵に対する優先位置の添字。未マップなら `None`。
    pub fn preferred_index(&self, key: u8) -> Option<usize> {
        self.preferred.get(key as usize)?.map(|i| i as usize)
    }

    /// この鍵が楽器に存在するか。
    pub fn is_mapped(&self, key: u8) -> bool {
        self.preferred_index(key).is_some()
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::standard_15_14()
    }
}

/// MIDI ノート番号 → 音名 (デバッグ・設計表の表示用)。
pub fn note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = (midi as i32) / 12 - 1;
    format!("{}{}", NAMES[(midi % 12) as usize], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_layout_has_44_positions() {
        let l = Layout::standard_15_14();
        assert_eq!(l.positions().len(), POSITION_COUNT);
        assert_eq!(POSITION_COUNT, 44);
    }

    #[test]
    fn ranges_match_the_instrument() {
        let l = Layout::standard_15_14();
        let bass: Vec<_> = l
            .positions()
            .iter()
            .filter(|p| p.side == BridgeSide::Bass)
            .collect();
        let right: Vec<_> = l
            .positions()
            .iter()
            .filter(|p| p.side == BridgeSide::TrebleRight)
            .collect();
        let left: Vec<_> = l
            .positions()
            .iter()
            .filter(|p| p.side == BridgeSide::TrebleLeft)
            .collect();

        // バス G2–F#4、トレブル右 G3–G5、トレブル左 D4–D6。
        assert_eq!((bass[0].midi, bass[13].midi), (43, 66));
        assert_eq!((right[0].midi, right[14].midi), (55, 79));
        assert_eq!((left[0].midi, left[14].midi), (62, 86));
    }

    #[test]
    fn duplicates_prefer_the_longest_segment() {
        let l = Layout::standard_15_14();
        // D4 (62) は 3 箇所にあるが、優先はバス (添字 0–13 の範囲)。
        let idx = l.preferred_index(62).unwrap();
        assert_eq!(l.positions()[idx].side, BridgeSide::Bass);
        // A4 (69) はバスに無いのでトレブル右。
        let idx = l.preferred_index(69).unwrap();
        assert_eq!(l.positions()[idx].side, BridgeSide::TrebleRight);
        // C#6 (85) はトレブル左にしか無い。
        let idx = l.preferred_index(85).unwrap();
        assert_eq!(l.positions()[idx].side, BridgeSide::TrebleLeft);
    }

    #[test]
    fn accidentals_are_absent_except_c_sharp() {
        let l = Layout::standard_15_14();
        // 全音階配置なので、これらの半音は楽器に存在しない。
        for key in [44u8, 46, 49, 51, 53, 56, 58, 61, 63, 65, 68, 70, 75, 77, 84] {
            assert!(!l.is_mapped(key), "{} は無いはず", note_name(key));
        }
        // D メジャーの導音 C# だけは左側に存在する。
        assert!(l.is_mapped(73), "C#5 はトレブル左にある");
        assert!(l.is_mapped(85), "C#6 はトレブル左にある");
        // 範囲外。
        assert!(!l.is_mapped(42));
        assert!(!l.is_mapped(87));
    }

    #[test]
    fn every_position_is_reachable_or_shadowed_consistently() {
        let l = Layout::standard_15_14();
        // すべての音高について、優先位置は必ず「その音高を持つ位置の中で最初」。
        for p in l.positions() {
            let idx = l.preferred_index(p.midi).expect("マップされていること");
            let chosen = l.positions()[idx];
            assert_eq!(chosen.midi, p.midi);
        }
    }

    #[test]
    fn the_chromatic_layout_covers_e3_to_e6_without_gaps() {
        // P7: 半音階配置。E3 (52)–E6 (88) の 37 音がすべて鳴る。
        let l = Layout::chromatic_e3_e6();
        assert_eq!(l.positions().len(), 48);
        assert_eq!((l.key_min(), l.key_max()), (52, 88));
        for key in 52..=88u8 {
            assert!(l.is_mapped(key), "{} が無い", note_name(key));
        }
        // 範囲外は無音。
        assert!(!l.is_mapped(51));
        assert!(!l.is_mapped(89));
    }

    #[test]
    fn the_chromatic_left_side_is_seven_semitones_above_the_right() {
        // 共有弦の制約: 左は常に右 +7 半音 (半音列なので左も半音列になる)。
        let l = Layout::chromatic_e3_e6();
        for course in 0..l.treble_courses() {
            let find = |side: BridgeSide| {
                l.positions()
                    .iter()
                    .find(|p| p.side == side && p.course == course)
                    .unwrap()
                    .midi
            };
            assert_eq!(
                find(BridgeSide::TrebleLeft),
                find(BridgeSide::TrebleRight) + 7
            );
        }
    }

    #[test]
    fn chromatic_duplicates_prefer_the_longest_segment() {
        // 重複域 B4–A5 (71–81) は右側 (長い区間) が優先。
        let l = Layout::chromatic_e3_e6();
        for key in 71..=81u8 {
            let p = l.positions()[l.preferred_index(key).unwrap()];
            assert_eq!(p.side, BridgeSide::TrebleRight, "{}", note_name(key));
        }
        // A#5 (82) 以上は左にしか無い。
        for key in 82..=88u8 {
            let p = l.positions()[l.preferred_index(key).unwrap()];
            assert_eq!(p.side, BridgeSide::TrebleLeft, "{}", note_name(key));
        }
    }

    #[test]
    fn layout_of_matches_the_constructors() {
        assert_eq!(Layout::of(LayoutKind::Diatonic1514).positions().len(), 44);
        assert_eq!(Layout::of(LayoutKind::ChromaticE3E6).positions().len(), 48);
        // 15/14 のフィールド化した値が既存の定数と一致する。
        let d = Layout::standard_15_14();
        assert_eq!(d.bass_courses(), BASS_COURSES);
        assert_eq!(d.treble_courses(), TREBLE_COURSES);
        assert_eq!((d.key_min(), d.key_max()), (43, 86));
    }

    #[test]
    fn note_names_render() {
        assert_eq!(note_name(43), "G2");
        assert_eq!(note_name(62), "D4");
        assert_eq!(note_name(73), "C#5");
        assert_eq!(note_name(86), "D6");
    }
}
