use crate::config::AppConfig;
use crate::mouse_hook::{self, HotkeyAction};
use crate::window_mgr;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindResult {
    pub action: String,
    pub binding: String,
    pub ok: bool,
    pub error: Option<String>,
}

pub fn apply_all_hotkeys(app: &AppHandle, cfg: &AppConfig) -> Vec<HotkeyBindResult> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut mouse_bindings = Vec::new();
    let mut results = Vec::new();

    let items = [
        ("togglePopup", HotkeyAction::TogglePopup, cfg.hotkeys.toggle_popup.as_str()),
        ("mouseToggle", HotkeyAction::TogglePopup, cfg.hotkeys.mouse_toggle.as_str()),
        ("captureRegion", HotkeyAction::CaptureRegion, cfg.hotkeys.capture_region.as_str()),
        ("captureWindow", HotkeyAction::CaptureWindow, cfg.hotkeys.capture_window.as_str()),
    ];

    for (name, action, raw) in items {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            results.push(HotkeyBindResult {
                action: name.to_string(),
                binding: String::new(),
                ok: true,
                error: None,
            });
            continue;
        }

        if let Some(mb) = mouse_hook::parse_mouse_binding(trimmed) {
            mouse_bindings.push((mb, action));
            results.push(HotkeyBindResult {
                action: name.to_string(),
                binding: trimmed.to_string(),
                ok: true,
                error: None,
            });
            continue;
        }

        let app_clone = app.clone();
        match gs.on_shortcut(trimmed, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                dispatch_action(&app_clone, action);
            }
        }) {
            Ok(_) => results.push(HotkeyBindResult {
                action: name.to_string(),
                binding: trimmed.to_string(),
                ok: true,
                error: None,
            }),
            Err(e) => results.push(HotkeyBindResult {
                action: name.to_string(),
                binding: trimmed.to_string(),
                ok: false,
                error: Some(format!("快捷键冲突或无效: {e}")),
            }),
        }
    }

    mouse_hook::update_hook_config(
        mouse_bindings,
        cfg.mouse_priority.passthrough_fullscreen,
        &cfg.mouse_priority.blacklist_processes,
        cfg.ui.clipboard_image_freshness_secs,
    );

    results
}

pub fn dispatch_action(app: &AppHandle, action: HotkeyAction) {
    match action {
        HotkeyAction::TogglePopup => {
            window_mgr::toggle_popup(app, false);
        }
        HotkeyAction::CaptureRegion => {
            window_mgr::start_region_capture(app);
        }
        HotkeyAction::CaptureWindow => {
            window_mgr::capture_active_window_and_ask(app);
        }
    }
}
