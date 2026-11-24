mod capture;
mod clipboard_watch;
mod config;
mod hotkey;
mod llm;
mod mouse_hook;
mod search;
mod selection;
mod tray;
mod win_native;
mod window_mgr;

use config::{load_config, save_config, AppConfig};
use hotkey::HotkeyBindResult;
use llm::{ChatStreamRequest, FetchModelsResponse};
use mouse_hook::MouseHookEvent;
use tauri::{AppHandle, Emitter, Manager, State};
use window_mgr::{AppState, PopupPrefillPayload};

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().clone()
}

#[tauri::command]
fn save_app_config(
    app: AppHandle,
    state: State<'_, AppState>,
    new_config: AppConfig,
) -> Result<Vec<HotkeyBindResult>, String> {
    save_config(&new_config)?;
    win_native::apply_autostart(new_config.ui.autostart);
    let show_ball = new_config.ui.show_ball;
    {
        let mut guard = state.config.lock();
        *guard = new_config.clone();
    }

    if let Some(ball) = app.get_webview_window("ball") {
        if show_ball && !window_mgr::is_suppressed_by_app(&app) {
            let _ = ball.show();
        } else {
            let _ = ball.hide();
        }
    }
    window_mgr::apply_ball_edge_setting(&app);
    window_mgr::refresh_app_suppression(&app);

    let bind_results = hotkey::apply_all_hotkeys(&app, &new_config);
    let _ = app.emit("config:updated", &new_config);
    Ok(bind_results)
}

#[tauri::command]
fn set_active_model(
    app: AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
    model_id: String,
) -> Result<(), String> {
    let updated = {
        let mut guard = state.config.lock();
        guard.active_provider_id = provider_id;
        guard.active_model_id = model_id;
        save_config(&guard)?;
        guard.clone()
    };
    let _ = app.emit("config:updated", &updated);
    Ok(())
}

#[tauri::command]
fn set_default_web_search(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let updated = {
        let mut guard = state.config.lock();
        guard.ui.default_web_search = enabled;
        save_config(&guard)?;
        guard.clone()
    };
    let _ = app.emit("config:updated", &updated);
    Ok(())
}

#[tauri::command]
fn toggle_popup_from_ball(app: AppHandle) {
    window_mgr::toggle_popup(&app, true);
}

#[tauri::command]
fn hide_popup(app: AppHandle) {
    window_mgr::hide_popup_and_clear(&app);
}

#[tauri::command]
fn resize_popup(app: AppHandle, height: f64) {
    window_mgr::resize_popup(&app, height);
}

#[tauri::command]
fn set_popup_pin(app: AppHandle, pinned: bool) {
    window_mgr::set_popup_pin(&app, pinned);
}

#[tauri::command]
fn ball_drag_finished(app: AppHandle) {
    window_mgr::sync_ball_after_drag(&app);
}

#[tauri::command]
fn get_ball_edge_state(app: AppHandle) -> window_mgr::BallEdgePayload {
    window_mgr::get_ball_edge_state(&app)
}

#[tauri::command]
fn show_ball_menu(app: AppHandle) -> Result<(), String> {
    tray::show_ball_context_menu(&app)
}

#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    window_mgr::open_settings_window_async(&app);
    Ok(())
}

#[tauri::command]
fn set_mouse_recording(recording: bool) {
    mouse_hook::set_recording_mode(recording);
}

#[tauri::command]
fn list_running_processes() -> Vec<String> {
    win_native::list_running_process_names()
}

#[tauri::command]
fn take_pending_prefill(state: State<'_, AppState>) -> Option<PopupPrefillPayload> {
    state.pending_prefill.lock().take()
}

#[tauri::command]
fn start_region_capture_cmd(app: AppHandle) {
    window_mgr::start_region_capture(&app);
}

#[tauri::command]
fn capture_active_window_cmd(app: AppHandle) {
    window_mgr::capture_active_window_and_ask(&app);
}

#[tauri::command]
fn get_frozen_capture_preview() -> Option<String> {
    capture::get_frozen_preview()
}

#[tauri::command]
fn finish_region_capture(
    app: AppHandle,
    x_ratio: f64,
    y_ratio: f64,
    w_ratio: f64,
    h_ratio: f64,
) {
    window_mgr::finish_region_capture(&app, x_ratio, y_ratio, w_ratio, h_ratio);
}

#[tauri::command]
fn cancel_region_capture(app: AppHandle) {
    window_mgr::cancel_region_capture(&app);
}

#[tauri::command]
async fn fetch_provider_models(
    base_url: String,
    api_key: String,
) -> Result<FetchModelsResponse, String> {
    llm::fetch_provider_models(&base_url, &api_key).await
}

#[tauri::command]
async fn start_chat_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ChatStreamRequest,
) -> Result<(), String> {
    let cfg = state.config.lock().clone();
    llm::spawn_chat_stream(app, cfg, request, state.streams.clone())
}

#[tauri::command]
fn abort_chat_stream(state: State<'_, AppState>, request_id: String) {
    state.streams.abort(&request_id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial_cfg = load_config();
    let app_state = AppState::new(initial_cfg.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(app_state)
        .on_menu_event(|app, event| {
            tray::handle_menu_event(app, event.id().as_ref());
        })
        .setup(move |app| {
            let handle = app.handle().clone();

            tray::init_tray(&handle)?;
            window_mgr::init_windows(&handle);
            window_mgr::start_foreground_watcher(&handle);

            // Start 60s fresh clipboard image watcher
            clipboard_watch::start_clipboard_watcher();

            // Start Win32 low-level mouse side button hook
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MouseHookEvent>();
            mouse_hook::start_mouse_hook(tx);

            let hook_app = handle.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    match ev {
                        MouseHookEvent::Action(action) => {
                            hotkey::dispatch_action(&hook_app, action);
                        }
                        MouseHookEvent::Captured(binding) => {
                            mouse_hook::set_recording_mode(false);
                            let _ = hook_app.emit("hotkey:mouse-captured", binding);
                        }
                    }
                }
            });

            // Apply initial keyboard + mouse hotkeys
            let _ = hotkey::apply_all_hotkeys(&handle, &initial_cfg);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_app_config,
            set_active_model,
            set_default_web_search,
            toggle_popup_from_ball,
            hide_popup,
            resize_popup,
            set_popup_pin,
            ball_drag_finished,
            get_ball_edge_state,
            show_ball_menu,
            open_settings,
            set_mouse_recording,
            list_running_processes,
            take_pending_prefill,
            start_region_capture_cmd,
            capture_active_window_cmd,
            get_frozen_capture_preview,
            finish_region_capture,
            cancel_region_capture,
            fetch_provider_models,
            start_chat_stream,
            abort_chat_stream,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BTW-Float");
}
