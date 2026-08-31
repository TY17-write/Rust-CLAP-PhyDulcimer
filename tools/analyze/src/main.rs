//! WAV 解析ツール。`tools/render` の出力を数値で検証する。
//!
//! ```text
//! phydulcimer-analyze --in out/smoke.wav --t60 --f0
//! phydulcimer-analyze --in out/tone.wav --partials 440 --count 8 --partial-t60
//! ```
//!
//! 解析器そのものは lib 側 (`phydulcimer_analyze`) にある。ここは引数を読んで
//! 表を出すだけ。

use std::path::PathBuf;
use std::process::ExitCode;

use phydulcimer_analyze::{
    estimate_fundamental, estimate_inharmonicity, estimate_partial_t60_with, estimate_t60,
    find_partial, goertzel_magnitude, read_wav, to_db, Wav,
};

const USAGE: &str = "\
phydulcimer-analyze — PhyDulcimer WAV analysis

USAGE:
    phydulcimer-analyze --in <PATH> [OPTIONS]

OPTIONS:
    --in <PATH>            WAV to analyse                          [required]
    --channel <SEL>        mix | 0 | 1 | ...                       [default: mix]
    --window <SEC>         analysis window length; 0 = whole file  [default: 0]
    --offset <SEC>         where the window starts                 [default: 0]

    --f0                   estimate the fundamental (autocorrelation)
    --t60                  estimate T60 of the whole signal
    --freq <HZ>            measure the level at this frequency (repeatable)
    --partials <F0_HZ>     scan for the partial series of F0
    --count <N>            how many partials                       [default: 16]
    --scan-cents <C>       how far above n*f0 to scan              [default: 200]
    --partial-t60          T60 of each partial (needs --partials)
    --t60-window <SEC>     analysis window for --partial-t60       [default: 0.2]
    --t60-hop <SEC>        hop for --partial-t60; use <= T60/12    [default: 0.05]

    -h, --help             show this help

NOTES:
    Partials are found by scanning, not by computing n*f0*sqrt(1+B*n^2), so you
    do not need to know B in advance — it is reported instead.

    A struck string has a different decay time per partial, so --t60 on the whole
    signal is only meaningful for a single decaying sine (Phase 0). Use
    --partial-t60 once the model is in.
";

#[derive(Debug, Clone)]
struct Args {
    input: PathBuf,
    channel: Channel,
    window: f64,
    offset: f64,
    want_f0: bool,
    want_t60: bool,
    freqs: Vec<f64>,
    partials: Option<f64>,
    count: usize,
    scan_cents: f64,
    partial_t60: bool,
    t60_window: f64,
    t60_hop: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Mix,
    One(usize),
}

impl Default for Args {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            channel: Channel::Mix,
            window: 0.0,
            offset: 0.0,
            want_f0: false,
            want_t60: false,
            freqs: Vec::new(),
            partials: None,
            count: 16,
            scan_cents: 200.0,
            partial_t60: false,
            t60_window: 0.2,
            t60_hop: 0.05,
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

    let wav = read_wav(&args.input)?;
    let sr = wav.sample_rate;
    let full = select_channel(&wav, args.channel)?;
    let samples = slice_window(&full, sr, args.offset, args.window)?;

    print_header(&args, &wav, &samples);

    if args.want_f0 {
        match estimate_fundamental(&samples, sr, 20.0, 5_000.0) {
            Some(f0) => println!("f0 (autocorrelation)   {f0:.2} Hz"),
            None => println!("f0 (autocorrelation)   —  (推定できません)"),
        }
    }

    if args.want_t60 {
        match estimate_t60(&samples, sr) {
            Some(e) => println!(
                "T60 (whole signal)     {:.3} s   (fit {:.2}–{:.2} s, R^2 = {:.4})",
                e.t60_sec, e.fit_start_sec, e.fit_end_sec, e.r_squared
            ),
            None => println!("T60 (whole signal)     —  (減衰が見つかりません)"),
        }
    }

    for f in &args.freqs {
        let mag = goertzel_magnitude(&samples, sr, *f);
        println!(
            "level @ {f:>9.2} Hz    {:>9.6}  ({:>7.2} dB)",
            mag,
            to_db(mag)
        );
    }

    if let Some(f0) = args.partials {
        print_partials(&args, &samples, sr, f0);
    }

    Ok(())
}

fn print_header(args: &Args, wav: &Wav, samples: &[f32]) {
    let peak = samples.iter().fold(0.0f32, |a, &b| a.max(b.abs())) as f64;
    println!("file      {}", args.input.display());
    println!(
        "format    {} ch @ {} Hz, {} frames ({:.3} s)",
        wav.channel_count(),
        wav.sample_rate,
        wav.frames(),
        wav.duration_sec()
    );
    let ch = match args.channel {
        Channel::Mix => "mix".to_string(),
        Channel::One(i) => i.to_string(),
    };
    println!(
        "analysed  channel {}, {} samples ({:.3} s from {:.3} s)",
        ch,
        samples.len(),
        samples.len() as f64 / wav.sample_rate,
        args.offset
    );
    println!("peak      {:.6}  ({:.2} dBFS)", peak, to_db(peak));
    println!();
}

fn print_partials(args: &Args, samples: &[f32], sr: f64, f0: f64) {
    println!(
        "partials (f0 = {f0} Hz, scanning +{} cent)",
        args.scan_cents
    );
    if args.partial_t60 {
        println!("   n        freq     cent        dB       T60      R^2");
    } else {
        println!("   n        freq     cent        dB");
    }

    let mut found = Vec::new();
    for n in 1..=args.count {
        let Some(p) = find_partial(samples, sr, f0, n, args.scan_cents) else {
            continue;
        };
        let db = to_db(p.magnitude);

        if args.partial_t60 {
            match estimate_partial_t60_with(samples, sr, p.freq_hz, args.t60_window, args.t60_hop) {
                Some(e) => println!(
                    "{:>4}  {:>10.2}  {:>+7.1}  {:>8.2}  {:>8.3}  {:>7.4}",
                    p.n, p.freq_hz, p.cents, db, e.t60_sec, e.r_squared
                ),
                None => println!(
                    "{:>4}  {:>10.2}  {:>+7.1}  {:>8.2}  {:>8}  {:>7}",
                    p.n, p.freq_hz, p.cents, db, "—", "—"
                ),
            }
        } else {
            println!(
                "{:>4}  {:>10.2}  {:>+7.1}  {:>8.2}",
                p.n, p.freq_hz, p.cents, db
            );
        }
        found.push(p);
    }

    match estimate_inharmonicity(&found) {
        Some(b) => println!("\ninharmonicity B = {b:.3e}  (n >= 4 の中央値)"),
        None => println!("\ninharmonicity B = —  (n >= 4 の部分音が足りません)"),
    }
}

fn select_channel(wav: &Wav, sel: Channel) -> Result<Vec<f32>, String> {
    match sel {
        Channel::Mix => Ok(wav.mono()),
        Channel::One(i) => wav
            .channels
            .get(i)
            .cloned()
            .ok_or_else(|| format!("チャンネル {i} がありません ({} ch)", wav.channel_count())),
    }
}

/// `offset` から `window` 秒ぶんを切り出す。`window` が 0 なら末尾まで。
fn slice_window(samples: &[f32], sr: f64, offset: f64, window: f64) -> Result<Vec<f32>, String> {
    if offset < 0.0 || window < 0.0 {
        return Err("--offset / --window は 0 以上が必要です".into());
    }
    let start = (offset * sr).round() as usize;
    if start >= samples.len() {
        return Err(format!(
            "--offset {offset} s は信号長 {:.3} s を超えています",
            samples.len() as f64 / sr
        ));
    }
    let end = if window > 0.0 {
        (start + (window * sr).round() as usize).min(samples.len())
    } else {
        samples.len()
    };
    Ok(samples[start..end].to_vec())
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
            "--in" => args.input = PathBuf::from(value()?),
            "--channel" => args.channel = parse_channel(&value()?)?,
            "--window" => args.window = parse_f64(&value()?, "--window")?,
            "--offset" => args.offset = parse_f64(&value()?, "--offset")?,
            "--f0" => args.want_f0 = true,
            "--t60" => args.want_t60 = true,
            "--freq" => args.freqs.push(parse_f64(&value()?, "--freq")?),
            "--partials" => args.partials = Some(parse_f64(&value()?, "--partials")?),
            "--count" => args.count = parse_usize(&value()?, "--count")?,
            "--scan-cents" => args.scan_cents = parse_f64(&value()?, "--scan-cents")?,
            "--partial-t60" => args.partial_t60 = true,
            "--t60-window" => args.t60_window = parse_f64(&value()?, "--t60-window")?,
            "--t60-hop" => args.t60_hop = parse_f64(&value()?, "--t60-hop")?,
            other => return Err(format!("不明な引数: {other}")),
        }
    }

    if args.input.as_os_str().is_empty() {
        return Err("--in は必須です (-h でヘルプ)".into());
    }
    if args.partial_t60 && args.partials.is_none() {
        return Err("--partial-t60 は --partials と併せて使います".into());
    }
    Ok(Some(args))
}

fn parse_channel(s: &str) -> Result<Channel, String> {
    if s == "mix" {
        return Ok(Channel::Mix);
    }
    s.parse::<usize>()
        .map(Channel::One)
        .map_err(|_| format!("--channel は mix か番号です: {s}"))
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

    fn wav_of(channels: Vec<Vec<f32>>) -> Wav {
        Wav {
            channels,
            sample_rate: 48_000.0,
        }
    }

    #[test]
    fn window_slices_from_the_offset() {
        let x: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let w = slice_window(&x, 48_000.0, 0.5, 0.25).unwrap();
        assert_eq!(w.len(), 12_000);
        assert_eq!(w[0], 24_000.0);
    }

    #[test]
    fn window_zero_means_to_the_end() {
        let x: Vec<f32> = vec![1.0; 1000];
        assert_eq!(slice_window(&x, 1000.0, 0.5, 0.0).unwrap().len(), 500);
    }

    #[test]
    fn window_beyond_the_signal_is_an_error() {
        let x: Vec<f32> = vec![1.0; 100];
        assert!(slice_window(&x, 1000.0, 1.0, 0.0).is_err());
    }

    #[test]
    fn channel_selection_picks_the_right_side() {
        let w = wav_of(vec![vec![1.0, 1.0], vec![-1.0, -1.0]]);
        assert_eq!(select_channel(&w, Channel::One(0)).unwrap()[0], 1.0);
        assert_eq!(select_channel(&w, Channel::One(1)).unwrap()[0], -1.0);
        // mix は平均なので打ち消し合う。
        assert_eq!(select_channel(&w, Channel::Mix).unwrap()[0], 0.0);
        assert!(select_channel(&w, Channel::One(2)).is_err());
    }

    #[test]
    fn parse_args_requires_input() {
        assert!(parse_args(vec!["--t60".into()]).is_err());
    }

    #[test]
    fn parse_args_rejects_partial_t60_without_partials() {
        let e =
            parse_args(vec!["--in".into(), "x.wav".into(), "--partial-t60".into()]).unwrap_err();
        assert!(e.contains("--partials"), "{e}");
    }

    #[test]
    fn parse_args_collects_repeated_freqs() {
        let a = parse_args(vec![
            "--in".into(),
            "x.wav".into(),
            "--freq".into(),
            "440".into(),
            "--freq".into(),
            "880".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.freqs, vec![440.0, 880.0]);
    }

    #[test]
    fn parse_channel_accepts_mix_and_index() {
        assert_eq!(parse_channel("mix").unwrap(), Channel::Mix);
        assert_eq!(parse_channel("1").unwrap(), Channel::One(1));
        assert!(parse_channel("left").is_err());
    }

    #[test]
    fn parse_args_reports_help() {
        assert!(parse_args(vec!["--help".into()]).unwrap().is_none());
    }
}
