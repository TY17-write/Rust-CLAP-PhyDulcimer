//! オフラインレンダラ。
//!
//! 3 つのモードがある。
//!
//! - 既定 — 減衰正弦波。`analyze` が設計値どおりに読めるかを確かめる疎通確認 (Phase 0)
//! - `--string` — 弦の 1 区間を鳴らす (Phase 1)
//! - `--contact-table` — 撥の接触時間の表を出す。音は出ない
//!
//! 弦のモデルは `phydulcimer-core` にある。ここは引数を読んで WAV を書くだけ。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phydulcimer_core::hammer::{Hammer, HammerFace, HammerParams, HammerState};
use phydulcimer_core::segment::{DampingParams, Decimation, Segment, SegmentParams};
use phydulcimer_core::{smoke::DecayingSine, Sample, DEFAULT_SAMPLE_RATE};

const USAGE: &str = "\
phydulcimer-render — PhyDulcimer offline renderer

USAGE:
    phydulcimer-render --out <PATH> [OPTIONS]
    phydulcimer-render --contact-table [OPTIONS]

MODES:
    (default)              decaying sine, for checking the analysis path
    --string               render one string segment (Phase 1)
    --instrument           render the whole instrument (Phase 4)
    --contact-table        print hammer contact times; writes no audio
    --table                print the 15/14 design table (44 positions); no audio

INSTRUMENT (--instrument):
    --key <MIDI>           strike this key at t=0 (repeatable)
    --vel <0..1>           note velocity (NOT m/s in this mode)   [default: 0.8]
    --strike <0..0.5>      strike point x/L                       [default: 0.09]
    --no-coupling          disconnect the treble bridge coupling (A/B)

COMMON:
    --out <PATH>           write the WAV here      [required except --contact-table]
    --dur <SEC>            length                                 [default: 3.0]
    --sample-rate <HZ>     sample rate                            [default: 48000]
    --peak <0..1>          normalise to this peak; 0 = don't      [default: 0]

DECAYING SINE:
    --freq <HZ>            fundamental frequency                  [default: 440]
    --t60 <SEC>            60 dB decay time; 0 = no decay         [default: 2.0]
    --amp <A>              amplitude of the fundamental           [default: 0.5]
    --partials <N>         stack N partials (amplitude 1/n)       [default: 1]
    --inharmonicity <B>    place partials at n*f0*sqrt(1+B*n^2)   [default: 0]
    --t60-taper <R>        partial n decays with t60 / n^R        [default: 0]

STRING (--string):
    --segment <NAME>       treble-long | treble-short             [default: treble-long]
    --strike <0..0.5>      strike point x/L                       [default: 0.09]
    --vel <M/S>            hammer speed at the string             [default: 2.0]
    --face <NAME>          wood | leather | felt                  [default: wood]
    --os <N>               oversampling while in contact          [default: 16]
    --decimate <MODE>      drop | average                         [default: drop]
    --modes <N>            cap the partial count; 0 = no cap      [default: 0]
    --stiffness <K>        override the hammer stiffness (sweeps)
    --t60-low <SEC>        T60 at the fundamental                 [default: 10.0]
    --t60-high <SEC>       T60 at 5 kHz                           [default: 0.8]

    -h, --help             show this help

NOTE:
    --peak defaults to 0 (no normalisation). PhyPiano defaulted it to 0.9 and
    every A/B comparison silently lost its level difference unless --peak 0 was
    passed. Normalise only when the absolute level does not matter.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sine,
    String,
    ContactTable,
    /// 44 発音位置の設計表を出す。音は出ない。
    DesignTable,
    /// 楽器全体 (全弦常時走行) を鳴らす。
    Instrument,
}

#[derive(Debug, Clone)]
struct Args {
    mode: Mode,
    out: PathBuf,
    dur: f64,
    sample_rate: f64,
    peak: f64,

    // 減衰正弦波
    freq: f64,
    t60: f64,
    amp: f64,
    partials: usize,
    inharmonicity: f64,
    t60_taper: f64,

    // 弦
    segment: SegmentKind,
    strike: f64,
    vel: f64,
    face: HammerFace,
    oversample: usize,
    decimation: Decimation,
    modes: usize,
    stiffness: Option<f64>,
    t60_low: f64,
    t60_high: f64,

    // 楽器全体
    keys: Vec<u8>,
    no_coupling: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SegmentKind {
    #[default]
    TrebleLong,
    TrebleShort,
}

impl SegmentKind {
    fn params(self) -> SegmentParams {
        match self {
            SegmentKind::TrebleLong => SegmentParams::treble_low_long(),
            SegmentKind::TrebleShort => SegmentParams::treble_low_short(),
        }
    }
}

impl Default for Args {
    fn default() -> Self {
        Self {
            mode: Mode::Sine,
            out: PathBuf::new(),
            dur: 3.0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            // 既定は「正規化しない」。理由は USAGE の NOTE を参照。
            peak: 0.0,

            freq: 440.0,
            t60: 2.0,
            amp: 0.5,
            partials: 1,
            inharmonicity: 0.0,
            t60_taper: 0.0,

            segment: SegmentKind::default(),
            strike: 0.09,
            vel: 2.0,
            face: HammerFace::Wood,
            // Segment の既定 (16 倍) と揃える。8 以下は収束していない (D-010)。
            oversample: 16,
            decimation: Decimation::Drop,
            modes: 0,
            stiffness: None,
            t60_low: 10.0,
            t60_high: 0.8,

            keys: Vec::new(),
            no_coupling: false,
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

    if args.mode == Mode::ContactTable {
        print_contact_table(&args);
        return Ok(());
    }
    if args.mode == Mode::DesignTable {
        print_design_table(args.sample_rate);
        return Ok(());
    }

    let (buf, note) = match args.mode {
        Mode::Sine => (render_sine(&args)?, String::new()),
        Mode::String => render_string(&args)?,
        Mode::Instrument => render_instrument(&args)?,
        Mode::ContactTable | Mode::DesignTable => unreachable!(),
    };

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
        "wrote {} ({:.3} s @ {} Hz, raw peak {:.4})",
        args.out.display(),
        buf.len() as f64 / args.sample_rate,
        args.sample_rate,
        raw_peak
    );
    if !note.is_empty() {
        println!("{note}");
    }
    Ok(())
}

/// 部分音を重ねた減衰正弦波。
fn render_sine(args: &Args) -> Result<Vec<Sample>, String> {
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
        let t60 = if args.t60 > 0.0 {
            args.t60 / kf.powf(args.t60_taper)
        } else {
            f64::INFINITY
        };
        DecayingSine::new(freq, t60, args.sample_rate, args.amp / kf).add_to(&mut buf);
    }

    Ok(buf)
}

/// 弦の 1 区間を打弦してレンダリングする。
fn render_string(args: &Args) -> Result<(Vec<Sample>, String), String> {
    if args.dur <= 0.0 {
        return Err(format!("--dur は正の値が必要です: {}", args.dur));
    }

    let params = args.segment.params();
    let mut seg = Segment::new(params, args.sample_rate);

    let mut hammer = HammerParams::for_face(args.face);
    if let Some(k) = args.stiffness {
        hammer.stiffness = k;
    }
    seg.set_hammer_params(hammer);

    seg.set_oversample(args.oversample);
    seg.set_decimation(args.decimation);
    seg.set_mode_limit(args.modes);
    seg.set_damping(DampingParams::from_t60_anchors(
        params.f0_hz,
        args.t60_low,
        5_000.0,
        args.t60_high,
    ));
    seg.set_strike_ratio(args.strike);

    let n = (args.dur * args.sample_rate).round() as usize;
    let mut buf = vec![0.0 as Sample; n];
    seg.strike(args.vel);
    seg.process_block(&mut buf);

    let note = format!(
        "segment  L = {:.1} mm, f0 = {:.2} Hz, d = {:.3} mm\n\
         design   T = {:.1} N, stress = {:.0} MPa, B = {:.3e}, modes = {}\n\
         hammer   {:?} K = {:.3e}, contact = {:.3} ms (os = {}x, {:?})\n\
         strike   x/L = {:.3}  (first notch at partial {:.1})",
        params.length_m * 1000.0,
        params.f0_hz,
        params.diameter_m * 1000.0,
        params.tension(),
        params.stress_pa() / 1e6,
        params.inharmonicity(),
        seg.partial_count(),
        args.face,
        hammer.stiffness,
        seg.hammer().contact_duration() * 1000.0,
        args.oversample,
        args.decimation,
        seg.strike_ratio(),
        1.0 / seg.strike_ratio(),
    );

    Ok((buf, note))
}

/// 楽器全体を鳴らす (Phase 4)。出力はブリッジ力の和 (モノ、校正前)。
fn render_instrument(args: &Args) -> Result<(Vec<Sample>, String), String> {
    use phydulcimer_core::instrument::Instrument;

    if args.dur <= 0.0 {
        return Err(format!("--dur は正の値が必要です: {}", args.dur));
    }
    if args.keys.is_empty() {
        return Err("--instrument には --key <MIDI> が最低 1 つ必要です".into());
    }

    let mut inst = Instrument::new(args.sample_rate);
    inst.set_strike_ratio(args.strike);
    if args.no_coupling {
        inst.set_bridge_coupling(0.0);
    }

    // --instrument の --vel は MIDI 的な 0–1 (--string の m/s とは違う)。
    let velocity = args.vel.clamp(0.0, 1.0);
    for &key in &args.keys {
        inst.note_on(key, velocity);
    }

    let n = (args.dur * args.sample_rate).round() as usize;
    let mut buf = vec![0.0 as Sample; n];
    for chunk in buf.chunks_mut(64) {
        inst.process(chunk);
    }

    let note = format!(
        "instrument  keys {:?}, vel {:.2}, strike {:.3}, coupling {}",
        args.keys,
        velocity,
        args.strike,
        if args.no_coupling { "OFF" } else { "on" },
    );
    Ok((buf, note))
}

/// 15/14 の設計表 (44 発音位置)。P3 の完了条件の確認に使う。
fn print_design_table(sample_rate: f64) {
    use phydulcimer_core::layout::{note_name, BridgeSide, Layout};
    use phydulcimer_core::scaling::{design_position, key_to_hz};

    let layout = Layout::standard_15_14();
    println!(
        "15/14 standard layout — {} speaking positions @ {} Hz",
        layout.positions().len(),
        sample_rate
    );
    println!(
        "{:<7} {:<3} {:<5} {:>4} {:>9} {:>7} {:>7} {:>5} {:>7} {:>7} {:>9} {:>6} {:>8}",
        "side",
        "crs",
        "note",
        "midi",
        "f0[Hz]",
        "L[mm]",
        "d[mm]",
        "wrap",
        "T[N]",
        "s[MPa]",
        "B",
        "modes",
        "T60f0[s]"
    );

    for p in layout.positions() {
        let (design, damping) = design_position(p);
        let params = design.segment_params();
        let side = match p.side {
            BridgeSide::Bass => "bass",
            BridgeSide::TrebleRight => "treb-R",
            BridgeSide::TrebleLeft => "treb-L",
        };
        // 左側は純正5度で 12 平均律から +2 cent。表では実周波数を出す。
        let cents = 1200.0 * (design.f0_hz / key_to_hz(p.midi)).log2();
        println!(
            "{:<7} {:<3} {:<5} {:>4} {:>9.2} {:>7.1} {:>7.3} {:>5.2} {:>7.1} {:>7.0} {:>9.2e} {:>6} {:>8.2}{}",
            side,
            p.course,
            note_name(p.midi),
            p.midi,
            design.f0_hz,
            design.speaking_m * 1000.0,
            design.diameter_m * 1000.0,
            design.wrap,
            params.tension(),
            params.stress_pa() / 1e6,
            params.inharmonicity(),
            params.mode_count(sample_rate),
            damping.t60_at(design.f0_hz),
            if cents.abs() > 0.5 { format!("  ({cents:+.1}c)") } else { String::new() },
        );
    }
    println!("\nwrap > 1 = wound string (mass multiplier on the steel core).");
    println!("(+2.0c) = pure fifth from the 2:3 bridge, vs 12-TET (D-017).");
}

/// 撥を剛体壁に当てて接触時間を測る。弦は関与しない。
///
/// **弦を鳴らして測ると、弦が撥を押し返すぶん接触が短くなる。** 撥そのものの
/// 係数を決めるときは壁で測るほうが切り分けやすい。
fn print_contact_table(args: &Args) {
    let faces = if args.stiffness.is_some() {
        vec![args.face]
    } else {
        vec![HammerFace::Wood, HammerFace::Leather, HammerFace::Felt]
    };

    println!(
        "contact times against a rigid wall (oversample {}x, dt = {:.3} us)",
        args.oversample,
        1e6 / (args.sample_rate * args.oversample as f64)
    );
    println!(
        "{:<9} {:>8} {:>12} {:>12} {:>12} {:>10}",
        "face", "vel", "contact[ms]", "steps", "peak[N]", "rebound"
    );

    for face in faces {
        let mut params = HammerParams::for_face(face);
        if let Some(k) = args.stiffness {
            params.stiffness = k;
        }
        for v in [0.5, 1.0, 2.0, 4.0, 6.0] {
            let (ms, peak, rebound) = strike_wall(params, v, args.sample_rate, args.oversample);
            let dt_us = 1e6 / (args.sample_rate * args.oversample as f64);
            println!(
                "{:<9} {:>8.1} {:>12.4} {:>12.1} {:>12.1} {:>10.2}",
                format!("{face:?}"),
                v,
                ms,
                ms * 1000.0 / dt_us,
                peak,
                rebound
            );
        }
    }
    println!(
        "\nsteps = how many integration steps the contact lasted.\n\
         Below ~10 the contact is not resolved and the result depends on --os."
    );
}

/// 剛体壁への打撃。返り値は (接触時間 [ms], 最大力 [N], 跳ね返り速度 [m/s])。
fn strike_wall(
    params: HammerParams,
    velocity: f64,
    sample_rate: f64,
    oversample: usize,
) -> (f64, f64, f64) {
    let dt = 1.0 / (sample_rate * oversample as f64);
    let mut h = Hammer::new(params);
    h.strike(velocity);

    let mut peak = 0.0f64;
    for _ in 0..(0.050 / dt) as usize {
        let f = h.step(0.0, 0.0, dt);
        peak = peak.max(f);
        if h.state() == HammerState::Released {
            break;
        }
    }

    // 反発係数 = 跳ね返り速度 / 打撃速度。1 に近いほど散逸が少ない。
    let restitution = if velocity > 0.0 {
        -h.velocity() / velocity
    } else {
        0.0
    };
    (h.contact_duration() * 1000.0, peak, restitution)
}

/// 32-bit float のモノ WAV を書く。
///
/// `phydulcimer-analyze` の lib にも同じものがある。共有すれば重複は消えるが、
/// **レンダラが解析ツールに依存する**という筋の悪い依存方向になる。30 行の
/// 重複のほうが安いと判断した (→ `docs/problems.md` D-003)。
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
            "--string" => args.mode = Mode::String,
            "--contact-table" => args.mode = Mode::ContactTable,
            "--table" => args.mode = Mode::DesignTable,
            "--instrument" => args.mode = Mode::Instrument,
            "--key" => args.keys.push(
                parse_usize(&value()?, "--key")?
                    .try_into()
                    .map_err(|_| "--key は 0–127 です".to_string())?,
            ),
            "--no-coupling" => args.no_coupling = true,

            "--out" => args.out = PathBuf::from(value()?),
            "--dur" => args.dur = parse_f64(&value()?, "--dur")?,
            "--sample-rate" => args.sample_rate = parse_f64(&value()?, "--sample-rate")?,
            "--peak" => args.peak = parse_f64(&value()?, "--peak")?,

            "--freq" => args.freq = parse_f64(&value()?, "--freq")?,
            "--t60" => args.t60 = parse_f64(&value()?, "--t60")?,
            "--amp" => args.amp = parse_f64(&value()?, "--amp")?,
            "--partials" => args.partials = parse_usize(&value()?, "--partials")?,
            "--inharmonicity" => args.inharmonicity = parse_f64(&value()?, "--inharmonicity")?,
            "--t60-taper" => args.t60_taper = parse_f64(&value()?, "--t60-taper")?,

            "--segment" => args.segment = parse_segment(&value()?)?,
            "--strike" => args.strike = parse_f64(&value()?, "--strike")?,
            "--vel" => args.vel = parse_f64(&value()?, "--vel")?,
            "--face" => args.face = parse_face(&value()?)?,
            "--os" => args.oversample = parse_usize(&value()?, "--os")?.max(1),
            "--decimate" => args.decimation = parse_decimation(&value()?)?,
            "--modes" => args.modes = parse_usize(&value()?, "--modes")?,
            "--stiffness" => args.stiffness = Some(parse_f64(&value()?, "--stiffness")?),
            "--t60-low" => args.t60_low = parse_f64(&value()?, "--t60-low")?,
            "--t60-high" => args.t60_high = parse_f64(&value()?, "--t60-high")?,

            other => return Err(format!("不明な引数: {other}")),
        }
    }

    let needs_out = !matches!(args.mode, Mode::ContactTable | Mode::DesignTable);
    if needs_out && args.out.as_os_str().is_empty() {
        return Err("--out は必須です (-h でヘルプ)".into());
    }
    Ok(Some(args))
}

fn parse_segment(s: &str) -> Result<SegmentKind, String> {
    match s {
        "treble-long" => Ok(SegmentKind::TrebleLong),
        "treble-short" => Ok(SegmentKind::TrebleShort),
        other => Err(format!(
            "--segment は treble-long | treble-short です: {other}"
        )),
    }
}

fn parse_face(s: &str) -> Result<HammerFace, String> {
    match s {
        "wood" => Ok(HammerFace::Wood),
        "leather" => Ok(HammerFace::Leather),
        "felt" => Ok(HammerFace::Felt),
        other => Err(format!("--face は wood | leather | felt です: {other}")),
    }
}

fn parse_decimation(s: &str) -> Result<Decimation, String> {
    match s {
        "drop" => Ok(Decimation::Drop),
        "average" => Ok(Decimation::Average),
        other => Err(format!("--decimate は drop | average です: {other}")),
    }
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

    fn sine_args(partials: usize) -> Args {
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
    fn sine_produces_the_requested_length() {
        let a = sine_args(1);
        let buf = render_sine(&a).expect("レンダリングできること");
        assert_eq!(buf.len(), (a.dur * a.sample_rate).round() as usize);
    }

    #[test]
    fn partials_are_stacked_with_falling_amplitude() {
        let one = render_sine(&sine_args(1)).unwrap();
        let four = render_sine(&sine_args(4)).unwrap();
        let peak = |b: &[Sample]| b.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert!(peak(&four) > peak(&one));
    }

    #[test]
    fn partials_above_nyquist_are_dropped() {
        let a = Args {
            freq: 20_000.0,
            partials: 8,
            ..sine_args(8)
        };
        let buf = render_sine(&a).expect("打ち切って成功すること");
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn zero_t60_means_no_decay() {
        let a = Args {
            t60: 0.0,
            dur: 1.0,
            ..sine_args(1)
        };
        let buf = render_sine(&a).unwrap();
        let head = buf[..1000].iter().fold(0.0f32, |x, &y| x.max(y.abs()));
        let tail = buf[buf.len() - 1000..]
            .iter()
            .fold(0.0f32, |x, &y| x.max(y.abs()));
        assert_relative_eq!(head, tail, max_relative = 1e-3);
    }

    #[test]
    fn string_mode_renders_audio() {
        let a = Args {
            mode: Mode::String,
            out: PathBuf::from("unused.wav"),
            dur: 0.2,
            ..Args::default()
        };
        let (buf, note) = render_string(&a).expect("レンダリングできること");
        assert_eq!(buf.len(), (a.dur * a.sample_rate).round() as usize);
        assert!(buf.iter().any(|&s| s.abs() > 0.0), "音が出ていない");
        assert!(buf.iter().all(|s| s.is_finite()));
        assert!(note.contains("segment"), "{note}");
    }

    #[test]
    fn stiffness_override_reaches_the_hammer() {
        // 柔らかくすれば接触が伸びる。掃引が効いていることの確認。
        let contact = |k: Option<f64>| -> f64 {
            let mut p = HammerParams::wood();
            if let Some(k) = k {
                p.stiffness = k;
            }
            strike_wall(p, 2.0, 48_000.0, 32).0
        };
        let hard = contact(Some(1.0e9));
        let soft = contact(Some(1.0e7));
        assert!(soft > hard, "hard={hard:.4} soft={soft:.4}");
    }

    #[test]
    fn parse_args_requires_out_except_for_the_table() {
        assert!(parse_args(vec!["--freq".into(), "440".into()]).is_err());
        // --contact-table は音を書かないので --out が要らない。
        assert!(parse_args(vec!["--contact-table".into()]).is_ok());
    }

    #[test]
    fn parse_args_reads_string_options() {
        let a = parse_args(vec![
            "--string".into(),
            "--out".into(),
            "x.wav".into(),
            "--face".into(),
            "felt".into(),
            "--os".into(),
            "16".into(),
            "--decimate".into(),
            "average".into(),
            "--segment".into(),
            "treble-short".into(),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(a.mode, Mode::String);
        assert_eq!(a.face, HammerFace::Felt);
        assert_eq!(a.oversample, 16);
        assert_eq!(a.decimation, Decimation::Average);
        assert_eq!(a.segment, SegmentKind::TrebleShort);
    }

    #[test]
    fn parse_args_rejects_unknown_values() {
        assert!(parse_face("plastic").is_err());
        assert!(parse_decimation("sinc").is_err());
        assert!(parse_segment("bass").is_err());
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
    fn peak_defaults_to_no_normalisation() {
        // ここが 0 でなくなったら A/B 比較が壊れる。意図的に固定する。
        assert_eq!(Args::default().peak, 0.0);
    }

    #[test]
    fn sine_rejects_degenerate_arguments() {
        assert!(render_sine(&Args {
            dur: 0.0,
            ..sine_args(1)
        })
        .is_err());
        assert!(render_sine(&Args {
            freq: 0.0,
            ..sine_args(1)
        })
        .is_err());
        assert!(render_sine(&Args {
            partials: 0,
            ..sine_args(1)
        })
        .is_err());
    }
}
