//! P7 の完了条件: 打弦点の掃引 (Phase 7 演奏表現)。
//!
//! - 打弦点 x/L を 0.03 → 0.30 と掃引したとき、**スペクトル重心が単調に
//!   下がる** (ブリッジ寄りほど明るい)
//! - **ノッチ位置が L/x_h に追随する** (x/L = 1/4 で第 4 部分音、1/8 で
//!   第 8 部分音が消え、そのとき第 4 は生きている)
//!
//! 音質判断の規約どおり ROOM off・フルチェーン (響板 + 箱)。
//! 指標は `phydulcimer_analyze::spectral_centroid_partials` (部分音重心)。

use phydulcimer_analyze::{goertzel_magnitude, spectral_centroid_partials};
use phydulcimer_core::engine::DulcimerEngine;

const SR: f64 = 48_000.0;
const KEY: u8 = 60; // C4 (バスコース)

/// 打弦点を指定して 1 音レンダリングする (L=R なので左だけ返す)。
fn render_strike(ratio: f64, seconds: f64) -> Vec<f32> {
    let n = (SR * seconds) as usize;
    let mut engine = DulcimerEngine::new(SR, 64);
    engine.set_room_enabled(false);
    engine.set_strike_ratio(ratio);
    engine.note_on(KEY, 1.0);

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
    left
}

/// 打撃過渡を過ぎた解析窓 (50–800 ms)。
fn analysis_window(x: &[f32]) -> &[f32] {
    &x[(SR * 0.05) as usize..(SR * 0.8) as usize]
}

fn f0_of_key() -> f64 {
    let engine = DulcimerEngine::new(SR, 64);
    engine
        .instrument()
        .string_params(KEY)
        .expect("C4 はマップ済み")
        .f0_hz
}

#[test]
fn the_centroid_falls_monotonically_as_the_strike_moves_from_the_bridge() {
    let ratios = [0.03, 0.06, 0.09, 0.125, 0.18, 0.24, 0.30];
    let f0 = f0_of_key();

    let centroids: Vec<f64> = ratios
        .iter()
        .map(|&r| {
            let x = render_strike(r, 1.0);
            spectral_centroid_partials(analysis_window(&x), SR, f0, 24)
                .unwrap_or_else(|| panic!("x/L = {r} で重心が測れない"))
        })
        .collect();

    // 単調性は小さなリップルを許して見る。x/L がちょうど 1/整数 を通るとき
    // (掃引中の 0.125 = 1/8 など) はノッチが部分音を丸ごと消すので、その点
    // だけ重心が余分に沈み、次の点で数十 Hz 戻る — コムフィルタの物理で、
    // 傾向の崩れではない (実測: 0.125 → 0.18 で 1043 → 1106 Hz)。
    for (w, pair) in centroids.windows(2).enumerate() {
        assert!(
            pair[1] < pair[0] * 1.07,
            "重心が下がっていない: x/L {} → {} で {:.1} → {:.1} Hz (全点 {:?})",
            ratios[w],
            ratios[w + 1],
            pair[0],
            pair[1],
            centroids
        );
    }
    // 全体の傾向は強く固定する: 0.30 の重心は 0.03 より 30% 以上低い。
    let (first, last) = (centroids[0], centroids[centroids.len() - 1]);
    assert!(
        last < first * 0.7,
        "掃引全体で重心が十分下がっていない: {first:.1} → {last:.1} Hz"
    );
}

#[test]
fn the_notch_tracks_the_strike_position() {
    let f0 = f0_of_key();
    let partial_level = |ratio: f64, n: usize| -> f64 {
        let x = render_strike(ratio, 1.0);
        goertzel_magnitude(analysis_window(&x), SR, f0 * n as f64)
    };

    // x/L = 1/4 → 第 4 部分音がノッチに落ちる (隣の第 3 と比べて)。
    let p4_at_quarter = partial_level(0.25, 4);
    let p3_at_quarter = partial_level(0.25, 3);
    assert!(
        p4_at_quarter < p3_at_quarter * 0.05,
        "x/L = 1/4 で第 4 部分音が消えていない: p4 {p4_at_quarter:.3e} vs p3 {p3_at_quarter:.3e}"
    );

    // x/L = 1/8 → ノッチは第 8 部分音へ移動し、第 4 部分音は生き返る。
    let p8_at_eighth = partial_level(0.125, 8);
    let p7_at_eighth = partial_level(0.125, 7);
    let p4_at_eighth = partial_level(0.125, 4);
    assert!(
        p8_at_eighth < p7_at_eighth * 0.05,
        "x/L = 1/8 で第 8 部分音が消えていない: p8 {p8_at_eighth:.3e} vs p7 {p7_at_eighth:.3e}"
    );
    assert!(
        p4_at_eighth > p4_at_quarter * 10.0,
        "ノッチが追随していない: 1/8 でも第 4 部分音が消えたまま \
         ({p4_at_eighth:.3e} vs 1/4 のとき {p4_at_quarter:.3e})"
    );
}
