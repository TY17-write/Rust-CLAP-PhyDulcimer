//! 音域バランスの回帰テスト (Phase 10 前半)。
//!
//! 全鍵掃引 (2026-08-31、ff・ROOM off) を `scaling::course_gain` で
//! ターゲット直線に載せた校正の固定。校正前の広がりは 18.3 LU、校正後は
//! 全 27 鍵が直線 ±2.02 LU (実測は `docs/context.md`)。
//!
//! 指標は **LUFS モーメンタリ最大** (ゲート付き積分値はレンダリング長と
//! T60 に依存するので使わない — バス T60 12 s / トレブル 3 s のこの楽器では
//! 積分値は「どれだけ長く鳴るか」を測ってしまう)。

use phydulcimer_analyze::loudness;
use phydulcimer_core::engine::DulcimerEngine;

const SR: f64 = 48_000.0;

/// ターゲット直線の傾き [LU/oct]。実機の低音はやや控えめ、を緩く写す。
/// 完全フラット (0.0) は実機と違い、ベースライン (無補償) は低音が 7〜9 LU
/// 沈んでいた。掃引の実測を見て +1.0 に決めた (docs/context.md)。
const SLOPE_LU_PER_OCT: f64 = 1.0;

/// 直線からの許容ずれ [LU]。響板・箱のモード構造による鍵ごとの個性
/// (±2 LU 程度) は音色として残す — ここを 0 に締めることはしない。
const TOLERANCE_LU: f64 = 2.5;

/// 1 鍵を ff で鳴らして LUFS モーメンタリ最大を測る。
///
/// 条件は掃引と同じ: vel 1.0・ROOM off・フルチェーン (響板 + 箱)。
/// 長さはアタック近傍の 400 ms が入れば足りるので 1.5 秒で切る。
fn m_max_lufs(key: u8) -> f64 {
    m_max_lufs_with(
        key,
        phydulcimer_core::instrument::InstrumentConfig::default(),
    )
}

fn m_max_lufs_with(key: u8, config: phydulcimer_core::instrument::InstrumentConfig) -> f64 {
    let n = (SR * 1.5) as usize;
    let mut engine = DulcimerEngine::with_config(SR, 64, config);
    engine.set_room_enabled(false);
    engine.note_on(key, 1.0);

    let mut left = vec![0.0f32; n];
    let mut right = vec![0.0f32; n];
    for start in (0..n).step_by(64) {
        let end = (start + 64).min(n);
        let mut l = [0.0f32; 64];
        let mut r = [0.0f32; 64];
        let len = end - start;
        engine.process_stereo(&mut l[..len], &mut r[..len]);
        left[start..end].copy_from_slice(&l[..len]);
        right[start..end].copy_from_slice(&r[..len]);
    }

    loudness(&[left, right], SR)
        .expect("1.5 秒あれば測れるはず")
        .momentary_max_lufs
}

#[test]
fn every_register_sits_on_the_target_loudness_line() {
    // A4 (校正の基準音) を測り、そこからの直線に代表鍵が載っていること。
    // 代表鍵は掃引で残差が大きかった鍵から選ぶ:
    //   G2 (43)  最低音
    //   E3 (52)  ベースラインで最も沈んでいた鍵 (−9 LU)
    //   C4 (60)  中音域
    //   C#5 (73) 校正後の残差が最大の鍵 (+2.0 LU)、トレブル左
    //   G5 (79)  ベースラインで最も出ていた鍵 (+7 LU)
    //   D6 (86)  最高音、トレブル左専用
    let a4 = m_max_lufs(69);
    assert!(
        a4.is_finite() && a4 < 0.0,
        "A4 の測定が壊れている: {a4:.2} LUFS"
    );

    for key in [43u8, 52, 60, 73, 79, 86] {
        let octaves = (key as f64 - 69.0) / 12.0;
        let target = a4 + SLOPE_LU_PER_OCT * octaves;
        let measured = m_max_lufs(key);
        let residual = measured - target;
        assert!(
            residual.abs() <= TOLERANCE_LU,
            "key {key} が音域バランスから外れた: 実測 {measured:.2} LUFS, \
             ターゲット {target:.2} LUFS (A4 {a4:.2} + {SLOPE_LU_PER_OCT} LU/oct), \
             残差 {residual:+.2} LU (許容 ±{TOLERANCE_LU})"
        );
    }
}

#[test]
fn the_chromatic_layout_sits_on_its_own_target_line() {
    // P7 (D-022): 半音階は 15/14 の校正表 + 補正表。2026-08-31 の掃引で
    // 全 37 鍵が ±0.8 LU に載った。代表鍵で固定する:
    //   E3 (52)  最低音
    //   E4 (64)  補正前の残差が最大だった鍵 (+4.5 LU、トレブル最低コース)
    //   C5 (72)  補正前に最も沈んでいた鍵 (−4.6 LU)
    //   F#5 (78) 補正前に最も出ていた鍵 (+4.8 LU)
    //   C#6 (85) 左専用域
    //   E6 (88)  最高音、左専用
    use phydulcimer_core::instrument::InstrumentConfig;
    use phydulcimer_core::layout::LayoutKind;
    let config = InstrumentConfig {
        layout: LayoutKind::ChromaticE3E6,
        ..InstrumentConfig::default()
    };

    let a4 = m_max_lufs_with(69, config);
    assert!(a4.is_finite() && a4 < 0.0, "A4 の測定が壊れている: {a4:.2}");

    for key in [52u8, 64, 72, 78, 85, 88] {
        let octaves = (key as f64 - 69.0) / 12.0;
        let target = a4 + SLOPE_LU_PER_OCT * octaves;
        let measured = m_max_lufs_with(key, config);
        let residual = measured - target;
        assert!(
            residual.abs() <= TOLERANCE_LU,
            "半音階の key {key} が音域バランスから外れた: 実測 {measured:.2} LUFS, \
             ターゲット {target:.2} LUFS, 残差 {residual:+.2} LU (許容 ±{TOLERANCE_LU})"
        );
    }
}
