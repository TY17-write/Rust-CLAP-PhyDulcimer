//! PhyDulcimer の CLAP フロントエンド。
//!
//! [clack](https://github.com/prokopyl/clack) で `phydulcimer-core` の
//! [`DulcimerEngine`] をホストに繋ぐだけの薄い層。DSP はすべて core 側にあり、
//! この層はイベントの変換とパラメータの受け渡ししかしない。
//!
//! # スレッド分担
//!
//! - **メインスレッド**: ホストの UI・オートメーション。パラメータの読み書き
//! - **オーディオスレッド**: [`DulcimerEngine`] の実行。確保・ロック・I/O をしない
//!
//! 両者はパラメータをアトミック経由でのみやり取りする。
//!
//! # この楽器に固有の点
//!
//! - **ノートオフを捨てる。** ダンパーが無いので、離鍵しても弦は鳴り続ける。
//!   ホストの停止 (choke / reset) だけが弦を止める
//! - 出力は **2ch** (Phase 5 では L = R。Phase 6 の X-Y ROOM がここを分ける)
//!
//! # ビルド
//!
//! ```text
//! cargo build --release -p phydulcimer-plugin
//! copy target\release\phydulcimer_plugin.dll target\release\PhyDulcimer.clap
//! ```

// `forbid` にすると `clack_export_entry!` が展開する `allow(unsafe_code)` と
// 衝突する。このクレートで unsafe を書くのはそのマクロだけなので `deny` にする。
#![deny(unsafe_code)]

pub mod params;

use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::event_types::ParamValueEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::Match;
use clack_plugin::prelude::*;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::ops::Bound;

use params::ParamValues;
use phydulcimer_core::engine::DulcimerEngine;
use phydulcimer_core::hammer::HammerFace;
use phydulcimer_core::instrument::InstrumentConfig;
use phydulcimer_core::layout::LayoutKind;
use phydulcimer_core::room::{RoomParams, RoomSize};
use phydulcimer_core::scaling::Temperament;

/// CLAP プラグイン ID (逆ドメイン形式)。公開後は変更しないこと。
pub const PLUGIN_ID: &str = "jp.ty17.phydulcimer";

/// ホストの UI に出る名前。
pub const PLUGIN_NAME: &str = "PhyDulcimer";

/// プラグインのバージョン (Cargo のパッケージバージョンに追従)。
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 「鳴っていない」と判定する出力ピークの閾値。
///
/// **ボイス数では判定できない** — この楽器にはそもそもボイスが無く、全弦が
/// 常時走っている。「ボイス 0 で眠る」型の判定は、まだ鳴っている残響を凍結させて
/// ポップノイズを出す。ここでは実際の出力レベルだけを見る。
/// −120 dBFS を下回っていれば、状態が凍結しても聴こえない。
const SILENCE_THRESHOLD: f32 = 1e-6;

/// 眠るまでに要求する連続無音ブロック数。
///
/// 1 ブロックの偶発的な無音 (打撃直後のゼロクロス近傍など) で眠らないため。
const SILENT_BLOCKS_TO_SLEEP: u32 = 8;

/// MIDI CC 番号: Level (CC7 = チャンネルボリュームの慣例に合わせる)。
///
/// GUI (Phase 9) が入るまで、パラメータを操作する現実的な手段が
/// ホストのオートメーションしかない。CC でも触れるようにしておくと、
/// CC 段を持つホスト (egui-clap-host など) から確認できる。
const CC_LEVEL: u8 = 7;

/// MIDI CC 番号: Strike Position (CC74 = Brightness の慣例に合わせる)。
///
/// 0 でブリッジ寄り (x/L = 0.03、明るい)、127 で中央寄り (0.30、丸い)。
const CC_STRIKE: u8 = 74;

/// MIDI CC 番号: Hammer Face (CC70 = Sound Variation の慣例に合わせる)。
///
/// 奏者は演奏中に撥を持ち替えるので、CC で切り替えられる価値がある。
/// 0–42 = Wood / 43–84 = Leather / 85–127 = Felt の 3 分割。
const CC_FACE: u8 = 70;

/// MIDI CC 番号: Mute (CC1 = モジュレーションホイールの慣例に合わせる)。
///
/// パームミュートは演奏中に連続で操作するものなので、ホイールが自然。
/// 0 = 開放、127 = 押さえ切る。
const CC_MUTE: u8 = 1;

/// パラメータ値 (0–2、丸め) → 撥の面。
fn face_from_value(value: f32) -> HammerFace {
    match value.round() as i32 {
        1 => HammerFace::Leather,
        2 => HammerFace::Felt,
        _ => HammerFace::Wood,
    }
}

/// Layout / Temperament パラメータ → 楽器の構成 (activate 時に適用)。
fn config_from_params(params: &ParamValues) -> InstrumentConfig {
    InstrumentConfig {
        layout: if params.layout.load() >= 0.5 {
            LayoutKind::ChromaticE3E6
        } else {
            LayoutKind::Diatonic1514
        },
        temperament: if params.temperament.load() >= 0.5 {
            Temperament::Equal12
        } else {
            Temperament::PureFifth
        },
    }
}

pub struct PhyDulcimerPlugin;

impl Plugin for PhyDulcimerPlugin {
    type AudioProcessor<'a> = PhyDulcimerAudioProcessor<'a>;
    type Shared<'a> = PhyDulcimerShared;
    type MainThread<'a> = PhyDulcimerMainThread<'a>;

    fn declare_extensions(
        builder: &mut PluginExtensions<Self>,
        _shared: Option<&PhyDulcimerShared>,
    ) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>();
    }
}

impl DefaultPluginFactory for PhyDulcimerPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;

        PluginDescriptor::new(PLUGIN_ID, PLUGIN_NAME).with_features([
            SYNTHESIZER,
            INSTRUMENT,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<PhyDulcimerShared, PluginError> {
        Ok(PhyDulcimerShared {
            params: ParamValues::new(),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a PhyDulcimerShared,
    ) -> Result<PhyDulcimerMainThread<'a>, PluginError> {
        Ok(PhyDulcimerMainThread { shared })
    }
}

/// スレッド間で共有される状態。アトミックだけを持つ。
pub struct PhyDulcimerShared {
    params: ParamValues,
}

impl PhyDulcimerShared {
    fn handle_param_event(&self, event: &ParamValueEvent) {
        if let Some(id) = event.param_id() {
            self.params.set(u32::from(id), event.value());
        }
    }
}

impl PluginShared<'_> for PhyDulcimerShared {}

pub struct PhyDulcimerMainThread<'a> {
    shared: &'a PhyDulcimerShared,
}

impl<'a> PluginMainThread<'a, PhyDulcimerShared> for PhyDulcimerMainThread<'a> {}

/// オーディオスレッドで動くプロセッサ。
pub struct PhyDulcimerAudioProcessor<'a> {
    engine: DulcimerEngine,
    shared: &'a PhyDulcimerShared,
    /// ホストへの参照 (Layout/Temperament 変更時の `request_restart` 用。
    /// CLAP 仕様でスレッドセーフ、中身は関数ポインタ呼び出しのみ)
    host: HostAudioProcessorHandle<'a>,
    /// エンジンのステレオ出力を受けてからポートへ配る (事前確保)
    left: Vec<f32>,
    right: Vec<f32>,
    /// 連続で無音だったブロック数。[`SILENT_BLOCKS_TO_SLEEP`] に達したら眠る
    silent_blocks: u32,
    /// restart を一度だけ要求するためのラッチ (連打防止)
    restart_requested: bool,
}

impl<'a> PluginAudioProcessor<'a, PhyDulcimerShared, PhyDulcimerMainThread<'a>>
    for PhyDulcimerAudioProcessor<'a>
{
    fn activate(
        host: HostAudioProcessorHandle<'a>,
        _main_thread: &PhyDulcimerMainThread,
        shared: &'a PhyDulcimerShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        // 確保はここだけ。activate はメインスレッドで呼ばれるので許される。
        // Layout / Temperament は弦バンクの再構築を伴うので**ここで**読む
        // (Phase 7)。以後の変更は request_restart → 次の activate で反映。
        let max_block = (audio_config.max_frames_count as usize).max(1);
        let config = config_from_params(&shared.params);
        Ok(Self {
            engine: DulcimerEngine::with_config(audio_config.sample_rate, max_block, config),
            shared,
            host,
            left: vec![0.0; max_block],
            right: vec![0.0; max_block],
            silent_blocks: 0,
            restart_requested: false,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port found"))?;
        let mut output_channels = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let total_frames = output_channels.channel(0).map(|c| c.len()).unwrap_or(0);

        // 打弦点をブロックの先頭で楽器へ同期する。メインスレッドの flush 経由で
        // 変わった値を拾うため。同じブロック内のイベントで変わる場合は
        // handle_event 側でも同期する — ここだけだと、パラメータ変更と打鍵が
        // 同じブロックに来たとき、打鍵が古い打弦点を使ってしまう。
        self.engine
            .set_strike_ratio(self.shared.params.strike_position.load() as f64);

        let mut block_peak = 0.0f32;

        // サンプル精度でイベントを処理しつつ、区間ごとに音を作る。
        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                self.handle_event(event);
            }

            // ゲインと ROOM は**イベントを処理した後**に読む (D-015 の教訓 —
            // ブロック先頭で読むと、同じブロックのパラメータ変更に追い越される)。
            // set_room_params は値が変わったときだけ再計算するので毎バッチでも安い。
            let gain = self.shared.params.level.load();
            {
                let p = &self.shared.params;
                self.engine
                    .set_hammer_face(face_from_value(p.hammer_face.load()));
                // エンジン側で値が動いたときだけ全弦へ書き込むので毎バッチでも安い。
                self.engine.set_mute(p.mute.load() as f64);
                self.engine.set_room_enabled(p.room.load() >= 0.5);
                let size = match p.room_size.load().round() as i32 {
                    0 => RoomSize::Small,
                    2 => RoomSize::Large,
                    _ => RoomSize::Medium,
                };
                self.engine.set_room_params(RoomParams {
                    mic_distance_m: p.mic_distance.load() as f64,
                    xy_angle_deg: p.xy_angle.load() as f64,
                    size,
                    absorption: p.absorption.load() as f64,
                });
            }

            let Some((start, end)) = resolve_bounds(event_batch.sample_bounds(), total_frames)
            else {
                continue;
            };
            // activate で確保した長さを超えることは無いはずだが、超えたぶんは
            // 捨てる。オーディオスレッドで確保も panic もしないため。
            let len = (end - start).min(self.left.len());
            if len == 0 {
                continue;
            }

            let raw_peak = self
                .engine
                .process_stereo(&mut self.left[..len], &mut self.right[..len]);
            block_peak = block_peak.max(raw_peak * gain);

            for channel in 0..output_channels.channel_count() {
                let Some(buffer) = output_channels.channel_mut(channel) else {
                    continue;
                };
                let Some(dst) = buffer.get_mut(start..start + len) else {
                    continue;
                };
                let src = if channel == 0 {
                    &self.left
                } else {
                    &self.right
                };
                for (d, &s) in dst.iter_mut().zip(&src[..len]) {
                    *d = s * gain;
                }
            }
        }

        // **出力が本当に消えたときだけ眠る。** ダンパーが無く T60 が 10 秒を
        // 超える楽器なので、鳴っている最中に眠るとホストが process を止め、
        // 次の打鍵で凍結した響きが再開してポップノイズになる。
        if block_peak < SILENCE_THRESHOLD && !self.engine.any_hammer_active() {
            self.silent_blocks = self.silent_blocks.saturating_add(1);
        } else {
            self.silent_blocks = 0;
        }
        if self.silent_blocks >= SILENT_BLOCKS_TO_SLEEP {
            Ok(ProcessStatus::Sleep)
        } else {
            Ok(ProcessStatus::Continue)
        }
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.silent_blocks = 0;
    }

    fn stop_processing(&mut self) {
        // ホストの停止・ループ折り返し。鳴っている弦を持ち越すと、折り返しの
        // たびに響きが積み上がる。ここで全弦を止める。
        self.engine.reset();
        self.silent_blocks = 0;
    }
}

impl PhyDulcimerAudioProcessor<'_> {
    fn handle_event(&mut self, event: &UnknownEvent) {
        match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(event)) => {
                if let Match::Specific(key) = event.key() {
                    self.engine.note_on(key as u8, event.velocity());
                }
            }
            // **ノートオフは捨てる。** ダンパーが無い。`Instrument::note_off` を
            // 経由するのは「実装し忘れ」と区別するため。
            Some(CoreEventSpace::NoteOff(event)) => {
                if let Match::Specific(key) = event.key() {
                    self.engine.note_off(key as u8);
                }
            }
            // ホストが停止・シーク・シーケンス差し替えのときに送ってくる消音。
            // 無視すると再生を止めたときに音が残り続ける。
            Some(CoreEventSpace::NoteChoke(event)) => match event.key() {
                Match::Specific(key) => self.engine.choke(key as u8),
                Match::All => self.engine.reset(),
            },
            Some(CoreEventSpace::ParamValue(event)) => {
                self.shared.handle_param_event(event);
                // 打弦点と撥の面は即座に楽器へ同期する (どちらもただの store で、
                // 係数の再構築は次の打撃まで起きない)。同じブロックの後続の
                // 打鍵に効かせるため (D-015 の教訓)。
                self.engine
                    .set_strike_ratio(self.shared.params.strike_position.load() as f64);
                self.engine
                    .set_hammer_face(face_from_value(self.shared.params.hammer_face.load()));
                // Layout / Temperament は再構築が要るので、ここでは変更を検出して
                // ホストへ再 activate を頼むだけ (非対応ホストでは次の activate で
                // 反映される)。
                self.request_restart_if_config_changed();
            }
            Some(CoreEventSpace::Midi(event)) => {
                self.handle_midi(event.data());
            }
            _ => {}
        }
    }
}

impl PhyDulcimerAudioProcessor<'_> {
    /// Layout / Temperament が今のエンジンと食い違ったら、ホストへ一度だけ
    /// 再 activate を要求する (`clap_host.request_restart` — CLAP 仕様で
    /// どのスレッドからでも呼べる。確保なし)。
    fn request_restart_if_config_changed(&mut self) {
        if self.restart_requested {
            return;
        }
        if config_from_params(&self.shared.params) != self.engine.config() {
            self.host.shared().request_restart();
            self.restart_requested = true;
        }
    }

    /// 生の MIDI から必要な CC だけ拾う。
    ///
    /// パラメータと同じ `ParamValues` へ書くので、CC とホストのオートメーションは
    /// 同じ場所を動かす (後勝ち)。
    fn handle_midi(&mut self, data: [u8; 3]) {
        // Control Change 以外は使わない。
        if data[0] & 0xF0 != 0xB0 {
            return;
        }
        let amount = f64::from(data[2]) / 127.0;
        let params = &self.shared.params;

        match data[1] {
            CC_LEVEL => params.set(params::id::LEVEL, amount),
            CC_STRIKE => {
                let spec = params::spec(params::id::STRIKE_POSITION);
                if let Some(spec) = spec {
                    let value = spec.min + amount * (spec.max - spec.min);
                    params.set(params::id::STRIKE_POSITION, value);
                    self.engine.set_strike_ratio(value);
                }
            }
            CC_FACE => {
                // 0–42 / 43–84 / 85–127 の 3 分割 (境界はパラメータの丸めと同じ)。
                let value = (amount * 2.0).round();
                params.set(params::id::HAMMER_FACE, value);
                self.engine.set_hammer_face(face_from_value(value as f32));
            }
            CC_MUTE => {
                params.set(params::id::MUTE, amount);
                self.engine.set_mute(amount);
            }
            _ => {}
        }
    }
}

/// イベントバッチの範囲を `[start, end)` に解決する。
///
/// 空区間や範囲外は `None`。オーディオスレッドで添字 panic を起こさないため、
/// ここで一度きちんと閉じておく。
#[inline]
fn resolve_bounds(bounds: (Bound<usize>, Bound<usize>), total: usize) -> Option<(usize, usize)> {
    let start = match bounds.0 {
        Bound::Included(v) => v,
        Bound::Excluded(v) => v.checked_add(1)?,
        Bound::Unbounded => 0,
    };
    let end = match bounds.1 {
        Bound::Included(v) => v.checked_add(1)?,
        Bound::Excluded(v) => v,
        Bound::Unbounded => total,
    };
    let end = end.min(total);
    if start >= end {
        None
    } else {
        Some((start, end))
    }
}

impl PluginAudioPortsImpl for PhyDulcimerMainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input {
            0
        } else {
            1
        }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for PhyDulcimerMainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input {
            1
        } else {
            0
        }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if is_input && index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(1),
                name: b"main",
                // CLAP ダイアレクトを優先しつつ MIDI も受ける。Phase 7 の
                // ミュート CC は MIDI で来る。
                preferred_dialect: Some(NoteDialect::Clap),
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
            });
        }
    }
}

impl PluginMainThreadParams for PhyDulcimerMainThread<'_> {
    fn count(&self) -> u32 {
        params::PARAMS.len() as u32
    }

    fn get_info(&self, param_index: u32, info: &mut ParamInfoWriter) {
        let Some(spec) = params::PARAMS.get(param_index as usize) else {
            return;
        };
        let Some(id) = ClapId::from_raw(spec.id) else {
            return;
        };
        info.set(&ParamInfo {
            id,
            flags: ParamInfoFlags::IS_AUTOMATABLE,
            cookie: Default::default(),
            name: spec.name,
            module: b"",
            min_value: spec.min,
            max_value: spec.max,
            default_value: spec.default,
        });
    }

    fn get_value(&self, param_id: ClapId) -> Option<f64> {
        self.shared.params.get(u32::from(param_id))
    }

    fn value_to_text(
        &self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        let id = u32::from(param_id);
        let spec = params::spec(id).ok_or(std::fmt::Error)?;
        if id == params::id::ROOM {
            return write!(writer, "{}", if value >= 0.5 { "On" } else { "Off" });
        }
        if id == params::id::ROOM_SIZE {
            let name = match value.round() as i32 {
                0 => "Small",
                2 => "Large",
                _ => "Medium",
            };
            return write!(writer, "{name}");
        }
        if id == params::id::HAMMER_FACE {
            let name = match value.round() as i32 {
                1 => "Leather",
                2 => "Felt",
                _ => "Wood",
            };
            return write!(writer, "{name}");
        }
        if id == params::id::TEMPERAMENT {
            let name = if value >= 0.5 { "Equal" } else { "Pure Fifth" };
            return write!(writer, "{name}");
        }
        if id == params::id::LAYOUT {
            let name = if value >= 0.5 {
                "Chromatic E3-E6"
            } else {
                "Diatonic 15/14"
            };
            return write!(writer, "{name}");
        }
        write!(writer, "{:.*}{}", spec.decimals, value, spec.unit)
    }

    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        let id = u32::from(param_id);
        let spec = params::spec(id)?;
        let text = text.to_str().ok()?.trim();

        if id == params::id::ROOM {
            return match text.to_ascii_lowercase().as_str() {
                "on" | "1" => Some(1.0),
                "off" | "0" => Some(0.0),
                _ => None,
            };
        }
        if id == params::id::ROOM_SIZE {
            return match text.to_ascii_lowercase().as_str() {
                "small" | "s" | "0" => Some(0.0),
                "medium" | "m" | "1" => Some(1.0),
                "large" | "l" | "2" => Some(2.0),
                _ => None,
            };
        }
        if id == params::id::HAMMER_FACE {
            return match text.to_ascii_lowercase().as_str() {
                "wood" | "w" | "0" => Some(0.0),
                "leather" | "l" | "1" => Some(1.0),
                "felt" | "f" | "2" => Some(2.0),
                _ => None,
            };
        }
        if id == params::id::TEMPERAMENT {
            return match text.to_ascii_lowercase().as_str() {
                "pure fifth" | "pure" | "p" | "0" => Some(0.0),
                "equal" | "e" | "1" => Some(1.0),
                _ => None,
            };
        }
        if id == params::id::LAYOUT {
            return match text.to_ascii_lowercase().as_str() {
                "diatonic 15/14" | "diatonic" | "d" | "0" => Some(0.0),
                "chromatic e3-e6" | "chromatic" | "c" | "1" => Some(1.0),
                _ => None,
            };
        }

        let text = spec
            .unit
            .strip_prefix(' ')
            .and_then(|u| text.strip_suffix(u))
            .unwrap_or(text)
            .trim();
        Some(text.parse::<f64>().ok()?.clamp(spec.min, spec.max))
    }

    fn flush(
        &self,
        input_parameter_changes: &InputEvents,
        _output_parameter_changes: &mut OutputEvents,
    ) {
        for event in input_parameter_changes {
            if let Some(CoreEventSpace::ParamValue(event)) = event.as_core_event() {
                self.shared.handle_param_event(event);
            }
        }
    }
}

impl PluginAudioProcessorParams for PhyDulcimerAudioProcessor<'_> {
    fn flush(
        &mut self,
        input_parameter_changes: &InputEvents,
        _output_parameter_changes: &mut OutputEvents,
    ) {
        for event in input_parameter_changes {
            self.handle_event(event);
        }
    }
}

clack_export_entry!(SinglePluginEntry<PhyDulcimerPlugin>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_is_reverse_domain() {
        assert!(PLUGIN_ID.contains('.'), "CLAP の ID は逆ドメイン形式が慣例");
        assert!(PLUGIN_ID.starts_with("jp.ty17."));
    }

    #[test]
    fn every_param_id_fits_in_a_clap_id() {
        for spec in params::PARAMS {
            assert!(
                ClapId::from_raw(spec.id).is_some(),
                "param {} は ClapId にできない",
                spec.id
            );
        }
    }

    #[test]
    fn resolve_bounds_clamps_and_rejects_empty() {
        use std::ops::Bound::*;
        assert_eq!(resolve_bounds((Unbounded, Unbounded), 64), Some((0, 64)));
        assert_eq!(
            resolve_bounds((Included(8), Excluded(16)), 64),
            Some((8, 16))
        );
        assert_eq!(resolve_bounds((Included(0), Unbounded), 0), None);
        assert_eq!(resolve_bounds((Included(70), Unbounded), 64), None);
        assert_eq!(resolve_bounds((Included(10), Excluded(10)), 64), None);
    }
}
