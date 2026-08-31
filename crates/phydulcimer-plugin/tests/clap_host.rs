//! CLAP の ABI 経由でプラグインを検証する。
//!
//! # なぜこれが要るか
//!
//! `phydulcimer-core` のテストは Rust の関数として DSP を叩いているだけで、
//! **CLAP の C ABI を一切通らない**。以下はここでしか検証できない:
//!
//! - エントリポイントとファクトリが正しく公開されているか
//! - ディスクリプタ (ID・名前・機能) がホストから読めるか
//! - `audio-ports` / `note-ports` / `params` 拡張がネゴシエートできるか
//! - ホストが投げたノートイベントが本当に音になるか
//! - **ノートオフで音が切れない**か (この楽器の定義)
//! - `process` がヒープ確保をしないか (`assert_no_alloc`)
//!
//! # ファイルを読み込まない理由
//!
//! ビルド済みの `.clap` を `dlopen` するとプロファイルや拡張子の違いでパス解決が
//! 脆くなる。clack は [`PluginEntry::load_from_clack`] で **`clack_export_entry!` が
//! 作るエントリを直接ホストへ渡せる**ので、ABI 経路はそのままにファイル探索だけを
//! 省ける (PhyPiano で確立した方法)。

use assert_no_alloc::{assert_no_alloc, AllocDisabler};
use clack_extensions::audio_ports::{AudioPortInfoBuffer, AudioPortType, PluginAudioPorts};
use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_host::events::event_types::{NoteChokeEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use clack_plugin::entry::prelude::SinglePluginEntry;

use phydulcimer_plugin::{params as plugin_params, PhyDulcimerPlugin, PLUGIN_ID, PLUGIN_NAME};

/// `process` の中でヒープ確保が起きたらテストを落とす。
///
/// このテストバイナリ全体のアロケータを差し替える。検査が働くのは
/// `assert_no_alloc(..)` で包んだ区間だけで、それ以外は素通しになる。
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK: usize = 256;

/// 入力ポートを持たないプラグインへ渡す「空の入力バッファ」の型。
type NoInputBuffers = [AudioPortBuffer<
    std::iter::Empty<InputChannel<'static, f32>>,
    std::iter::Empty<InputChannel<'static, f64>>,
>; 0];

// ---------------------------------------------------------------------------
// 最小のホスト実装
// ---------------------------------------------------------------------------

struct TestHostShared;

impl SharedHandler<'_> for TestHostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct TestHost;

impl HostHandlers for TestHost {
    type Shared<'a> = TestHostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

fn load_entry() -> PluginEntry {
    PluginEntry::load_from_clack::<SinglePluginEntry<PhyDulcimerPlugin>>(c"PhyDulcimer.clap")
        .expect("エントリを読み込めること")
}

fn host_info() -> HostInfo {
    HostInfo::new(
        "PhyDulcimer Test Host",
        "PhyDulcimer",
        "https://example.invalid",
        "0.1.0",
    )
    .expect("ホスト情報を作れること")
}

/// 発音させて音を集めるための一式。
struct Rig {
    instance: PluginInstance<TestHost>,
}

impl Rig {
    fn new() -> Self {
        let entry = Box::leak(Box::new(load_entry()));
        let instance = PluginInstance::<TestHost>::new(
            |_| TestHostShared,
            |_| (),
            entry,
            c"jp.ty17.phydulcimer",
            &host_info(),
        )
        .expect("プラグインを生成できること");

        Self { instance }
    }

    /// イベントを与えて `blocks` ブロック処理し、左右のチャンネルを返す。
    ///
    /// `process` の呼び出しは `assert_no_alloc` で包んである。**プラグインの
    /// オーディオ経路でヒープ確保が起きたら、その場でテストが落ちる。**
    fn render(
        &mut self,
        blocks: usize,
        mut events_at_block: impl FnMut(usize, &mut EventBuffer),
    ) -> (Vec<f32>, Vec<f32>) {
        let config = PluginAudioConfiguration {
            sample_rate: SAMPLE_RATE,
            min_frames_count: 1,
            max_frames_count: BLOCK as u32,
        };
        let processor = self
            .instance
            .activate(|_, _| (), config)
            .expect("アクティベートできること");
        let mut processor = processor.start_processing().expect("処理を開始できること");

        let mut input_ports = AudioPorts::with_capacity(0, 0);
        let mut output_ports = AudioPorts::with_capacity(2, 1);
        let mut block_l = vec![0.0f32; BLOCK];
        let mut block_r = vec![0.0f32; BLOCK];

        let mut left = Vec::with_capacity(blocks * BLOCK);
        let mut right = Vec::with_capacity(blocks * BLOCK);
        let mut in_events = EventBuffer::new();
        let mut out_events = EventBuffer::new();

        for b in 0..blocks {
            in_events.clear();
            events_at_block(b, &mut in_events);
            out_events.clear();
            block_l.fill(0.0);
            block_r.fill(0.0);

            {
                let no_inputs: NoInputBuffers = [];
                let input_audio = input_ports.with_input_buffers(no_inputs);
                let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        [&mut block_l[..], &mut block_r[..]].into_iter(),
                    ),
                }]);

                // イベントバッファの構築 (確保しうる) は外、process だけを包む。
                assert_no_alloc(|| {
                    processor
                        .process(
                            &input_audio,
                            &mut output_audio,
                            &InputEvents::from_buffer(&in_events),
                            &mut OutputEvents::from_buffer(&mut out_events),
                            Some((b * BLOCK) as u64),
                            None,
                        )
                        .expect("処理が成功すること");
                });
            }

            left.extend_from_slice(&block_l);
            right.extend_from_slice(&block_r);
        }

        let stopped = processor.stop_processing();
        self.instance.deactivate(stopped);

        (left, right)
    }
}

// ---------------------------------------------------------------------------
// 測定の補助
// ---------------------------------------------------------------------------

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |a, &b| a.max(b.abs()))
}

/// 指定周波数の振幅 (Goertzel 法)。
fn magnitude_at(x: &[f32], freq_hz: f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let w = std::f64::consts::TAU * freq_hz / SAMPLE_RATE;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
        wsum += win;
        let s0 = v as f64 * win + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let re = s1 - s2 * w.cos();
    let im = s2 * w.sin();
    2.0 * (re * re + im * im).sqrt() / wsum
}

fn push_note_on(buf: &mut EventBuffer, time: u32, key: u16, velocity: f64) {
    buf.push(&NoteOnEvent::new(
        time,
        Pckn::new(0u16, 0u16, key, 0u32),
        velocity,
    ));
}

fn push_note_off(buf: &mut EventBuffer, time: u32, key: u16) {
    buf.push(&NoteOffEvent::new(
        time,
        Pckn::new(0u16, 0u16, key, 0u32),
        0.0,
    ));
}

fn push_param(buf: &mut EventBuffer, id: u32, value: f64) {
    buf.push(&ParamValueEvent::new(
        0,
        ClapId::from_raw(id).unwrap(),
        Pckn::match_all(),
        value,
    ));
}

// ---------------------------------------------------------------------------
// エントリ・ディスクリプタ
// ---------------------------------------------------------------------------

#[test]
fn entry_exposes_exactly_one_plugin() {
    let entry = load_entry();
    let factory = entry
        .get_factory::<PluginFactory>()
        .expect("プラグインファクトリがあること");
    assert_eq!(factory.plugin_count(), 1);
}

#[test]
fn descriptor_matches_the_published_identity() {
    let entry = load_entry();
    let factory = entry.get_factory::<PluginFactory>().unwrap();
    let desc = factory
        .plugin_descriptor(0)
        .expect("ディスクリプタがあること");

    // ID はホストの設定やプリセットが紐づくので、公開後に変えてはいけない。
    assert_eq!(desc.id().unwrap().to_str().unwrap(), PLUGIN_ID);
    assert_eq!(desc.name().unwrap().to_str().unwrap(), PLUGIN_NAME);

    let features: Vec<String> = desc
        .features()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    assert!(
        features.iter().any(|f| f == "instrument"),
        "instrument が宣言されていない: {features:?}"
    );
}

#[test]
fn plugin_instantiates_and_deactivates_cleanly() {
    let mut rig = Rig::new();
    let (l, r) = rig.render(4, |_, _| {});
    assert_eq!(l.len(), 4 * BLOCK);
    assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
}

// ---------------------------------------------------------------------------
// 拡張のネゴシエーション
// ---------------------------------------------------------------------------

#[test]
fn audio_ports_extension_reports_one_stereo_output() {
    let mut rig = Rig::new();
    let handle = rig.instance.plugin_handle();
    let ext = handle
        .get_extension::<PluginAudioPorts>()
        .expect("audio-ports 拡張があること");
    let mt = &handle;

    assert_eq!(ext.count(mt, true), 0, "入力ポートは持たない");
    assert_eq!(ext.count(mt, false), 1, "出力ポートは 1 本");

    let mut buf = AudioPortInfoBuffer::new();
    let info = ext
        .get(mt, 0, false, &mut buf)
        .expect("出力ポートの情報が読めること");
    // ROOM (Phase 6) が来る前から 2ch。後からチャンネル数を変えると
    // ホストの接続が壊れる。
    assert_eq!(info.channel_count, 2, "ステレオであること");
    assert_eq!(info.port_type, Some(AudioPortType::STEREO));
}

#[test]
fn note_ports_extension_accepts_clap_and_midi() {
    let mut rig = Rig::new();
    let handle = rig.instance.plugin_handle();
    let ext = handle
        .get_extension::<PluginNotePorts>()
        .expect("note-ports 拡張があること");
    let mt = &handle;

    assert_eq!(ext.count(mt, true), 1, "ノート入力は 1 本");

    let mut buf = NotePortInfoBuffer::new();
    let info = ext
        .get(mt, 0, true, &mut buf)
        .expect("ノートポートの情報が読めること");

    // Phase 7 のミュート CC は MIDI で来る。
    assert!(
        info.supported_dialects.contains(NoteDialects::MIDI),
        "MIDI ダイアレクトを受けられない"
    );
    assert!(info.supported_dialects.contains(NoteDialects::CLAP));
}

#[test]
fn params_extension_exposes_every_parameter() {
    let mut rig = Rig::new();
    let handle = rig.instance.plugin_handle();
    let ext = handle
        .get_extension::<PluginParams>()
        .expect("params 拡張があること");
    let mt = &handle;

    let count = ext.count(mt) as usize;
    assert_eq!(
        count,
        plugin_params::PARAMS.len(),
        "公開しているパラメータ数が食い違う"
    );

    let mut info_buf = ParamInfoBuffer::new();
    for i in 0..count {
        let info = ext
            .get_info(mt, i as u32, &mut info_buf)
            .unwrap_or_else(|| panic!("パラメータ {i} の情報が読めない"));

        assert!(
            info.flags.contains(ParamInfoFlags::IS_AUTOMATABLE),
            "パラメータ {i} がオートメーションできない"
        );
        assert!(
            info.min_value < info.max_value,
            "パラメータ {i} の範囲が不正"
        );
        assert!(
            (info.min_value..=info.max_value).contains(&info.default_value),
            "パラメータ {i} の既定値が範囲外"
        );

        let value = ext
            .get_value(mt, info.id)
            .unwrap_or_else(|| panic!("パラメータ {i} の値が読めない"));
        assert!(
            (info.min_value..=info.max_value).contains(&value),
            "パラメータ {i} の現在値 {value} が範囲外"
        );
    }
}

#[test]
fn parameter_values_render_as_text() {
    let mut rig = Rig::new();
    let handle = rig.instance.plugin_handle();
    let ext = handle.get_extension::<PluginParams>().unwrap();
    let mt = &handle;

    let mut info_buf = ParamInfoBuffer::new();
    let mut text = [0u8; 128];
    for i in 0..ext.count(mt) {
        let info = ext.get_info(mt, i, &mut info_buf).unwrap();
        let written = ext
            .value_to_text(mt, info.id, info.default_value, &mut text)
            .unwrap_or_else(|_| panic!("パラメータ {i} の表示文字列を作れない"));
        assert!(!written.is_empty(), "パラメータ {i} の表示文字列が空");
        assert!(std::str::from_utf8(written).is_ok());
    }
}

// ---------------------------------------------------------------------------
// 発音
// ---------------------------------------------------------------------------

#[test]
fn note_on_produces_sound_through_the_clap_boundary() {
    let mut rig = Rig::new();
    let blocks = (SAMPLE_RATE as usize) / BLOCK;
    let (left, right) = rig.render(blocks, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 69, 0.8);
        }
    });

    assert!(peak(&left) > 0.01, "音が出ていない: peak {}", peak(&left));
    assert!(left.iter().all(|s| s.is_finite()));
    assert!(right.iter().all(|s| s.is_finite()));
    assert!(peak(&left) < 1.0, "単音で出力が過大: {}", peak(&left));
}

#[test]
fn rendered_pitch_matches_equal_temperament() {
    let mut rig = Rig::new();
    let blocks = (SAMPLE_RATE as usize) / BLOCK;
    let (left, _) = rig.render(blocks, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 69, 0.8);
        }
    });

    // 立ち上がりを避けて測る。
    let window = &left[BLOCK * 8..BLOCK * 40];
    let on_pitch = magnitude_at(window, 440.0);
    let off_pitch = magnitude_at(window, 466.16); // 半音上 (A#4)

    assert!(
        on_pitch > off_pitch * 10.0,
        "A4 が 440 Hz で鳴っていない: 440 Hz {on_pitch:.4e}, 466 Hz {off_pitch:.4e}"
    );
}

#[test]
fn both_channels_carry_the_signal() {
    let mut rig = Rig::new();
    let (left, right) = rig.render(40, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.8);
        }
    });
    assert!(peak(&left) > 0.001);
    assert!(peak(&right) > 0.001);
    // ROOM (Phase 6) までは L = R。分かれたらこのテストをそちらの検証に変える。
    assert_eq!(left, right, "Phase 6 より前に L/R が分かれている");
}

#[test]
fn velocity_changes_the_level() {
    let level = |velocity: f64| {
        let mut rig = Rig::new();
        let (left, _) = rig.render(60, |b, ev| {
            if b == 0 {
                push_note_on(ev, 0, 60, velocity);
            }
        });
        peak(&left)
    };
    let soft = level(0.2);
    let loud = level(1.0);
    assert!(
        loud > soft * 1.5,
        "ベロシティが効いていない: {soft} → {loud}"
    );
}

/// **この楽器の定義そのもの。** ノートオフで音が切れない。
///
/// PhyPiano の同名テストは「離鍵後しばらくで止まる」を検証するが、
/// ダルシマーにはダンパーが無いので、ノートオフの前後で減衰の速さが
/// 変わらないことを検証する。
#[test]
fn note_off_does_not_stop_the_ring() {
    let mut rig = Rig::new();
    let release_block = 40;
    let blocks = 120;

    let (left, _) = rig.render(blocks, move |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.9);
        } else if b == release_block {
            push_note_off(ev, 0, 60);
        }
    });

    let at = |block: usize| peak(&left[block * BLOCK..(block + 1) * BLOCK]);

    let before_release = at(release_block - 1);
    let well_after = at(release_block + 30); // 離鍵の約 160 ms 後

    assert!(before_release > 0.001, "離鍵前に鳴っていない");
    // T60 ≈ 10 秒の弦は 160 ms でほぼ減らない。半分より上なら鳴り続けている。
    assert!(
        well_after > before_release * 0.5,
        "ノートオフで音が切れている: {before_release:.4e} → {well_after:.4e}"
    );
}

#[test]
fn choke_stops_the_ring_immediately() {
    // ホストの停止・シークで送られる消音イベント。これが効かないと
    // 再生を止めても音が残り続ける。
    let mut rig = Rig::new();
    let choke_block = 40;
    let (left, _) = rig.render(80, move |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.9);
        } else if b == choke_block {
            ev.push(&NoteChokeEvent::new(0, Pckn::new(0u16, 0u16, 60u16, 0u32)));
        }
    });

    let before = peak(&left[(choke_block - 1) * BLOCK..choke_block * BLOCK]);
    let after = peak(&left[(choke_block + 2) * BLOCK..(choke_block + 3) * BLOCK]);
    assert!(before > 0.001);
    assert!(
        after < before * 0.01,
        "choke が効いていない: {before:.4e} → {after:.4e}"
    );
}

#[test]
fn out_of_range_keys_are_silent_and_harmless() {
    let mut rig = Rig::new();
    let (left, _) = rig.render(20, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 21, 1.0); // A0 — 範囲外
            push_note_on(ev, 0, 108, 1.0); // C8 — 範囲外
        }
    });
    assert_eq!(peak(&left), 0.0, "範囲外の鍵で音が出た");
}

#[test]
fn strike_position_parameter_moves_the_notch() {
    // params → 楽器 → 音、の経路が ABI 越しに繋がっていることの検証。
    // x/L = 0.25 で叩けば第 4 部分音が節に当たって消える。
    let partial4 = |strike: f64| {
        let mut rig = Rig::new();
        let (left, _) = rig.render(80, move |b, ev| {
            if b == 0 {
                push_param(ev, plugin_params::id::STRIKE_POSITION, strike);
                push_note_on(ev, 0, 60, 0.8);
            }
        });
        let f0 = 261.63;
        let window = &left[BLOCK * 8..BLOCK * 72];
        // 部分音はインハーモニシティで伸びるので、走査して最大を採る。
        (0..=40)
            .map(|c| {
                let f = 4.0 * f0 * (1.0 + c as f64 / 4000.0);
                magnitude_at(window, f)
            })
            .fold(0.0f64, f64::max)
    };

    let normal = partial4(0.09);
    let notched = partial4(0.25);
    assert!(
        notched < normal * 0.05,
        "打弦点パラメータが効いていない: 0.09 → {normal:.4e}, 0.25 → {notched:.4e}"
    );
}

#[test]
fn level_parameter_scales_the_output() {
    let level_of = |value: f64| {
        let mut rig = Rig::new();
        let (left, _) = rig.render(40, move |b, ev| {
            if b == 0 {
                push_param(ev, plugin_params::id::LEVEL, value);
                push_note_on(ev, 0, 60, 0.8);
            }
        });
        peak(&left)
    };
    let half = level_of(0.35);
    let full = level_of(0.7);
    assert!(
        (full / half - 2.0).abs() < 0.1,
        "Level が線形に効いていない: {half:.4e} → {full:.4e}"
    );
}

#[test]
fn all_strings_at_once_stay_stable() {
    let mut rig = Rig::new();
    let (left, right) = rig.render(160, |b, ev| {
        if b == 0 {
            for key in 43..=86u16 {
                push_note_on(ev, 0, key, 1.0);
            }
        }
    });

    assert!(
        left.iter().chain(right.iter()).all(|s| s.is_finite()),
        "全 44 弦の同時打弦で非有限値が出た"
    );

    // 打撃の過渡がピークで、以降は増え続けない (発散の兆候が無い)。
    let attack = peak(&left[..BLOCK * 16]);
    let later = peak(&left[BLOCK * 120..]);
    assert!(attack > 0.0, "音が出ていない");
    assert!(
        later < attack,
        "時間とともに出力が増えている: 打鍵直後 {attack:.3} → 後半 {later:.3}"
    );
}

#[test]
fn processing_can_be_restarted() {
    let mut rig = Rig::new();
    let (a, _) = rig.render(20, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.8);
        }
    });
    // stop_processing → 再 activate。ホストのトランスポート停止と再開に相当。
    let (b, _) = rig.render(20, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.8);
        }
    });
    assert!(peak(&a) > 0.001);
    assert!(peak(&b) > 0.001);
}

/// **鳴っている弦がループの次の周回へ持ち越されない。**
///
/// ダンパーが無い楽器は T60 が 10 秒を超える。DAW のループ (数秒) で折り返す
/// たびに前の周回の響きが残ると、周回ごとに音が積み上がっていく。ホストは
/// 折り返しで `stop_processing` を呼ぶので、そこで全弦が止まることを検証する。
/// PhyPiano の P-036 (ペダルの持ち越し) と同じ形の罠。
#[test]
fn ringing_strings_do_not_survive_a_loop_restart() {
    let mut rig = Rig::new();
    // 1 周目: 打鍵して鳴らしたまま終わる。
    let (first, _) = rig.render(40, |b, ev| {
        if b == 0 {
            push_note_on(ev, 0, 60, 0.9);
        }
    });
    assert!(
        peak(&first[BLOCK * 38..]) > 0.001,
        "1 周目の最後に鳴っていない"
    );

    // 2 周目: 何も弾かない。前の周回の響きが聴こえたら持ち越されている。
    let (second, _) = rig.render(40, |_, _| {});
    assert_eq!(
        peak(&second),
        0.0,
        "前の周回の響きがループを越えて残っている"
    );
}
