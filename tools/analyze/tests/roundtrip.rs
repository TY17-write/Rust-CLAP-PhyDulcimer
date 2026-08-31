//! WAV を書いて読み直し、設計値どおりに測れることを確かめる統合テスト。
//!
//! **これが Phase 0 の完了条件そのもの。** `render` → WAV → `analyze` の経路の
//! どこかで標本が壊れたり、サンプリング周波数が失われたりすれば、以降の
//! フェーズで測る値がすべて信用できなくなる。
//!
//! 信号はここで直接合成する。`phydulcimer-core` の発振器を使うと、コアの
//! バグと解析器のバグが切り分けられなくなるため。

use std::path::PathBuf;

use approx::assert_relative_eq;
use phydulcimer_analyze::{
    estimate_fundamental, estimate_partial_t60, estimate_t60, find_partial, goertzel_magnitude,
    read_wav, write_wav, write_wav_mono,
};

const SR: f64 = 48_000.0;

/// テストごとに固有の一時パス。並列実行で衝突しないよう名前を分ける。
fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("phydulcimer-test-{name}.wav"));
    p
}

/// 指数減衰する正弦波。解析解が既知の参照信号。
fn decaying_sine(freq: f64, t60: f64, amp: f64, len: usize) -> Vec<f32> {
    let decay = if t60.is_finite() && t60 > 0.0 {
        (-3.0 * std::f64::consts::LN_10 / (t60 * SR)).exp()
    } else {
        1.0
    };
    let mut a = amp;
    (0..len)
        .map(|i| {
            let s = a * (std::f64::consts::TAU * freq * i as f64 / SR).sin();
            a *= decay;
            s as f32
        })
        .collect()
}

#[test]
fn samples_survive_the_round_trip_exactly() {
    // 32-bit float で書くので、丸めは一切入らない。1 ビットでも変わったら
    // どこかで型変換か正規化が混ざっている。
    let path = temp_path("exact");
    let x = decaying_sine(440.0, 1.0, 0.5, 4_800);

    write_wav_mono(&path, &x, SR).expect("書けること");
    let wav = read_wav(&path).expect("読めること");

    assert_eq!(wav.sample_rate, SR);
    assert_eq!(wav.channel_count(), 1);
    assert_eq!(wav.frames(), x.len());
    assert_eq!(wav.channels[0], x);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn frequency_and_t60_are_recovered_from_the_file() {
    // Phase 0 の完了条件: 書いた WAV の周波数と T60 が設計値どおりに読めること。
    let path = temp_path("design");
    let (freq, t60) = (440.0, 2.0);
    let x = decaying_sine(freq, t60, 0.5, (SR * 3.0) as usize);

    write_wav_mono(&path, &x, SR).expect("書けること");
    let wav = read_wav(&path).expect("読めること");
    let y = wav.mono();

    let f0 = estimate_fundamental(&y, wav.sample_rate, 20.0, 5_000.0).expect("f0 を推定できること");
    assert_relative_eq!(f0, freq, max_relative = 0.01);

    let est = estimate_t60(&y, wav.sample_rate).expect("T60 を推定できること");
    assert_relative_eq!(est.t60_sec, t60, max_relative = 0.05);
    assert!(est.r_squared > 0.99, "R^2 = {}", est.r_squared);

    // 振幅も保たれている (Goertzel は入力振幅と同じスケールを返す)。
    let mag = goertzel_magnitude(&y[..(SR * 0.2) as usize], wav.sample_rate, freq);
    assert!(mag > 0.3 && mag < 0.55, "振幅 {mag} が想定外");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn stereo_channels_do_not_swap() {
    // Phase 6 の ROOM では L/R の差そのものを測る。入れ替わりや取り違えは
    // 「X-Y の定位が反転している」として現れ、原因の特定が難しい。ここで固定する。
    let path = temp_path("stereo");
    let left = decaying_sine(440.0, 1.0, 0.5, 2_400);
    let right = decaying_sine(660.0, 1.0, 0.25, 2_400);

    write_wav(&path, &[left.clone(), right.clone()], SR).expect("書けること");
    let wav = read_wav(&path).expect("読めること");

    assert_eq!(wav.channel_count(), 2);
    assert_eq!(wav.frames(), 2_400);
    assert_eq!(wav.channels[0], left);
    assert_eq!(wav.channels[1], right);

    // 各チャンネルには自分の周波数だけがある。
    assert!(goertzel_magnitude(&wav.channels[0], SR, 440.0) > 0.3);
    assert!(goertzel_magnitude(&wav.channels[0], SR, 660.0) < 0.01);
    assert!(goertzel_magnitude(&wav.channels[1], SR, 660.0) > 0.15);
    assert!(goertzel_magnitude(&wav.channels[1], SR, 440.0) < 0.01);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn mono_is_the_average_not_the_sum() {
    // ここを取り違えると X-Y のモノ互換性が 6 dB ずれて見える。
    let path = temp_path("mono-average");
    let a = vec![1.0f32; 128];
    let b = vec![0.0f32; 128];

    write_wav(&path, &[a, b], SR).expect("書けること");
    let wav = read_wav(&path).expect("読めること");

    assert_relative_eq!(wav.mono()[0], 0.5, epsilon = 1e-9);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn partials_and_their_decay_survive_the_round_trip() {
    // 部分音ごとに違う減衰時間を与え、ファイル経由でも独立に測れることを見る。
    // 減衰設計 (Phase 1 以降) を検証する能力がここで担保される。
    let path = temp_path("partials");
    let len = (SR * 4.0) as usize;
    let mut x = vec![0.0f32; len];
    for (freq, t60, amp) in [(220.0, 3.0, 0.5), (440.0, 1.5, 0.25), (880.0, 0.75, 0.125)] {
        for (dst, src) in x.iter_mut().zip(decaying_sine(freq, t60, amp, len)) {
            *dst += src;
        }
    }

    write_wav_mono(&path, &x, SR).expect("書けること");
    let wav = read_wav(&path).expect("読めること");
    let y = wav.mono();

    for (n, expected_t60) in [(1usize, 3.0), (2, 1.5), (4, 0.75)] {
        let p = find_partial(&y, SR, 220.0, n, 50.0).expect("部分音が見つかること");
        assert_relative_eq!(p.freq_hz, 220.0 * n as f64, max_relative = 5e-3);

        let est = estimate_partial_t60(&y, SR, p.freq_hz).expect("部分音の T60 が測れること");
        assert_relative_eq!(est.t60_sec, expected_t60, max_relative = 0.1);
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn writing_refuses_ragged_channels() {
    let path = temp_path("ragged");
    let err = write_wav(&path, &[vec![0.0; 10], vec![0.0; 11]], SR).unwrap_err();
    assert!(err.contains("長さ"), "{err}");
}

#[test]
fn writing_refuses_no_channels() {
    let path = temp_path("empty-channels");
    assert!(write_wav(&path, &[], SR).is_err());
}

#[test]
fn reading_a_missing_file_is_an_error() {
    assert!(read_wav(&temp_path("does-not-exist-at-all")).is_err());
}
