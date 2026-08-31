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

/// CLAP プラグイン ID (逆ドメイン形式)。公開後は変更しないこと。
pub const PLUGIN_ID: &str = "jp.ty17.phydulcimer";

/// ホストの UI に出る名前。
pub const PLUGIN_NAME: &str = "PhyDulcimer";

/// プラグインのバージョン (Cargo のパッケージバージョンに追従)。
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 「鳴っていない」と判定する出力ピークの閾値。
///
/// **ボイス数では判定できない** — この楽器にはそもそもボイスが無く、全弦が
/// 常時走っている。PhyPiano は「ボイス 0 で眠る」判定で残響を凍結させて
/// ポップノイズを出した (P-035)。ここでは実際の出力レベルだけを見る。
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
    /// エンジンのステレオ出力を受けてからポートへ配る (事前確保)
    left: Vec<f32>,
    right: Vec<f32>,
    /// 連続で無音だったブロック数。[`SILENT_BLOCKS_TO_SLEEP`] に達したら眠る
    silent_blocks: u32,
}

impl<'a> PluginAudioProcessor<'a, PhyDulcimerShared, PhyDulcimerMainThread<'a>>
    for PhyDulcimerAudioProcessor<'a>
{
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &PhyDulcimerMainThread,
        shared: &'a PhyDulcimerShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        // 確保はここだけ。activate はメインスレッドで呼ばれるので許される。
        let max_block = (audio_config.max_frames_count as usize).max(1);
        Ok(Self {
            engine: DulcimerEngine::new(audio_config.sample_rate, max_block),
            shared,
            left: vec![0.0; max_block],
            right: vec![0.0; max_block],
            silent_blocks: 0,
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

            // ゲインはイベントを処理した後に読む。同じブロックに Level の変更が
            // 来ていたら、この区間から効かせる。校正はエンジンの中にあるので、
            // ここは音量つまみ (クリップ後) だけ。
            let gain = self.shared.params.level.load();

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
        // 次の打鍵で凍結した響きが再開してポップノイズになる (PhyPiano P-035)。
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
                // 打弦点は即座に楽器へ同期する (ただの store で、係数の再構築は
                // 次の打撃まで起きない)。同じブロックの後続の打鍵に効かせるため。
                self.engine
                    .set_strike_ratio(self.shared.params.strike_position.load() as f64);
            }
            // ミュート CC (手のひら) は Phase 7 でここに入る。
            Some(CoreEventSpace::Midi(event)) => {
                self.handle_midi(event.data());
            }
            _ => {}
        }
    }
}

impl PhyDulcimerAudioProcessor<'_> {
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
        let spec = params::spec(u32::from(param_id)).ok_or(std::fmt::Error)?;
        write!(writer, "{:.*}{}", spec.decimals, value, spec.unit)
    }

    fn text_to_value(&self, param_id: ClapId, text: &CStr) -> Option<f64> {
        let spec = params::spec(u32::from(param_id))?;
        let text = text.to_str().ok()?.trim();
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
