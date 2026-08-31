//! Phase 0 のオフラインレンダラ。
//!
//! モデル本体はまだ無い。`phydulcimer-core` の `DecayingSine` を鳴らして WAV に
//! 書き出し、`phydulcimer-analyze` がそれを**設計値どおりに読み取れるか**を
//! 確かめるためのもの。解析解が既知の信号なので、解析ツール側のバグとモデルの
//! バグを切り分ける基準になる。
//!
//! Phase 1 以降、ここに弦とハンマーが入る。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phydulcimer_core::{smoke::DecayingSine, Sample, DEFAULT_SAMPLE_RATE};

const USAGE: &str = "\
phydulcimer-render — PhyDulcimer offline renderer (Phase 0: smoke only)

USAGE:
    phydulcimer-render --out <PATH> [OPTIONS]

OPTIONS:
    --out <PATH>           write the WAV here                     [required]
    --freq <HZ>            fundamental frequency                  [default: 440]
    --t60 <SEC>            60 dB decay time; 0 = no decay         [default: 2.0]
    --dur <SEC>            length                                 [default: 3.0]
    --amp <A>              amplitude of the fundamental           [default: 0.5]
    --partials <N>         stack N partials (amplitude 1/n)       [default: 1]
    --inharmonicity <B>    place partials at n*f0*sqrt(1+B*n^2)   [default: 0]
    --t60-taper <R>        partial n decays with t60 / n^R        [default: 0]
    --sample-rate <HZ>     sample rate                            [default: 48000]
    --peak <0..1>          normalise to this peak; 0 = don't      [default: 0]
    -h, --help             show this help

NOTE:
    --peak defaults to 0 (no normalisation). PhyPiano defaulted it to 0.9 and
    every A/B comparison silently lost its level difference unless --peak 0 was
    passed. Normalise only when the absolute level does not matter.
";

#[derive(Debug, Clone)]
struct Args {
    out: PathBuf,
    freq: f64,
    t60: f64,
    dur: f64,
    amp: f64,
    partials: usize,
    inharmonicity: f64,
    t60_taper: f64,
    sample_rate: f64,
    peak: f64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            out: PathBuf::new(),
            freq: 440.0,
            t60: 2.0,
            dur: 3.0,
            amp: 0.5,
            partials: 1,
            inharmonicity: 0.0,
            t60_taper: 0.0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            // 既定は「正規化しない」。理由は USAGE の NOTE を参照。
            peak: 0.0,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args(std::env::args().skip(1).collect())? else {
        print!("{USAGE}");
        return Ok(());
    };

    let buf = render(&args)?;
    let raw_peak = buf.iter().fold(0.0 as Sample, |a, &b| a.max(b.abs()));
    if !raw_peak.is_finite() {
        return Err("出力に非有限の値が含まれています".into());
    }

    let mut buf = buf;
    if args.peak > 0.0 && raw_peak > 0.0 {
        let gain = args.peak as Sample / raw_peak;
        for s in buf.iter_mut() {
            *s *= gain;
        }
    }

    write_wav(&args.out, &buf, args.sample_rate)?;

    println!(
        "wrote {} ({:.3} s @ {} Hz, {} partial(s), raw peak {:.4})",
        args.out.display(),
        buf.len() as f64 / args.sample_rate,
        args.sample_rate,
        args.partials,
        raw_peak
    );
    Ok(())
}

/// 部分音を重ねた減衰正弦波を合成する。
///
/// 第 n 部分音は `f_n = n·f0·√(1+B·n²)`、振幅 `amp/n`、減衰 `t60/n^taper`。
/// ナイキストを超える部分音は打ち切る。
fn render(args: &Args) -> Result<Vec<Sample>, String> {
    if args.dur <= 0.0 {
        return Err(format!("--dur は正の値が必要です: {}", args.dur));
    }
    if args.freq <= 0.0 {
        return Err(format!("--freq は正の値が必要です: {}", args.freq));
    }
    if args.sample_rate <= 0.0 {
        return Err(format!(
            "--sample-rate は正の値が必要です: {}",
            args.sample_rate
        ));
    }
    if args.partials == 0 {
        return Err("--partials は 1 以上が必要です".into());
    }

    let n = (args.dur * args.sample_rate).round() as usize;
    let mut buf = vec![0.0 as Sample; n];
    let nyquist = args.sample_rate * 0.5;

    for k in 1..=args.partials {
        let kf = k as f64;
        let freq = kf * args.freq * (1.0 + args.inharmonicity * kf * kf).sqrt();
        if freq >= nyquist {
            break;
        }

        // t60 が 0 なら減衰なし。DecayingSine は INFINITY をそう解釈する。
        let t60 = if args.t60 > 0.0 {
            args.t60 / kf.powf(args.t60_taper)
        } else {
            f64::INFINITY
        };

        DecayingSine::new(freq, t60, args.sample_rate, args.amp / kf).add_to(&mut buf);
    }

    Ok(buf)
}

/// 32-bit float のモノ WAV を書く。
///
/// `phydulcimer-analyze` の lib にも同じものがある。共有すれば重複は消えるが、
/// **レンダラが解析ツールに依存する**という筋の悪い依存方向になる。30 行の
/// 重複のほうが安いと判断した (→ `docs/problems.md` D-002)。
fn write_wav(path: &Path, buf: &[Sample], sample_rate: f64) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("{} を作成できません: {e}", dir.display()))?;
        }
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate.round() as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("{} を開けません: {e}", path.display()))?;

    for &s in buf {
        writer
            .write_sample(s)
            .map_err(|e| format!("書き込みに失敗しました: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("WAV を確定できません: {e}"))
}

/// `Ok(None)` はヘルプを表示して終わることを表す。
fn parse_args(argv: Vec<String>) -> Result<Option<Args>, String> {
    let mut args = Args::default();
    let mut it = argv.into_iter();

    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            return Ok(None);
        }

        let mut value = || -> Result<String, String> {
            it.next().ok_or_else(|| format!("{flag} に値がありません"))
        };

        match flag.as_str() {
            "--out" => args.out = PathBuf::from(value()?),
            "--freq" => args.freq = parse_f64(&value()?, "--freq")?,
            "--t60" => args.t60 = parse_f64(&value()?, "--t60")?,
            "--dur" => args.dur = parse_f64(&value()?, "--dur")?,
            "--amp" => args.amp = parse_f64(&value()?, "--amp")?,
            "--partials" => args.partials = parse_usize(&value()?, "--partials")?,
            "--inharmonicity" => args.inharmonicity = parse_f64(&value()?, "--inharmonicity")?,
            "--t60-taper" => args.t60_taper = parse_f64(&value()?, "--t60-taper")?,
            "--sample-rate" => args.sample_rate = parse_f64(&value()?, "--sample-rate")?,
            "--peak" => args.peak = parse_f64(&value()?, "--peak")?,
            other => return Err(format!("不明な引数: {other}")),
        }
    }

    if args.out.as_os_str().is_empty() {
        return Err("--out は必須です (-h でヘルプ)".into());
    }
    Ok(Some(args))
}

fn parse_f64(s: &str, name: &str) -> Result<f64, String> {
    s.parse::<f64>()
        .map_err(|_| format!("{name} の値が数値ではありません: {s}"))
}

fn parse_usize(s: &str, name: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .map_err(|_| format!("{name} の値が整数ではありません: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn args_for(partials: usize) -> Args {
        Args {
            out: PathBuf::from("unused.wav"),
            freq: 440.0,
            t60: 1.0,
            dur: 0.5,
            partials,
            ..Args::default()
        }
    }

    #[test]
    fn render_produces_the_requested_length() {
        let a = args_for(1);
        let buf = render(&a).expect("レンダリングできること");
        assert_eq!(buf.len(), (a.dur * a.sample_rate).round() as usize);
    }

    #[test]
    fn partials_are_stacked_with_falling_amplitude() {
        // 1 本のときより 4 本のほうがピークが大きい (位相が揃った瞬間がある)。
        let one = render(&args_for(1)).unwrap();
        let four = render(&args_for(4)).unwrap();
        let peak = |b: &[Sample]| b.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert!(peak(&four) > peak(&one));
    }

    #[test]
    fn partials_above_nyquist_are_dropped() {
        // 20 kHz を 8 倍まで積んでもナイキストで打ち切られ、非有限にならない。
        let a = Args {
            freq: 20_000.0,
            partials: 8,
            ..args_for(8)
        };
        let buf = render(&a).expect("打ち切って成功すること");
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn zero_t60_means_no_decay() {
        let a = Args {
            t60: 0.0,
            dur: 1.0,
            ..args_for(1)
        };
        let buf = render(&a).unwrap();
        let head = buf[..1000].iter().fold(0.0f32, |x, &y| x.max(y.abs()));
        let tail = buf[buf.len() - 1000..]
            .iter()
            .fold(0.0f32, |x, &y| x.max(y.abs()));
        assert_relative_eq!(head, tail, max_relative = 1e-3);
    }

    #[test]
    fn t60_taper_makes_higher_partials_decay_faster() {
        let a = Args {
            partials: 2,
            t60_taper: 1.0,
            ..args_for(2)
        };
        // 第 2 部分音の T60 は基音の半分になる (t60 / 2^1)。
        assert_relative_eq!(a.t60 / 2.0_f64.powf(a.t60_taper), 0.5, epsilon = 1e-12);
        assert!(render(&a).is_ok());
    }

    #[test]
    fn parse_args_requires_out() {
        assert!(parse_args(vec!["--freq".into(), "440".into()]).is_err());
    }

    #[test]
    fn parse_args_reports_help() {
        assert!(parse_args(vec!["-h".into()]).unwrap().is_none());
    }

    #[test]
    fn parse_args_rejects_unknown_flags() {
        let e = parse_args(vec!["--nope".into()]).unwrap_err();
        assert!(e.contains("--nope"), "{e}");
    }

    #[test]
    fn parse_args_rejects_missing_values() {
        assert!(parse_args(vec!["--freq".into()]).is_err());
    }

    #[test]
    fn peak_defaults_to_no_normalisation() {
        // ここが 0 でなくなったら A/B 比較が壊れる。意図的に固定する。
        assert_eq!(Args::default().peak, 0.0);
    }

    #[test]
    fn render_rejects_degenerate_arguments() {
        assert!(render(&Args {
            dur: 0.0,
            ..args_for(1)
        })
        .is_err());
        assert!(render(&Args {
            freq: 0.0,
            ..args_for(1)
        })
        .is_err());
        assert!(render(&Args {
            partials: 0,
            ..args_for(1)
        })
        .is_err());
    }
}
