//! CLAP `gui` 拡張 — エディタウィンドウの埋め込み (PhyPiano からの複製)。
//!
//! 描画そのものは [`phydulcimer_gui`] にあり、ここが受け持つのは
//!
//! - ホストから渡された親ウィンドウ (HWND / NSView / X11 Window) への埋め込み
//! - [`EditorHost`] の実装 (GUI ↔ プラグインの状態のやりとり)
//!
//! の 2 つだけ。UI のロジックを切り離してあるので、あちらはウィンドウを
//! 立てずにテストできる。
//!
//! # スレッド
//!
//! `egui-baseview` は**自前のスレッドでイベントループを回す**。したがって
//! [`EditorHost`] の実装はメインスレッド以外から呼ばれる。共有する状態
//! ([`SharedState`]) はアトミックとロックフリーのリングだけで構成してあり、
//! **DSP の状態には触らせない**。
//!
//! # DPI の既知の歪み (D-024)
//!
//! `get_size` は保持した数値をそのまま返すが、Win32 の CLAP はこれを
//! **物理 px** と解釈する。一方 baseview には `SystemScaleFactor` で
//! **論理 px** として渡している。100% スケーリングでは一致するが、
//! 125%/150% では親枠とずれる。PhyPiano から承知の上で継承した
//! (実害の報告があってから直す)。
//!
//! # 検証について
//!
//! **埋め込みの経路はホスト (DAW) が無いと確かめられない。** ここのコードは
//! コンパイルとロジックのテストまでしか自動検証できていない。実機確認は
//! `README.md` の手順で `.clap` を DAW に読み込ませて行うこと。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGuiImpl, Window};
use clack_plugin::plugin::PluginError;
use egui_baseview::baseview::dpi::LogicalSize;
use egui_baseview::baseview::{WindowHandle, WindowScalePolicy};
use egui_baseview::{EguiWindow, EguiWindowSettings};
use phydulcimer_gui::{Editor, EditorHost, ParamDescriptor, DEFAULT_EDITOR_SIZE};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle as RwhWindowHandle,
};

use crate::{params, PhyDulcimerMainThread, SharedState};

/// ホストから渡された親ウィンドウの薄い包み。
///
/// `baseview` は `raw-window-handle` の [`HasWindowHandle`] を要求するので、
/// CLAP の生ポインタをその形に持ち替えるためだけに置いてある。
struct ParentWindow(RawWindowHandle);

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<RwhWindowHandle<'_>, HandleError> {
        // SAFETY: ホストは `clap_plugin_gui::set_parent` で渡したウィンドウを、
        // `destroy` が呼ばれるまで有効に保つことを CLAP の仕様で約束している
        // (`clack_extensions::gui::Window` の説明も同じことを述べている)。
        // この handle はエディタウィンドウより長生きしない。
        #[allow(unsafe_code)]
        unsafe {
            Ok(RwhWindowHandle::borrow_raw(self.0))
        }
    }
}

/// CLAP の [`Window`] から `raw-window-handle` の生ハンドルを取り出す。
///
/// 対応していない API なら `None`。ホストは [`PluginGuiImpl::is_api_supported`]
/// で先に確かめてくるので、通常ここには来ない。
fn raw_handle(window: &Window) -> Option<RawWindowHandle> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::Win32WindowHandle;
        let hwnd = window.as_win32_hwnd()?;
        let handle = Win32WindowHandle::new(std::num::NonZeroIsize::new(hwnd as isize)?);
        return Some(RawWindowHandle::Win32(handle));
    }
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::AppKitWindowHandle;
        let view = window.as_cocoa_nsview()?;
        let ptr = std::ptr::NonNull::new(view)?;
        return Some(RawWindowHandle::AppKit(AppKitWindowHandle::new(ptr)));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use raw_window_handle::XcbWindowHandle;
        let id = window.as_x11_handle()?;
        let handle = XcbWindowHandle::new(std::num::NonZeroU32::new(id as u32)?);
        return Some(RawWindowHandle::Xcb(handle));
    }
    #[allow(unreachable_code)]
    {
        let _ = window;
        None
    }
}

/// このプラットフォームで CLAP が使うウィンドウ API。
fn native_api() -> GuiApiType<'static> {
    #[cfg(target_os = "windows")]
    {
        GuiApiType::WIN32
    }
    #[cfg(target_os = "macos")]
    {
        GuiApiType::COCOA
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        GuiApiType::X11
    }
}

/// GUI から見たプラグイン。
///
/// [`Arc`] で持つので、GUI スレッドへそのまま渡せる。
pub struct PluginEditorHost {
    shared: Arc<SharedState>,
}

impl PluginEditorHost {
    pub fn new(shared: Arc<SharedState>) -> Self {
        Self { shared }
    }
}

impl EditorHost for PluginEditorHost {
    fn params(&self) -> Vec<ParamDescriptor> {
        params::PARAMS
            .iter()
            .map(|p| ParamDescriptor {
                id: p.id,
                name: String::from_utf8_lossy(p.name).into_owned(),
                min: p.min,
                max: p.max,
                default: p.default,
                unit: p.unit.to_string(),
                decimals: p.decimals,
            })
            .collect()
    }

    fn param_value(&self, id: u32) -> f64 {
        self.shared.params.get(id).unwrap_or(0.0)
    }

    fn set_param(&self, id: u32, value: f64) {
        // 値はアトミックへ、変更の事実はビットマスクへ。process が
        // 出力イベントにしてホストへ通知する (emit_gui_edits)。
        self.shared.params.set(id, value);
        self.shared.mark_gui_edit(id);
    }

    fn note_on(&self, key: u8, velocity: f32) -> bool {
        self.shared.notes.push(key, velocity)
    }

    fn strike_serial(&self, key: u8) -> u32 {
        self.shared
            .strike_serials
            .get(key as usize)
            .map(|s| s.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    fn active_layout(&self) -> u32 {
        self.shared.active_layout.load(Ordering::Relaxed)
    }

    fn active_temperament(&self) -> u32 {
        self.shared.active_temperament.load(Ordering::Relaxed)
    }

    fn request_reload(&self) {
        // 前回の Reload で外れた旧エンジンをここ (GUI スレッド) で解放する。
        if let Ok(mut trash) = self.shared.engine_trash.lock() {
            trash.take();
        }
        let sample_rate = f64::from_bits(self.shared.sample_rate_bits.load(Ordering::Relaxed));
        let max_block = self.shared.max_block.load(Ordering::Relaxed) as usize;
        if sample_rate <= 0.0 || max_block == 0 {
            return; // まだ activate されていない (受け取る側がいない)
        }
        // 確保を伴う構築は GUI スレッドで行い、音声スレッドは交換だけ。
        let config = crate::config_from_params(&self.shared.params);
        let engine = Box::new(phydulcimer_core::engine::DulcimerEngine::with_config(
            sample_rate,
            max_block,
            config,
        ));
        if let Ok(mut slot) = self.shared.engine_swap.lock() {
            *slot = Some(engine);
        }
    }
}

/// エディタウィンドウの状態。
///
/// CLAP の `gui` 拡張はすべて `&self` で来るので、内側で可変にする必要がある。
/// GUI スレッドと同じ `Mutex` を共有するのはメインスレッドだけなので、
/// ここでロックを待っても問題ない (オーディオスレッドは一切触らない)。
#[derive(Default)]
pub struct EditorState {
    window: Mutex<Option<WindowHandle>>,
    /// ホストが要求した表示サイズ [px]
    size: Mutex<(u32, u32)>,
    /// `create` 済みか
    created: AtomicBool,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            window: Mutex::new(None),
            size: Mutex::new(DEFAULT_EDITOR_SIZE),
            created: AtomicBool::new(false),
        }
    }
}

impl PluginGuiImpl for PhyDulcimerMainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        // 埋め込みのみ。フローティングウィンドウは持たない。
        configuration.api_type == native_api() && !configuration.is_floating
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: native_api(),
            is_floating: false,
        })
    }

    fn create(&self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        if !self.is_api_supported(configuration) {
            return Err(PluginError::Message("対応していないウィンドウ API です"));
        }
        // 実際のウィンドウは `set_parent` で親が来てから開く。
        self.editor.created.store(true, Ordering::Release);
        // 開いている間はオーディオ側が Sleep しない (鍵盤クリックの排出のため)。
        self.shared.editor_open.store(true, Ordering::Release);
        Ok(())
    }

    fn destroy(&self) {
        self.editor.created.store(false, Ordering::Release);
        self.shared.editor_open.store(false, Ordering::Release);
        if let Ok(mut slot) = self.editor.window.lock() {
            if let Some(handle) = slot.take() {
                handle.close();
            }
        }
        // Reload で外れた旧エンジンが残っていればここ (メインスレッド) で捨てる。
        if let Ok(mut trash) = self.shared.engine_trash.lock() {
            trash.take();
        }
    }

    fn set_scale(&self, _scale: f64) -> Result<(), PluginError> {
        // baseview の `ScalePolicy::SystemScaleFactor` に任せる。
        Ok(())
    }

    fn get_size(&self) -> Option<GuiSize> {
        let (width, height) = *self.editor.size.lock().ok()?;
        Some(GuiSize { width, height })
    }

    fn can_resize(&self) -> bool {
        false
    }

    fn set_size(&self, size: GuiSize) -> Result<(), PluginError> {
        if let Ok(mut slot) = self.editor.size.lock() {
            *slot = (size.width, size.height);
        }
        Ok(())
    }

    fn set_parent(&self, window: Window) -> Result<(), PluginError> {
        let Some(raw) = raw_handle(&window) else {
            return Err(PluginError::Message(
                "ホストのウィンドウハンドルを解釈できません",
            ));
        };

        let (width, height) = self
            .editor
            .size
            .lock()
            .map(|s| *s)
            .unwrap_or(DEFAULT_EDITOR_SIZE);

        let settings = EguiWindowSettings::new()
            .with_tile(crate::PLUGIN_NAME)
            .with_size(LogicalSize::new(width as f64, height as f64))
            .with_scale_policy(WindowScalePolicy::SystemScaleFactor);

        let host = PluginEditorHost::new(Arc::clone(&self.shared.inner));
        let parent = ParentWindow(raw);

        let handle = EguiWindow::open_parented(
            &parent,
            settings,
            // `State` に相当するもの。エディタそのものを持たせる。
            Editor::new(host),
            // 構築時の 1 回だけ呼ばれる。
            |_ctx, _commands, _editor| {},
            // egui の出力を横取りするフック。使わない。
            |_output, _viewport, _editor| {},
            // 毎フレーム。
            |ui, _commands, editor| editor.ui(ui),
        );

        if let Ok(mut slot) = self.editor.window.lock() {
            *slot = Some(handle);
        }
        Ok(())
    }

    fn set_transient(&self, _window: Window) -> Result<(), PluginError> {
        // フローティングウィンドウを持たないので何もしない。
        Ok(())
    }

    fn suggest_title(&self, _title: &str) {}

    fn show(&self) -> Result<(), PluginError> {
        Ok(())
    }

    fn hide(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::id;

    fn host() -> PluginEditorHost {
        PluginEditorHost::new(Arc::new(SharedState::new()))
    }

    #[test]
    fn every_plugin_param_reaches_the_editor() {
        let descriptors = host().params();
        assert_eq!(descriptors.len(), params::PARAMS.len());
        for (d, p) in descriptors.iter().zip(params::PARAMS) {
            assert_eq!(d.id, p.id);
            assert_eq!(d.min, p.min);
            assert_eq!(d.max, p.max);
            assert_eq!(d.default, p.default);
        }
    }

    #[test]
    fn gui_param_ids_match_the_plugin() {
        // gui クレート側の複製 (param_id) がプラグインの id とずれていないこと。
        use phydulcimer_gui::param_id as g;
        for (gui, plugin) in [
            (g::LEVEL, id::LEVEL),
            (g::STRIKE_POSITION, id::STRIKE_POSITION),
            (g::ROOM, id::ROOM),
            (g::MIC_DISTANCE, id::MIC_DISTANCE),
            (g::XY_ANGLE, id::XY_ANGLE),
            (g::ROOM_SIZE, id::ROOM_SIZE),
            (g::ABSORPTION, id::ABSORPTION),
            (g::HAMMER_FACE, id::HAMMER_FACE),
            (g::MUTE, id::MUTE),
            (g::TEMPERAMENT, id::TEMPERAMENT),
            (g::LAYOUT, id::LAYOUT),
            (g::COMP, id::COMP),
        ] {
            assert_eq!(gui, plugin);
        }
    }

    #[test]
    fn set_param_marks_the_edit_for_the_host() {
        // GUI からの変更は値 + 通知ビットの両方に届く (通知が無いとホストの
        // オートメーションと Undo が壊れる)。
        let host = host();
        host.set_param(id::LEVEL, 0.42);
        assert!((host.param_value(id::LEVEL) - 0.42).abs() < 1e-6);
        let edits = host.shared.take_gui_edits();
        assert_ne!(edits, 0, "変更ビットが立っていない");
    }

    #[test]
    fn notes_flow_through_the_shared_ring() {
        let host = host();
        assert!(host.note_on(60, 0.8));
        assert_eq!(host.shared.notes.pop(), Some((60, 0.8)));
    }

    #[test]
    fn reload_builds_an_engine_with_the_current_params() {
        use phydulcimer_core::layout::LayoutKind;
        let host = host();
        // 未 activate (sample_rate = 0) では何も置かない。
        host.request_reload();
        assert!(host.shared.engine_swap.lock().unwrap().is_none());

        // activate 相当の実行条件を入れてから Reload。
        host.shared
            .sample_rate_bits
            .store(48_000.0f64.to_bits(), Ordering::Relaxed);
        host.shared.max_block.store(64, Ordering::Relaxed);
        host.set_param(id::LAYOUT, 1.0);
        host.request_reload();

        let engine = host.shared.engine_swap.lock().unwrap().take().unwrap();
        assert_eq!(engine.config().layout, LayoutKind::Chromatic);
    }

    #[test]
    fn out_of_range_strike_serials_read_as_zero() {
        // key は u16 由来のキャストで 128 以上になり得る。読みは 0 で防御。
        assert_eq!(host().strike_serial(200), 0);
    }
}
