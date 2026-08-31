//! WAV 解析の実体。
//!
//! モデルの検証に必要なのは「予測した周波数に本当にエネルギーがあるか」と
//! 「設計した減衰時間で減っているか」の 2 点なので、汎用 FFT ではなく
//! 目的を絞った 3 つの推定器を置いている。
//!
//! - [`goertzel_magnitude`] — 指定周波数ちょうどの振幅。部分音は
//!   `f_n = n·f0·√(1+B·n²)` で位置が予測できるため、その点だけ測れば十分で、
//!   FFT のビン間補間より直接的で誤差が小さい。
//! - [`estimate_t60`] — 包絡の対数傾きから 60 dB 減衰時間。
//! - [`estimate_fundamental`] — 自己相関による基本周波数。
//! - [`loudness`] — BS.1770-4 のラウドネス (音域バランスの指標、Phase 10)。
//!
//! # 設計の要点
//!
//! - **bin の中の `mod analysis` ではなく lib にしてある。** 測定器そのものの
//!   正しさを `tests/` から固定するため
//! - WAV の読み書き ([`read_wav`] / [`write_wav_mono`]) をここに置いた。
//!   `render` と `analyze` と統合テストで同じ経路を使う
//!
//! # このプロジェクトでの位置づけ
//!
//! 本プロジェクトは**参照音源を持たない** (`docs/plan.html` §10)。
//! 「参照より何 dB」で判断できないぶん、**ここの測定値が設計値と合っているか**が
//! 唯一の足場になる。測定器を疑う余地を残さないよう、解析解が既知の信号に対する
//! テストを厚く持つ。

use std::path::Path;

/// 指定周波数における振幅を Goertzel 法で求める。
///
/// Hann 窓を掛けてから積分するので、隣接部分音からの漏れ込みが抑えられる。
/// 返り値は入力信号の振幅と同じスケール (正弦波 `A·sin` に対して `A`)。
///
/// `samples` が空、または `freq_hz` がナイキストを超える場合は 0 を返す。
pub fn goertzel_magnitude(samples: &[f32], sample_rate: f64, freq_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 || freq_hz <= 0.0 || freq_hz >= sample_rate * 0.5 {
        return 0.0;
    }

    let w = std::f64::consts::TAU * freq_hz / sample_rate;
    let coeff = 2.0 * w.cos();

    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    // Hann 窓のコヒーレントゲインは 0.5。これで割って振幅を戻す。
    let mut window_sum = 0.0f64;

    for (i, &x) in samples.iter().enumerate() {
        let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        window_sum += win;

        let s0 = x as f64 * win + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }

    let real = s1 - s2 * w.cos();
    let imag = s2 * w.sin();
    let magnitude = (real * real + imag * imag).sqrt();

    if window_sum <= 0.0 {
        return 0.0;
    }
    // 実正弦波は正負の周波数に半分ずつ分かれるので 2 倍する。
    2.0 * magnitude / window_sum
}

/// ブロック RMS による包絡 (振幅, 線形)。
pub fn rms_envelope(samples: &[f32], block: usize) -> Vec<f64> {
    if block == 0 {
        return Vec::new();
    }
    samples
        .chunks(block)
        .map(|c| {
            let sum: f64 = c.iter().map(|&x| (x as f64) * (x as f64)).sum();
            (sum / c.len() as f64).sqrt()
        })
        .collect()
}

/// T60 の推定結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct T60Estimate {
    /// 推定した 60 dB 減衰時間 [s]
    pub t60_sec: f64,
    /// 当てはめに使った区間 [s]
    pub fit_start_sec: f64,
    pub fit_end_sec: f64,
    /// 当てはめの決定係数 R²。1 に近いほど単一の指数減衰に近い。
    pub r_squared: f64,
}

/// dB 系列 `(時刻 [s], dB)` に直線を当てはめて T60 を求める。
///
/// ピークから `-5 dB` 落ちた点を起点、`drop_db` 落ちた点を終点として最小二乗
/// 直線を当てはめ、その傾きから 60 dB 分を外挿する (音響測定で標準的な T30 外挿)。
/// 起点を −5 dB にするのは、アタック直後の非指数的な立ち上がりを避けるため。
///
/// 終点まで落ちきらない場合は、末尾までを使って当てはめる。ダンパーの無い
/// 楽器の基音は T60 が 10 秒を超えることがあり、実用的な長さのレンダリングでは
/// −35 dB に届かないため。
fn fit_t60(points: &[(f64, f64)], drop_db: f64) -> Option<T60Estimate> {
    if points.len() < 4 {
        return None;
    }

    let peak_idx = points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.1.is_finite())
        .max_by(|a, b| {
            a.1 .1
                .partial_cmp(&b.1 .1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)?;
    let peak_db = points[peak_idx].1;

    let find_after = |threshold: f64| -> Option<usize> {
        points
            .iter()
            .enumerate()
            .skip(peak_idx)
            .find(|(_, p)| p.1 <= threshold)
            .map(|(i, _)| i)
    };

    let start = find_after(peak_db - 5.0)?;
    // 目標まで落ちなければ末尾まで使う。
    let end = find_after(peak_db - drop_db).unwrap_or(points.len() - 1);
    if end <= start + 2 {
        return None;
    }

    let pts: Vec<(f64, f64)> = points[start..=end]
        .iter()
        .copied()
        .filter(|p| p.1.is_finite())
        .collect();
    if pts.len() < 3 {
        return None;
    }

    let n = pts.len() as f64;
    let mean_x = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx; // [dB/s], 負の値
    if slope >= 0.0 {
        return None; // 減衰していない
    }

    let syy: f64 = pts.iter().map(|p| (p.1 - mean_y).powi(2)).sum();
    let r_squared = if syy > 0.0 {
        (sxy * sxy) / (sxx * syy)
    } else {
        0.0
    };

    Some(T60Estimate {
        t60_sec: -60.0 / slope,
        fit_start_sec: pts[0].0,
        fit_end_sec: pts[pts.len() - 1].0,
        r_squared,
    })
}

/// 信号全体の RMS 包絡から T60 を推定する。
///
/// # 打弦楽器に使うときの注意
/// 打弦楽器の1音は部分音ごとに減衰時間が違う (高い部分音ほど速く減衰する)。
/// 全体の包絡は「最初は高次部分音の速い減衰、後半は基音の遅い減衰」という
/// 2段階になり、単一の指数では表せない。モデルの減衰設計を検証するときは
/// [`estimate_partial_t60`] を使うこと。
///
/// **ダンパーの無いダルシマーではさらに注意が要る。** 1 音を鳴らしても
/// 他の弦が共鳴して鳴り続けるので、全体の包絡は自分の減衰を表さない。
/// この関数が意味を持つのは Phase 0 の疎通確認 (単一の減衰正弦波) までで、
/// モデルが入ったら [`estimate_partial_t60`] に切り替えること。
pub fn estimate_t60(samples: &[f32], sample_rate: f64) -> Option<T60Estimate> {
    // 10 ms ブロック。最短の減衰でも数十点は取れる粒度。
    let block = ((sample_rate * 0.01) as usize).max(1);
    let env = rms_envelope(samples, block);
    if env.len() < 4 {
        return None;
    }

    let peak = env.iter().cloned().fold(0.0f64, f64::max);
    if peak <= 0.0 {
        return None;
    }

    let dt = block as f64 / sample_rate;
    let points: Vec<(f64, f64)> = env
        .iter()
        .enumerate()
        .map(|(i, &e)| {
            let db = if e > 0.0 {
                20.0 * (e / peak).log10()
            } else {
                f64::NEG_INFINITY
            };
            (i as f64 * dt, db)
        })
        .collect();

    fit_t60(&points, 35.0)
}

/// **1本の部分音**の減衰時間を推定する (既定の窓 200 ms / ホップ 50 ms)。
///
/// この既定は **T60 が約 0.5 秒より短いと測れない** (`docs/problems.md` の D-001)。
/// `fit_t60` はピークから −5 dB → −20 dB の区間 (時間にして T60/4) に最低 3 点を
/// 要求するので、ホップ 50 ms では T60 < 0.6 s あたりから点が足りなくなる。
/// 短い減衰は [`estimate_partial_t60_with`] で窓とホップを狭めて測ること。
/// 目安は **ホップ ≤ T60/12**。
pub fn estimate_partial_t60(
    samples: &[f32],
    sample_rate: f64,
    freq_hz: f64,
) -> Option<T60Estimate> {
    estimate_partial_t60_with(samples, sample_rate, freq_hz, 0.2, 0.05)
}

/// 窓とホップを指定して部分音の減衰時間を推定する。
///
/// 短い窓ごとに Goertzel で `freq_hz` の振幅を測り、その対数傾きから T60 を出す。
/// 部分音ごとに減衰時間が異なる打弦楽器では、これがモデルの減衰設計
/// (`σ(f) = c1 + c3·f²`) を検証する唯一まともな方法になる。
///
/// # 窓とホップの選び方
///
/// - **窓** は隣接部分音を分離できる長さ (周波数分解能 ≈ 2/窓)。基音間隔が
///   `f0` なら窓 ≥ 4/f0 程度
/// - **ホップ** は減衰を追える細かさ。**T60/12 以下**にしないと当てはめの
///   点数が足りず `None` になる
///
/// 短い窓は分解能が粗くなるので、両者はトレードオフ。高音の弦 (f0 が高く
/// T60 が短い) では両方を小さくできるため、実用上は成立する。
pub fn estimate_partial_t60_with(
    samples: &[f32],
    sample_rate: f64,
    freq_hz: f64,
    window_sec: f64,
    hop_sec: f64,
) -> Option<T60Estimate> {
    let window = (sample_rate * window_sec) as usize;
    let hop = (sample_rate * hop_sec) as usize;
    if window < 16 || hop == 0 || samples.len() < window * 2 {
        return None;
    }

    let mut points = Vec::new();
    let mut start = 0;
    while start + window <= samples.len() {
        let mag = goertzel_magnitude(&samples[start..start + window], sample_rate, freq_hz);
        let db = if mag > 0.0 {
            20.0 * mag.log10()
        } else {
            f64::NEG_INFINITY
        };
        // 窓の中心時刻を代表点にする。
        points.push(((start as f64 + window as f64 * 0.5) / sample_rate, db));
        start += hop;
    }

    // 部分音は −35 dB まで落ちないことが多いので、−20 dB で妥協する。
    fit_t60(&points, 20.0)
}

/// 自己相関による基本周波数推定 [Hz]。
///
/// `min_hz`..`max_hz` の範囲で正規化自己相関を計算し、放物線補間でサンプル間の
/// 精度に上げる。
///
/// # サブオクターブ誤りへの対処
/// 周期信号の自己相関は基本周期の**整数倍**すべてでほぼ同じ高さのピークを作る。
/// 単純に最大値を採ると、丸め誤差ひとつで 2 倍・3 倍のラグが勝ち、推定周波数が
/// 1/2・1/3 に落ちる。そこで「最大値の 90% を超える最初の極大」を基本周期として
/// 採用する。ピッチ推定器で標準的な対処。
pub fn estimate_fundamental(
    samples: &[f32],
    sample_rate: f64,
    min_hz: f64,
    max_hz: f64,
) -> Option<f64> {
    if samples.len() < 64 || min_hz <= 0.0 || max_hz <= min_hz || sample_rate <= 0.0 {
        return None;
    }

    let min_lag = (sample_rate / max_hz).floor().max(1.0) as usize;
    let max_lag = (sample_rate / min_hz).ceil() as usize;
    // 最長ラグでも十分な重なりが残るよう、解析長はラグの 4 倍までに制限する。
    // 精度を落とさずに計算量を抑えられる。
    let max_lag = max_lag.min(samples.len() / 4);
    if max_lag <= min_lag + 1 {
        return None;
    }
    let x = &samples[..samples.len().min(max_lag * 4)];

    // 正規化自己相関 r(lag) ∈ [-1, 1]。エネルギーで割ることで、包絡が減衰して
    // いてもラグ間で公平に比較できる。
    let corr_at = |lag: usize| -> f64 {
        let m = x.len() - lag;
        let (mut acc, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..m {
            let a = x[i] as f64;
            let b = x[i + lag] as f64;
            acc += a * b;
            e0 += a * a;
            e1 += b * b;
        }
        let denom = (e0 * e1).sqrt();
        if denom > 0.0 {
            acc / denom
        } else {
            0.0
        }
    };

    let corr: Vec<f64> = (min_lag..=max_lag).map(corr_at).collect();
    let best = corr.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !best.is_finite() || best <= 0.0 {
        return None;
    }

    // 最大値の 90% 以上に達する最初の極大を採る。
    let threshold = best * 0.9;
    let mut peak = None;
    for i in 1..corr.len().saturating_sub(1) {
        if corr[i] >= threshold && corr[i] >= corr[i - 1] && corr[i] >= corr[i + 1] {
            peak = Some(i);
            break;
        }
    }
    // 極大が取れない (範囲の端に真のピークがある) 場合は最大値の位置に落とす。
    let i = peak.or_else(|| {
        corr.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    })?;

    // 放物線補間でピーク位置を精密化する。
    let refined = if i > 0 && i + 1 < corr.len() {
        let (y0, y1, y2) = (corr[i - 1], corr[i], corr[i + 1]);
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() > f64::EPSILON {
            (min_lag + i) as f64 + 0.5 * (y0 - y2) / denom
        } else {
            (min_lag + i) as f64
        }
    } else {
        (min_lag + i) as f64
    };

    if refined > 0.0 {
        Some(sample_rate / refined)
    } else {
        None
    }
}

/// 走査で見つけた 1 本の部分音。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoundPartial {
    /// 何番目の部分音か (1 始まり)
    pub n: usize,
    /// 実測したピークの周波数 [Hz]
    pub freq_hz: f64,
    /// そのときの振幅
    pub magnitude: f64,
    /// 整数倍 `n·f0` からのずれ [cent]
    pub cents: f64,
}

/// 第 `n` 部分音を**走査して**見つける。
///
/// # なぜ計算した位置を直接測らないか
///
/// 部分音は `f_n = n·f0·√(1+B·n²)` の位置にあるが、**B は音源ごとに違う**。
/// 想定した B が外れていると高次ほど測定点がずれ、実際には鳴っている部分音を
/// 「無い」と誤判定する (想定 B のずれで実在する部分音を −70 dB と
/// 読み違える型の事故が起きる)。
///
/// ダルシマーは弦が短く張力も低いので **B はピアノより大きくなる方向**で、
/// この走査はより効いてくる。
///
/// 整数倍の位置から上へ `up_cents` まで走査してピークを採る。剛性による
/// インハーモニシティは**必ず上方向**にずれるので、下側は少しだけ見ればよい。
pub fn find_partial(
    samples: &[f32],
    sample_rate: f64,
    f0_hz: f64,
    n: usize,
    up_cents: f64,
) -> Option<FoundPartial> {
    let center = n as f64 * f0_hz;
    if center <= 0.0 || center >= sample_rate * 0.5 {
        return None;
    }
    // 下は 20 cent だけ (測定誤差ぶん)、上は指定ぶん。
    let lo = center * (-20.0f64 / 1200.0).exp2();
    let hi = (center * (up_cents / 1200.0).exp2()).min(sample_rate * 0.49);
    if hi <= lo {
        return None;
    }

    // 1 cent 刻みで十分細かい。
    let steps = (((hi / lo).log2() * 1200.0) as usize).clamp(8, 4096);
    let mut best = (lo, 0.0f64);
    for i in 0..=steps {
        let f = lo * (hi / lo).powf(i as f64 / steps as f64);
        let m = goertzel_magnitude(samples, sample_rate, f);
        if m > best.1 {
            best = (f, m);
        }
    }

    Some(FoundPartial {
        n,
        freq_hz: best.0,
        magnitude: best.1,
        cents: 1200.0 * (best.0 / center).log2(),
    })
}

/// 部分音ベースのスペクトル重心 [Hz] (Phase 7 の打弦点指標)。
///
/// n = 1..`max_partials` の部分音を [`find_partial`] で走査し (インハーモニ
/// シティに追随)、振幅加重平均 `Σ f_n·a_n / Σ a_n` を返す。
///
/// # なぜ FFT 全帯域の重心ではないか
///
/// 減衰音の全帯域重心は、音が減った後のノイズ床とデノーマル防止 DC に
/// 引きずられて安定しない。この楽器の検証では f0 が既知 (鍵を選んで鳴らす)
/// なので、実在する部分音だけを拾う部分音重心が頑健で、ノッチの検証と
/// 同じ部分音表を共有できる。
///
/// 部分音が 1 本も見つからない場合は `None`。
pub fn spectral_centroid_partials(
    samples: &[f32],
    sample_rate: f64,
    f0_hz: f64,
    max_partials: usize,
) -> Option<f64> {
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for n in 1..=max_partials {
        let Some(p) = find_partial(samples, sample_rate, f0_hz, n, 200.0) else {
            continue;
        };
        num += p.freq_hz * p.magnitude;
        den += p.magnitude;
    }
    if den > 0.0 {
        Some(num / den)
    } else {
        None
    }
}

/// 実測した部分音の位置からインハーモニシティ係数 `B` を推定する。
///
/// `f_n = n·f0·√(1+B·n²)` を `B` について解くと `B = ((f_n/(n·f0))² − 1)/n²`。
/// 低次はずれが小さくて誤差に埋もれるので **n ≥ 4 だけ**を使い、外れ値に強い
/// ように中央値を採る。
pub fn estimate_inharmonicity(partials: &[FoundPartial]) -> Option<f64> {
    let mut estimates: Vec<f64> = partials
        .iter()
        .filter(|p| p.n >= 4 && p.magnitude > 0.0)
        .map(|p| {
            let ratio = (p.cents / 1200.0).exp2();
            (ratio * ratio - 1.0) / (p.n * p.n) as f64
        })
        .filter(|b| b.is_finite() && *b > 0.0)
        .collect();
    if estimates.is_empty() {
        return None;
    }
    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(estimates[estimates.len() / 2])
}

/// 帯域包絡の変調 (うなり) の推定結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulationEstimate {
    /// 指数減衰を除いた後の包絡の起伏 [dB] (最大 − 最小)
    pub depth_db: f64,
    /// 使った包絡の点数
    pub points: usize,
}

/// 部分音の包絡の変調深さ (うなり) を測る。
///
/// # なぜこれが要るか
///
/// うなり (ユニゾン弦のデチューン) は**部分音のピーク位置では測れない**。
/// 1 本のピークが 2 本に割れると片方しか拾わず、暗くなったように誤って見える。
/// うなりが現れるのは**時間方向の包絡の変調**だけ。
///
/// 窓ごとに Goertzel で振幅を測り、dB にして指数減衰 (直線) を最小二乗で除き、
/// 残差の起伏 (最大 − 最小) を返す。うなりが無ければ残差はほぼ平坦。
///
/// `skip_sec` は打撃の過渡を捨てる長さ。過渡はチャタリングで包絡が暴れるので、
/// 0.5 秒以上を推奨。
pub fn modulation_depth(
    samples: &[f32],
    sample_rate: f64,
    freq_hz: f64,
    window_sec: f64,
    hop_sec: f64,
    skip_sec: f64,
) -> Option<ModulationEstimate> {
    let window = (sample_rate * window_sec) as usize;
    let hop = (sample_rate * hop_sec) as usize;
    let skip = (sample_rate * skip_sec) as usize;
    if window < 16 || hop == 0 || samples.len() < skip + window * 3 {
        return None;
    }

    let mut db = Vec::new();
    let mut start = skip;
    while start + window <= samples.len() {
        let mag = goertzel_magnitude(&samples[start..start + window], sample_rate, freq_hz);
        if mag <= 0.0 {
            return None;
        }
        db.push(20.0 * mag.log10());
        start += hop;
    }
    if db.len() < 6 {
        return None;
    }

    // dB 直線 (指数減衰) を除く。
    let n = db.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = db.iter().sum::<f64>() / n;
    let sxx: f64 = (0..db.len()).map(|i| (i as f64 - mean_x).powi(2)).sum();
    let sxy: f64 = db
        .iter()
        .enumerate()
        .map(|(i, &y)| (i as f64 - mean_x) * (y - mean_y))
        .sum();
    let slope = sxy / sxx;

    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for (i, &y) in db.iter().enumerate() {
        let r = y - (mean_y + slope * (i as f64 - mean_x));
        lo = lo.min(r);
        hi = hi.max(r);
    }

    Some(ModulationEstimate {
        depth_db: hi - lo,
        points: db.len(),
    })
}

/// 1/2 オクターブバンドのレベル。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLevel {
    /// バンド中心 [Hz]
    pub center_hz: f64,
    /// バンド内の 1/12 オクターブ格子点の振幅の RMS (dB)
    pub level_db: f64,
}

/// 1/2 オクターブバンドの絶対レベルを測る (31.5 Hz 〜 16 kHz)。
///
/// 響板のインパルス応答 (`render --soundboard`) の校正用。バンド内に
/// 1/12 オクターブ間隔で Goertzel を置き、振幅の RMS を取る。
/// 個々のモードの山谷 (20–30 dB 振れる) を均して帯域の傾向を読むための道具で、
/// **1 本ずつのピークで判断しないこと**。
pub fn band_levels(samples: &[f32], sample_rate: f64) -> Vec<BandLevel> {
    let mut out = Vec::new();
    let mut center = 31.5;
    while center < 17_000.0 && center < sample_rate * 0.45 {
        let lo = center / 2.0f64.powf(0.25);
        let hi = center * 2.0f64.powf(0.25);
        // 1/12 オクターブ刻み = 1/2 オクターブバンドに 6 点。
        let mut acc = 0.0;
        let mut n = 0;
        let mut f = lo;
        while f < hi {
            let m = goertzel_magnitude(samples, sample_rate, f);
            acc += m * m;
            n += 1;
            f *= 2.0f64.powf(1.0 / 12.0);
        }
        if n > 0 {
            let rms = (acc / n as f64).sqrt();
            out.push(BandLevel {
                center_hz: center,
                level_db: to_db(rms),
            });
        }
        center *= 2.0f64.sqrt();
    }
    out
}

/// L/R の相関の推定結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrelationEstimate {
    /// ラグ 0 の正規化相関係数
    pub at_zero: f64,
    /// 最大の相関係数
    pub best: f64,
    /// 最大が現れたラグ [サンプル]。X-Y なら 0
    pub best_lag: i64,
}

/// L/R の正規化相互相関を測る (X-Y の検証用、Phase 6)。
///
/// - `best_lag != 0` → どこかで L/R 間に遅延差が入っている (X-Y が壊れている)
/// - `at_zero` → 直接音区間で 0.9 以上、残響区間で 0.5–0.8 が設計値
pub fn stereo_correlation(
    left: &[f32],
    right: &[f32],
    max_lag: usize,
) -> Option<CorrelationEstimate> {
    let n = left.len().min(right.len());
    if n < max_lag * 4 || n == 0 {
        return None;
    }
    let energy = |x: &[f32]| x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
    let denom = (energy(&left[..n]) * energy(&right[..n])).sqrt();
    if denom <= 0.0 {
        return None;
    }

    let mut best = (f64::MIN, 0i64);
    let mut at_zero = 0.0;
    for lag in -(max_lag as i64)..=(max_lag as i64) {
        let mut acc = 0.0;
        // ラグつきの 2 配列参照なので range で回す。
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < n {
                acc += left[i] as f64 * right[j as usize] as f64;
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
    Some(CorrelationEstimate {
        at_zero,
        best: best.0,
        best_lag: best.1,
    })
}

// ---------------------------------------------------------------------------
// ラウドネス (ITU-R BS.1770-4)
//
// 音域バランス (Phase 10) の指標。ピーク dBFS は打撃スパイクに支配され、
// バンドレベルは単音の「大きさ」を 1 つの数にしない。知覚的な音量の比較には
// K 特性 + 400 ms ブロックのラウドネスを使う。
// ---------------------------------------------------------------------------

/// ラウドネス (BS.1770-4) の測定結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// ゲート付き積分ラウドネス [LUFS]。全ブロックがゲートを下回ると −∞
    pub integrated_lufs: f64,
    /// モーメンタリ (400 ms ブロック) の最大値 [LUFS]。ゲートなし
    pub momentary_max_lufs: f64,
    /// 400 ms ブロック (75% オーバーラップ) の数
    pub blocks: usize,
}

/// バイカッド 1 段 (係数は a0 = 1 に正規化済み、とは限らない — ITU の表の
/// 2 段目は b が非正規化のまま定義されるので、係数をそのまま持つ)。
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    /// 転置直接形 II で in-place に処理する。状態は 0 から始める。
    fn filter_in_place(&self, x: &mut [f64]) {
        let (mut z1, mut z2) = (0.0f64, 0.0f64);
        for v in x.iter_mut() {
            let y = self.b0 * *v + z1;
            z1 = self.b1 * *v - self.a1 * y + z2;
            z2 = self.b2 * *v - self.a2 * y;
            *v = y;
        }
    }
}

/// K 特性 1 段目: 頭部の音響効果を模す高域シェルフ (約 +4 dB)。
///
/// BS.1770 は 48 kHz の係数表しか与えないので、表を再現する設計パラメータ
/// (De Man 2014) からどのサンプリング周波数でも解析的に導出する。
/// **48 kHz で規格表と 1e-6 以内で一致すること**をテストで固定している。
fn k_shelf(sample_rate: f64) -> Biquad {
    let fc = 1_681.974_450_955_533;
    let g_db = 3.999_843_853_973_347;
    let q = 0.707_175_236_955_419_6;
    let k = (std::f64::consts::PI * fc / sample_rate).tan();
    let vh = 10.0f64.powf(g_db / 20.0);
    let vb = vh.powf(0.499_666_774_155);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

/// K 特性 2 段目: RLB ハイパス (低域の重み下げ)。
///
/// 規格表どおり b = [1, −2, 1] を非正規化のまま使う (DC ゲインは 0 のまま、
/// 通過域ゲインがぴったり 1 になる形)。
fn k_highpass(sample_rate: f64) -> Biquad {
    let fc = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (std::f64::consts::PI * fc / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    }
}

/// ブロックパワー → LUFS。−0.691 は K 特性の 997 Hz でのゲインを打ち消す
/// 規格の定数 (997 Hz フルスケール正弦 1ch がちょうど −3.01 LUFS になる)。
fn power_to_lufs(power: f64) -> f64 {
    if power > 0.0 {
        -0.691 + 10.0 * power.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// ラウドネス (BS.1770-4) を測る。
///
/// K 特性 (2 段バイカッド) → 400 ms ブロック (75% オーバーラップ) の平均二乗
/// → チャンネル和 (重みは全チャンネル 1.0。L/R/C 相当。サラウンドの 1.41 は
/// 扱わない)。積分値は −70 LUFS の絶対ゲート + −10 LU の相対ゲート付き。
///
/// # 単音の音域バランスにはモーメンタリ最大を使うこと
///
/// 単音は減衰音で、ゲート付き積分値は**レンダリング長と T60 に依存する**。
/// この楽器はバスの基音 T60 = 12 s / トレブル 3 s なので、積分値は「どれだけ
/// 長く鳴り続けるか」を測ってしまい、低音を実態より大きく読む。打撃音の
/// 知覚的な大きさはアタック近傍の最も大きい 400 ms が支配するので、音域の
/// 比較は [`Loudness::momentary_max_lufs`] で行い、条件 (長さ・速度・ROOM off)
/// を固定して比べる。積分値は持続部の寄与の観察用。
///
/// 信号が 400 ms に満たない場合は `None`。無音は `Some` で −∞ を返す
/// (「測れない」と「無音」を区別する)。
pub fn loudness(channels: &[Vec<f32>], sample_rate: f64) -> Option<Loudness> {
    if channels.is_empty() || sample_rate <= 0.0 {
        return None;
    }
    let block = (sample_rate * 0.4).round() as usize;
    let hop = (sample_rate * 0.1).round() as usize;
    let frames = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    if block == 0 || hop == 0 || frames < block {
        return None;
    }

    let shelf = k_shelf(sample_rate);
    let hp = k_highpass(sample_rate);
    let weighted: Vec<Vec<f64>> = channels
        .iter()
        .map(|c| {
            let mut x: Vec<f64> = c[..frames].iter().map(|&v| v as f64).collect();
            shelf.filter_in_place(&mut x);
            hp.filter_in_place(&mut x);
            x
        })
        .collect();

    // ブロックごとの Σ_ch 平均二乗パワー。
    let mut block_power = Vec::new();
    let mut start = 0;
    while start + block <= frames {
        let mut p = 0.0;
        for w in &weighted {
            p += w[start..start + block].iter().map(|&v| v * v).sum::<f64>() / block as f64;
        }
        block_power.push(p);
        start += hop;
    }

    let momentary_max_lufs = block_power
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, |a, p| a.max(power_to_lufs(p)));

    // 積分値: 絶対ゲート (−70 LUFS) → 相対ゲート (通過ブロック平均 −10 LU)。
    let mean = |ps: &[f64]| ps.iter().sum::<f64>() / ps.len() as f64;
    let above_abs: Vec<f64> = block_power
        .iter()
        .copied()
        .filter(|&p| power_to_lufs(p) > -70.0)
        .collect();
    let integrated_lufs = if above_abs.is_empty() {
        f64::NEG_INFINITY
    } else {
        let gamma_r = power_to_lufs(mean(&above_abs)) - 10.0;
        let above_rel: Vec<f64> = above_abs
            .iter()
            .copied()
            .filter(|&p| power_to_lufs(p) > gamma_r)
            .collect();
        if above_rel.is_empty() {
            f64::NEG_INFINITY
        } else {
            power_to_lufs(mean(&above_rel))
        }
    };

    Some(Loudness {
        integrated_lufs,
        momentary_max_lufs,
        blocks: block_power.len(),
    })
}

/// 2 つの窓の振幅比から T60 [s] を求める。
///
/// 単一の指数減衰を仮定した粗い推定。ダブルデケイがあると意味を失うが、
/// **2 つの音源を同じ条件で比べる**ぶんには十分。
pub fn t60_between(early: f64, late: f64, dt_sec: f64) -> Option<f64> {
    if early <= 0.0 || late <= 0.0 || dt_sec <= 0.0 || late >= early {
        return None;
    }
    Some(3.0 * std::f64::consts::LN_10 * dt_sec / (early / late).ln())
}

/// 線形振幅 → dBFS。0 以下は `f64::NEG_INFINITY`。
pub fn to_db(amplitude: f64) -> f64 {
    if amplitude > 0.0 {
        20.0 * amplitude.log10()
    } else {
        f64::NEG_INFINITY
    }
}

// ---------------------------------------------------------------------------
// WAV の読み書き
//
// `render` (書く) と `analyze` (読む) と統合テスト (往復) が同じ経路を通るよう、
// 1 か所だけに置く。
//
// **チャンネルは分けて持つ。** Phase 6 の ROOM では L/R の相互相関を測るので、
// 読んだ時点でモノ化すると測定そのものが成立しなくなる。Phase 0 の時点では
// モノしか書かないが、後から直すと「モノ化された値で調整してしまった」事故が
// 起きうるので、最初から分けておく。
// ---------------------------------------------------------------------------

/// 読み込んだ WAV。
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    /// チャンネルごとの標本列。長さは全チャンネルで等しい。
    pub channels: Vec<Vec<f32>>,
    pub sample_rate: f64,
}

impl Wav {
    /// フレーム数 (1 チャンネルあたりの標本数)。
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, |c| c.len())
    }

    /// チャンネル数。
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// 長さ [s]。
    pub fn duration_sec(&self) -> f64 {
        if self.sample_rate > 0.0 {
            self.frames() as f64 / self.sample_rate
        } else {
            0.0
        }
    }

    /// 全チャンネルの平均を取ってモノにする。
    ///
    /// **モノ和ではなく平均**であることに注意 (2ch なら `(L+R)/2`)。
    /// X-Y のモノ互換性を見るときは和と平均でレベルが 6 dB 違うので、
    /// どちらを見ているかを取り違えないこと。
    pub fn mono(&self) -> Vec<f32> {
        match self.channels.len() {
            0 => Vec::new(),
            1 => self.channels[0].clone(),
            n => (0..self.frames())
                .map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / n as f32)
                .collect(),
        }
    }
}

/// WAV を読む。float / 整数 PCM のどちらでも `[-1, 1]` に揃えて返す。
pub fn read_wav(path: &Path) -> Result<Wav, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("{} を開けません: {e}", path.display()))?;
    let spec = reader.spec();

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| format!("読み込みに失敗しました: {e}"))?,
        hound::SampleFormat::Int => {
            // 整数 PCM は最大値で正規化して [-1, 1] に揃える。
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("読み込みに失敗しました: {e}"))?
        }
    };

    if interleaved.is_empty() {
        return Err("WAV が空です".into());
    }

    let ch = (spec.channels as usize).max(1);
    let frames = interleaved.len() / ch;
    if frames == 0 {
        return Err("WAV にフレームがありません".into());
    }

    let mut channels = vec![Vec::with_capacity(frames); ch];
    // 端数フレーム (書き手が途中で落ちた WAV) は捨てる。長さを揃えるほうが大事。
    for (i, &s) in interleaved.iter().take(frames * ch).enumerate() {
        channels[i % ch].push(s);
    }

    Ok(Wav {
        channels,
        sample_rate: spec.sample_rate as f64,
    })
}

/// WAV を書く (32-bit float)。チャンネルの長さは揃っている必要がある。
pub fn write_wav(path: &Path, channels: &[Vec<f32>], sample_rate: f64) -> Result<(), String> {
    let Some(first) = channels.first() else {
        return Err("チャンネルがありません".into());
    };
    let frames = first.len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err("チャンネルの長さが揃っていません".into());
    }
    if sample_rate <= 0.0 {
        return Err(format!("サンプリング周波数が不正です: {sample_rate}"));
    }

    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("{} を作成できません: {e}", dir.display()))?;
        }
    }

    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: sample_rate.round() as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("{} を開けません: {e}", path.display()))?;

    for f in 0..frames {
        for c in channels {
            writer
                .write_sample(c[f])
                .map_err(|e| format!("書き込みに失敗しました: {e}"))?;
        }
    }
    writer
        .finalize()
        .map_err(|e| format!("WAV を確定できません: {e}"))
}

/// モノ WAV を書く。
pub fn write_wav_mono(path: &Path, samples: &[f32], sample_rate: f64) -> Result<(), String> {
    let channels = vec![samples.to_vec()];
    write_wav(path, &channels, sample_rate)
}

#[cfg(test)]
mod loudness_tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    fn sine(freq: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (std::f64::consts::TAU * freq * i as f64 / SR).sin()) as f32)
            .collect()
    }

    #[test]
    fn k_weighting_matches_the_bs1770_table_at_48khz() {
        // パラメトリック導出が規格の係数表 (BS.1770-4 Table 1/2) を再現すること。
        // ここが崩れたら以降のラウドネス値はすべて疑わしい。
        let s = k_shelf(SR);
        assert_relative_eq!(s.b0, 1.535_124_859_586_97, epsilon = 1e-6);
        assert_relative_eq!(s.b1, -2.691_696_189_406_38, epsilon = 1e-6);
        assert_relative_eq!(s.b2, 1.198_392_810_852_85, epsilon = 1e-6);
        assert_relative_eq!(s.a1, -1.690_659_293_182_41, epsilon = 1e-6);
        assert_relative_eq!(s.a2, 0.732_480_774_215_85, epsilon = 1e-6);

        let h = k_highpass(SR);
        assert_relative_eq!(h.b0, 1.0, epsilon = 1e-9);
        assert_relative_eq!(h.b1, -2.0, epsilon = 1e-9);
        assert_relative_eq!(h.b2, 1.0, epsilon = 1e-9);
        assert_relative_eq!(h.a1, -1.990_047_454_833_98, epsilon = 1e-6);
        assert_relative_eq!(h.a2, 0.990_072_250_366_21, epsilon = 1e-6);
    }

    #[test]
    fn a_full_scale_997hz_sine_reads_the_reference_levels() {
        // 規格の基準: 997 Hz フルスケール正弦を 1ch に入れると −3.01 LUFS。
        // L = R のステレオでは +3.01 dB されて 0.0 LUFS。
        let x = sine(997.0, 1.0, (SR * 2.0) as usize);

        let mono = loudness(&[x.clone()], SR).expect("測れること");
        assert_relative_eq!(mono.integrated_lufs, -3.01, epsilon = 0.05);
        assert_relative_eq!(mono.momentary_max_lufs, -3.01, epsilon = 0.05);

        let stereo = loudness(&[x.clone(), x.clone()], SR).expect("測れること");
        assert_relative_eq!(stereo.integrated_lufs, 0.0, epsilon = 0.05);

        // 片チャンネルだけなら 1ch と同じ (無音チャンネルは足しても変わらない)。
        let silent = vec![0.0f32; x.len()];
        let one_side = loudness(&[x, silent], SR).expect("測れること");
        assert_relative_eq!(one_side.integrated_lufs, -3.01, epsilon = 0.05);
    }

    #[test]
    fn loudness_scales_linearly_with_level() {
        // −18 dBFS のステレオ正弦は −18 LUFS 付近。
        let amp = 10.0f64.powf(-18.0 / 20.0);
        let x = sine(997.0, amp, (SR * 2.0) as usize);
        let l = loudness(&[x.clone(), x], SR).expect("測れること");
        assert_relative_eq!(l.integrated_lufs, -18.0, epsilon = 0.05);
    }

    #[test]
    fn the_absolute_gate_ignores_appended_silence() {
        // 音 5 秒 + 無音 15 秒。ゲートが無ければ平均パワーは 6 dB 落ちるが、
        // 無音ブロックは −70 LUFS の絶対ゲートで捨てられる。境界をまたぐ
        // ブロック (部分的に音を含む) は相対ゲートを通るので厳密には不変に
        // ならない — トーンを長くして寄与を薄め、0.3 LU 以内で見る。
        let tone = sine(997.0, 0.5, (SR * 5.0) as usize);
        let mut with_silence = tone.clone();
        with_silence.extend(std::iter::repeat(0.0f32).take((SR * 15.0) as usize));

        let short = loudness(&[tone], SR).expect("測れること");
        let long = loudness(&[with_silence], SR).expect("測れること");
        assert!(
            (short.integrated_lufs - long.integrated_lufs).abs() < 0.3,
            "無音の付加で積分値が動いた: {:.2} → {:.2}",
            short.integrated_lufs,
            long.integrated_lufs
        );
    }

    #[test]
    fn the_relative_gate_ignores_a_quiet_tail() {
        // 音 2 秒 + −46 dB の尾 8 秒。尾は絶対ゲート (−70) は超えるが、
        // 相対ゲート (−10 LU) で捨てられる。ゲートが無ければ −7 dB 以上動く。
        let loud = sine(997.0, 1.0, (SR * 2.0) as usize);
        let mut with_tail = loud.clone();
        with_tail.extend(sine(997.0, 0.005, (SR * 8.0) as usize));

        let short = loudness(&[loud], SR).expect("測れること");
        let long = loudness(&[with_tail], SR).expect("測れること");
        assert!(
            (short.integrated_lufs - long.integrated_lufs).abs() < 0.5,
            "小音量の尾で積分値が動いた: {:.2} → {:.2}",
            short.integrated_lufs,
            long.integrated_lufs
        );
    }

    #[test]
    fn momentary_max_is_untouched_by_appended_silence() {
        // 減衰音の比較にモーメンタリ最大を使う根拠: 後ろに何を足しても不変。
        let tone = sine(997.0, 0.7, SR as usize);
        let mut with_silence = tone.clone();
        with_silence.extend(std::iter::repeat(0.0f32).take((SR * 5.0) as usize));

        let a = loudness(&[tone], SR).unwrap().momentary_max_lufs;
        let b = loudness(&[with_silence], SR).unwrap().momentary_max_lufs;
        assert_relative_eq!(a, b, epsilon = 1e-9);
    }

    #[test]
    fn loudness_is_safe_on_degenerate_input() {
        // 400 ms 未満は「測れない」。
        assert!(loudness(&[vec![0.5f32; 100]], SR).is_none());
        assert!(loudness(&[], SR).is_none());
        // 無音は「測れた上で −∞」(「測れない」と区別する)。
        let l = loudness(&[vec![0.0f32; SR as usize]], SR).expect("測れること");
        assert!(l.integrated_lufs.is_infinite());
        assert!(l.momentary_max_lufs.is_infinite());
    }
}

#[cfg(test)]
mod partial_search_tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    /// インハーモニシティを持つ部分音列を合成する。
    fn inharmonic_tone(f0: f64, b: f64, count: usize, len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        for n in 1..=count {
            let nf = n as f64;
            let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
            if f >= SR * 0.5 {
                break;
            }
            // 高次ほど小さく。実際のピアノに似せる。
            let amp = 1.0 / nf;
            for (i, s) in x.iter_mut().enumerate() {
                *s += (amp * (std::f64::consts::TAU * f * i as f64 / SR).sin()) as f32;
            }
        }
        x
    }

    #[test]
    fn find_partial_locates_a_stretched_partial() {
        // 整数倍の位置を測るだけでは見つからない部分音を、走査で当てる。
        let b = 8.0e-4;
        let x = inharmonic_tone(440.0, b, 12, 48_000);

        for n in [4usize, 8, 12] {
            let found = find_partial(&x, SR, 440.0, n, 150.0).expect("見つかること");
            let expected = n as f64 * 440.0 * (1.0 + b * (n * n) as f64).sqrt();
            assert_relative_eq!(found.freq_hz, expected, max_relative = 2e-3);
            assert!(found.magnitude > 0.0);
            // ずれは必ず上方向。
            assert!(
                found.cents > 0.0,
                "第{n}部分音のずれが {} cent",
                found.cents
            );
        }
    }

    #[test]
    fn inharmonicity_is_recovered_from_the_measured_positions() {
        // これができるので、参照音源の B を知らなくても比較できる。
        for b in [2.0e-4, 5.0e-4, 1.0e-3] {
            let x = inharmonic_tone(440.0, b, 12, 48_000);
            let partials: Vec<_> = (1..=12)
                .filter_map(|n| find_partial(&x, SR, 440.0, n, 200.0))
                .collect();
            let estimated = estimate_inharmonicity(&partials).expect("推定できること");
            assert_relative_eq!(estimated, b, max_relative = 0.1);
        }
    }

    #[test]
    fn a_harmonic_tone_has_almost_no_inharmonicity() {
        let x = inharmonic_tone(440.0, 0.0, 12, 48_000);
        let partials: Vec<_> = (1..=12)
            .filter_map(|n| find_partial(&x, SR, 440.0, n, 150.0))
            .collect();
        // 完全な整数倍なら 0 近傍 (走査の刻みぶんの誤差しか出ない)。
        let estimated = estimate_inharmonicity(&partials).unwrap_or(0.0);
        assert!(estimated < 2.0e-5, "B = {estimated:.3e} は大きすぎる");
    }

    #[test]
    fn t60_between_two_windows() {
        // 1 秒で 1/1000 なら T60 = 1 秒。
        assert_relative_eq!(
            t60_between(1.0, 1e-3, 1.0).unwrap(),
            1.0,
            max_relative = 1e-9
        );
        // 減衰していなければ測れない。
        assert!(t60_between(1.0, 1.0, 1.0).is_none());
        assert!(t60_between(1.0, 2.0, 1.0).is_none());
        assert!(t60_between(0.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn find_partial_refuses_frequencies_above_nyquist() {
        let x = inharmonic_tone(440.0, 0.0, 4, 4_800);
        assert!(find_partial(&x, SR, 440.0, 60, 150.0).is_none());
    }

    #[test]
    fn centroid_of_two_known_partials_is_their_weighted_mean() {
        // 440 Hz (振幅 1.0) + 880 Hz (振幅 0.5) → (440·1 + 880·0.5)/1.5 = 586.67 Hz。
        let n = 48_000;
        let mut x = vec![0.0f32; n];
        for (f, a) in [(440.0, 1.0), (880.0, 0.5)] {
            for (i, s) in x.iter_mut().enumerate() {
                *s += (a * (std::f64::consts::TAU * f * i as f64 / SR).sin()) as f32;
            }
        }
        let c = spectral_centroid_partials(&x, SR, 440.0, 8).expect("測れること");
        assert_relative_eq!(c, 586.67, max_relative = 0.01);
    }

    #[test]
    fn centroid_rises_when_high_partials_get_stronger() {
        // 打弦点指標としての要件: 高次が増えれば重心は上がる。
        let make = |high_amp: f64| -> Vec<f32> {
            let n = 48_000;
            let mut x = vec![0.0f32; n];
            for (f, a) in [(220.0, 1.0), (1_760.0, high_amp)] {
                for (i, s) in x.iter_mut().enumerate() {
                    *s += (a * (std::f64::consts::TAU * f * i as f64 / SR).sin()) as f32;
                }
            }
            x
        };
        let dark = spectral_centroid_partials(&make(0.1), SR, 220.0, 12).unwrap();
        let bright = spectral_centroid_partials(&make(0.8), SR, 220.0, 12).unwrap();
        assert!(bright > dark + 200.0, "dark {dark:.1} / bright {bright:.1}");
    }

    #[test]
    fn centroid_is_none_on_silence() {
        assert!(spectral_centroid_partials(&[0.0; 48_000], SR, 440.0, 8).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const SR: f64 = 48_000.0;

    /// 振幅 `amp`、周波数 `freq` の定常正弦波。
    fn sine(freq: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (std::f64::consts::TAU * freq * i as f64 / SR).sin()) as f32)
            .collect()
    }

    #[test]
    fn goertzel_recovers_the_amplitude() {
        let x = sine(1_000.0, 0.25, 48_000);
        let mag = goertzel_magnitude(&x, SR, 1_000.0);
        assert_relative_eq!(mag, 0.25, max_relative = 0.01);
    }

    #[test]
    fn goertzel_rejects_other_frequencies() {
        let x = sine(1_000.0, 1.0, 48_000);
        // 1 kHz の信号を 3 kHz で測ればほぼ何も出ない。
        let off = goertzel_magnitude(&x, SR, 3_000.0);
        assert!(off < 1e-3, "leakage too high: {off}");
    }

    #[test]
    fn goertzel_separates_two_partials() {
        let n = 48_000;
        let a = sine(440.0, 1.0, n);
        let b = sine(880.0, 0.3, n);
        let mixed: Vec<f32> = a.iter().zip(b.iter()).map(|(x, y)| x + y).collect();

        assert_relative_eq!(
            goertzel_magnitude(&mixed, SR, 440.0),
            1.0,
            max_relative = 0.02
        );
        assert_relative_eq!(
            goertzel_magnitude(&mixed, SR, 880.0),
            0.3,
            max_relative = 0.02
        );
    }

    #[test]
    fn goertzel_is_safe_on_degenerate_input() {
        assert_eq!(goertzel_magnitude(&[], SR, 440.0), 0.0);
        assert_eq!(goertzel_magnitude(&[0.0; 100], SR, 0.0), 0.0);
        // ナイキスト以上は測れない。
        assert_eq!(goertzel_magnitude(&[0.1; 100], SR, SR), 0.0);
    }

    #[test]
    fn t60_matches_a_known_exponential_decay() {
        let t60 = 1.5;
        let n = (SR * 3.0) as usize;
        let decay = (-3.0 * std::f64::consts::LN_10 / (t60 * SR)).exp();
        let mut amp = 1.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let s = amp * (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin();
                amp *= decay;
                s as f32
            })
            .collect();

        let est = estimate_t60(&x, SR).expect("減衰を検出できるはず");
        assert_relative_eq!(est.t60_sec, t60, max_relative = 0.05);
        assert!(est.r_squared > 0.99, "R^2 = {}", est.r_squared);
    }

    #[test]
    fn partial_t60_measures_each_partial_separately() {
        // 2 本の部分音に別々の減衰時間を与え、それぞれ独立に測れることを確認する。
        // ピアノの減衰設計 σ(f) = c1 + c3·f² を検証するのに必要な能力。
        let n = (SR * 4.0) as usize;
        let mut x = vec![0.0f32; n];
        for (freq, t60) in [(440.0, 3.0), (1_760.0, 1.0)] {
            let decay = (-3.0 * std::f64::consts::LN_10 / (t60 * SR)).exp();
            let mut amp = 1.0f64;
            for (i, s) in x.iter_mut().enumerate() {
                *s += (amp * (std::f64::consts::TAU * freq * i as f64 / SR).sin()) as f32;
                let _ = i;
                amp *= decay;
            }
        }

        let low = estimate_partial_t60(&x, SR, 440.0).expect("基音を測れるはず");
        let high = estimate_partial_t60(&x, SR, 1_760.0).expect("倍音を測れるはず");

        assert_relative_eq!(low.t60_sec, 3.0, max_relative = 0.1);
        assert_relative_eq!(high.t60_sec, 1.0, max_relative = 0.1);
        assert!(low.r_squared > 0.98 && high.r_squared > 0.98);
    }

    #[test]
    fn short_t60_is_measurable_with_a_narrow_window() {
        // D-001 の解消の検証。既定 (窓 0.2 / ホップ 0.05) では測れない短い減衰を、
        // 窓とホップを狭めれば測れること。高音の弦の T60 は 0.4 秒を切る。
        for t60 in [0.375, 0.25, 0.15] {
            let n = (SR * 1.5) as usize;
            let decay = (-3.0 * std::f64::consts::LN_10 / (t60 * SR)).exp();
            let mut amp = 1.0f64;
            let x: Vec<f32> = (0..n)
                .map(|i| {
                    let s = amp * (std::f64::consts::TAU * 2_000.0 * i as f64 / SR).sin();
                    amp *= decay;
                    s as f32
                })
                .collect();

            // 既定では測れない (これが D-001)。
            if t60 < 0.4 {
                assert!(
                    estimate_partial_t60(&x, SR, 2_000.0).is_none(),
                    "T60 {t60} s が既定の窓で測れてしまった (D-001 の前提が変わった)"
                );
            }
            // ホップ ≤ T60/12 に狭めれば測れる。
            let hop = t60 / 15.0;
            let est = estimate_partial_t60_with(&x, SR, 2_000.0, hop * 3.0, hop)
                .unwrap_or_else(|| panic!("T60 {t60} s が狭い窓でも測れない"));
            assert_relative_eq!(est.t60_sec, t60, max_relative = 0.1);
        }
    }

    #[test]
    fn modulation_depth_detects_beating_and_ignores_plain_decay() {
        // 2 本のデチューンした正弦波の和はうなり、1 本は平坦。
        let n = (SR * 4.0) as usize;
        let decay = (-3.0 * std::f64::consts::LN_10 / (8.0 * SR)).exp();

        let make = |detune_hz: f64| -> Vec<f32> {
            let mut amp = 1.0f64;
            (0..n)
                .map(|i| {
                    let t = i as f64 / SR;
                    let s = amp
                        * ((std::f64::consts::TAU * 440.0 * t).sin()
                            + (std::f64::consts::TAU * (440.0 + detune_hz) * t).sin());
                    amp *= decay;
                    (0.5 * s) as f32
                })
                .collect()
        };

        let beating = modulation_depth(&make(0.5), SR, 440.0, 0.25, 0.1, 0.2).unwrap();
        let plain = modulation_depth(&make(0.0), SR, 440.0, 0.25, 0.1, 0.2).unwrap();

        assert!(
            beating.depth_db > 6.0,
            "うなりが検出されない: {:.2} dB",
            beating.depth_db
        );
        assert!(
            plain.depth_db < 1.0,
            "うなりの無い信号で誤検出: {:.2} dB",
            plain.depth_db
        );
    }

    #[test]
    fn partial_t60_returns_none_on_absent_partial() {
        let x = sine(440.0, 0.5, (SR * 3.0) as usize);
        // 存在しない部分音には減衰が見えない。
        assert!(estimate_partial_t60(&x, SR, 3_000.0).is_none());
    }

    #[test]
    fn partial_t60_returns_none_on_short_input() {
        assert!(estimate_partial_t60(&[0.0; 100], SR, 440.0).is_none());
    }

    #[test]
    fn t60_returns_none_without_decay() {
        let x = sine(440.0, 0.5, 48_000);
        assert!(estimate_t60(&x, SR).is_none());
    }

    #[test]
    fn t60_returns_none_on_silence() {
        assert!(estimate_t60(&[0.0; 48_000], SR).is_none());
    }

    #[test]
    fn fundamental_estimation_is_accurate() {
        for freq in [110.0, 261.6, 440.0, 1_000.0] {
            let x = sine(freq, 0.5, 24_000);
            let f0 = estimate_fundamental(&x, SR, 20.0, 5_000.0).expect("推定できるはず");
            assert_relative_eq!(f0, freq, max_relative = 0.01);
        }
    }

    #[test]
    fn fundamental_finds_the_root_of_a_harmonic_series() {
        let n = 24_000;
        let mut x = vec![0.0f32; n];
        for (h, amp) in [(1.0, 1.0), (2.0, 0.6), (3.0, 0.4), (4.0, 0.2)] {
            for (i, s) in sine(220.0 * h, amp, n).iter().enumerate() {
                x[i] += s;
            }
        }
        let f0 = estimate_fundamental(&x, SR, 50.0, 2_000.0).expect("推定できるはず");
        assert_relative_eq!(f0, 220.0, max_relative = 0.02);
    }

    #[test]
    fn fundamental_avoids_sub_octave_errors() {
        // 純音の自己相関は基本周期の整数倍すべてでほぼ同じ高さのピークを作る。
        // 探索範囲を極端に広く取っても 1/2・1/3 に落ちないことを確認する。
        let x = sine(440.0, 0.5, 24_000);
        let f0 = estimate_fundamental(&x, SR, 20.0, 8_000.0).expect("推定できるはず");
        assert_relative_eq!(f0, 440.0, max_relative = 0.01);
    }

    #[test]
    fn fundamental_is_safe_on_degenerate_input() {
        assert!(estimate_fundamental(&[], SR, 20.0, 5_000.0).is_none());
        assert!(estimate_fundamental(&[0.0; 48_000], SR, 20.0, 5_000.0).is_none());
        // 範囲が逆でも panic しない。
        assert!(estimate_fundamental(&[0.1; 48_000], SR, 5_000.0, 20.0).is_none());
    }

    #[test]
    fn db_conversion() {
        assert_relative_eq!(to_db(1.0), 0.0, epsilon = 1e-12);
        assert_relative_eq!(to_db(0.5), -6.0206, epsilon = 1e-3);
        assert_relative_eq!(to_db(0.001), -60.0, epsilon = 1e-9);
        assert!(to_db(0.0).is_infinite());
    }
}
