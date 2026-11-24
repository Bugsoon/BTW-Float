use crate::capture;
use crate::clipboard_watch;
use crate::config::{save_config, AppConfig};
use crate::llm::StreamRegistry;
use crate::selection;
use crate::tray;
use crate::win_native::{self, BallInputEvent};
use parking_lot::Mutex;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

// Shadow gutter sized to exceed Chromium/Skia's exact 3*sigma Gaussian blur cutoff (zero box clipping)
pub const BALL_WIN_LOGICAL_SIZE: f64 = 86.0;
pub const POPUP_WIN_LOGICAL_WIDTH: f64 = 584.0;
pub const POPUP_PAD_TOP_LOGICAL: f64 = 15.0;
pub const POPUP_PAD_BOTTOM_LOGICAL: f64 = 27.0;
pub const POPUP_PAD_TOTAL_Y: f64 = POPUP_PAD_TOP_LOGICAL + POPUP_PAD_BOTTOM_LOGICAL; // 42
pub const POPUP_CAPSULE_HEIGHT: f64 = 58.0;
pub const POPUP_WIN_INITIAL_HEIGHT: f64 = POPUP_CAPSULE_HEIGHT + POPUP_PAD_TOTAL_Y; // 100
pub const POPUP_WIN_MIN_HEIGHT: f64 = 98.0;
pub const POPUP_WIN_MAX_HEIGHT: f64 = 610.0;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupPrefillPayload {
    pub text: Option<String>,
    pub image: Option<String>,
    pub source: Option<String>, // "selection" | "clipboard_image" | "region_capture" | "window_capture" | "drop"
    pub auto_submit: bool,
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub streams: Arc<StreamRegistry>,
    pub popup_open: AtomicBool,
    pub popup_pinned: AtomicBool,
    pub last_popup_shown: Mutex<Instant>,
    pub last_popup_blur_hidden: Mutex<Option<Instant>>,
    pub pending_prefill: Mutex<Option<PopupPrefillPayload>>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Mutex::new(config),
            streams: Arc::new(StreamRegistry::default()),
            popup_open: AtomicBool::new(false),
            popup_pinned: AtomicBool::new(false),
            last_popup_shown: Mutex::new(Instant::now() - Duration::from_secs(10)),
            last_popup_blur_hidden: Mutex::new(None),
            pending_prefill: Mutex::new(None),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BallEdgePayload {
    edge: String, // "none" | "left" | "right"
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BallUiStatePayload {
    hovered: Option<bool>,
    pressed: Option<bool>,
}

pub fn init_windows(app: &AppHandle) {
    let cfg = app.state::<AppState>().config.lock().clone();

    if let Some(ball) = app.get_webview_window("ball") {
        let scale = ball.scale_factor().unwrap_or(1.0);
        let phys_ball = (BALL_WIN_LOGICAL_SIZE * scale).round() as i32;
        let _ = ball.set_size(Size::Physical(PhysicalSize::new(
            phys_ball as u32,
            phys_ball as u32,
        )));

        let (bx, by) = match (cfg.ui.ball_x, cfg.ui.ball_y) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                let area = win_native::get_monitor_work_area_for_point(100, 100);
                (
                    area.right - phys_ball - 14,
                    area.top + (area.bottom - area.top) / 2,
                )
            }
        };
        let _ = ball.set_position(Position::Physical(PhysicalPosition::new(bx, by)));

        if let Ok(hwnd) = ball.hwnd() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BallInputEvent>();
            win_native::install_ball_input_handler(hwnd.0 as isize, tx);

            let ball_app = app.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    match ev {
                        BallInputEvent::Hover(h) => {
                            if let Some(b) = ball_app.get_webview_window("ball") {
                                let _ = b.emit(
                                    "ball:ui-state",
                                    BallUiStatePayload {
                                        hovered: Some(h),
                                        pressed: None,
                                    },
                                );
                            }
                        }
                        BallInputEvent::Press(p) => {
                            if let Some(b) = ball_app.get_webview_window("ball") {
                                let _ = b.emit(
                                    "ball:ui-state",
                                    BallUiStatePayload {
                                        hovered: None,
                                        pressed: Some(p),
                                    },
                                );
                            }
                        }
                        BallInputEvent::LeftClick => {
                            toggle_popup(&ball_app, true);
                        }
                        BallInputEvent::RightClick => {
                            let app_for_menu = ball_app.clone();
                            let _ = ball_app.run_on_main_thread(move || {
                                let _ = tray::show_ball_context_menu(&app_for_menu);
                            });
                        }
                        BallInputEvent::DragEnded => {
                            sync_ball_after_drag(&ball_app);
                        }
                        BallInputEvent::DroppedFiles(files) => {
                            handle_dropped_files(&ball_app, files);
                        }
                    }
                }
            });
        }

        if cfg.ui.show_ball {
            let _ = ball.show();
        } else {
            let _ = ball.hide();
        }
    }

    if let Some(popup) = app.get_webview_window("popup") {
        let scale = popup.scale_factor().unwrap_or(1.0);
        let phys_w = (POPUP_WIN_LOGICAL_WIDTH * scale).round() as u32;
        let phys_h = (POPUP_WIN_INITIAL_HEIGHT * scale).round() as u32;
        let _ = popup.set_size(Size::Physical(PhysicalSize::new(phys_w, phys_h)));
        let _ = popup.hide();

        let app_handle = app.clone();
        popup.on_window_event(move |event| {
            if let WindowEvent::Focused(false) = event {
                let state = app_handle.state::<AppState>();
                if !state.popup_open.load(Ordering::SeqCst) {
                    return;
                }
                let pinned = state.popup_pinned.load(Ordering::SeqCst);
                let auto_hide = state.config.lock().ui.popup_auto_hide_on_blur;
                let elapsed = state.last_popup_shown.lock().elapsed();
                if !pinned && auto_hide && elapsed > Duration::from_millis(260) {
                    *state.last_popup_blur_hidden.lock() = Some(Instant::now());
                    hide_popup_and_clear(&app_handle);
                }
            }
        });
    }
}

pub fn toggle_popup(app: &AppHandle, from_ball: bool) {
    let state = app.state::<AppState>();

    if from_ball {
        let last_blur = *state.last_popup_blur_hidden.lock();
        if let Some(t) = last_blur {
            if t.elapsed() < Duration::from_millis(280) {
                return;
            }
        }
    }

    if state.popup_open.load(Ordering::SeqCst) {
        hide_popup_and_clear(app);
    } else {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            let (freshness_secs, auto_submit_sel) = {
                let state = app_clone.state::<AppState>();
                let cfg = state.config.lock();
                (
                    cfg.ui.clipboard_image_freshness_secs,
                    cfg.ui.auto_submit_selection,
                )
            };

            // Priority 1: Active highlighted selection in foreground app -> auto-ask AI immediately!
            // Priority 2: Fresh clipboard screenshot within freshness window -> open with image ready for user's question!
            let prefill = if let Some(sel) = selection::capture_foreground_selection() {
                Some(PopupPrefillPayload {
                    text: Some(sel),
                    image: None,
                    source: Some("selection".to_string()),
                    auto_submit: auto_submit_sel,
                })
            } else {
                clipboard_watch::take_fresh_clipboard_image(freshness_secs).map(|img| {
                    PopupPrefillPayload {
                        text: None,
                        image: Some(img),
                        source: Some("clipboard_image".to_string()),
                        auto_submit: false,
                    }
                })
            };

            open_popup_with_prefill(&app_clone, from_ball, prefill);
        });
    }
}

pub fn open_popup_with_prefill(
    app: &AppHandle,
    near_ball: bool,
    prefill: Option<PopupPrefillPayload>,
) {
    let Some(popup) = app.get_webview_window("popup") else {
        return;
    };
    let scale = popup.scale_factor().unwrap_or(1.0);
    let phys_w = (POPUP_WIN_LOGICAL_WIDTH * scale).round() as i32;
    let phys_h = (POPUP_WIN_INITIAL_HEIGHT * scale).round() as i32;

    // Always anchor near the floating ball when it is visible (ball click, side button, hotkey alike),
    // so the capsule appears in one predictable place. Fall back to cursor-based placement otherwise.
    let ball_visible = app.state::<AppState>().config.lock().ui.show_ball
        && app
            .get_webview_window("ball")
            .and_then(|b| b.is_visible().ok())
            .unwrap_or(false);

    let (target_x, target_y) = if near_ball || ball_visible {
        compute_near_ball_pos(app, phys_w, phys_h, scale)
    } else if prefill.as_ref().and_then(|p| p.text.as_ref()).is_some() {
        // When triggered on highlighted text (e.g. reading a paper), pop up right near the cursor!
        near_cursor_for_reading(phys_w, phys_h, scale)
    } else {
        center_upper_for_cursor(phys_w, phys_h)
    };

    {
        let state = app.state::<AppState>();
        *state.pending_prefill.lock() = prefill.clone();
    }

    present_popup(app, &popup, target_x, target_y, phys_w, phys_h, prefill);
}

fn compute_near_ball_pos(app: &AppHandle, phys_w: i32, phys_h: i32, scale: f64) -> (i32, i32) {
    if let Some(ball) = app.get_webview_window("ball") {
        if let (Ok(pos), Ok(size)) = (ball.outer_position(), ball.outer_size()) {
            let cx = pos.x + (size.width as i32) / 2;
            let cy = pos.y + (size.height as i32) / 2;
            let area = win_native::get_monitor_work_area_for_point(cx, cy);
            let overlap = (6.0 * scale).round() as i32;

            let x = if cx > (area.left + area.right) / 2 {
                pos.x - phys_w + overlap
            } else {
                pos.x + size.width as i32 - overlap
            };
            let clamped_x = x.clamp(area.left + 4, (area.right - phys_w - 4).max(area.left + 4));
            let clamped_y = (cy - phys_h / 2)
                .clamp(area.top + 8, (area.bottom - phys_h - 8).max(area.top + 8));
            return (clamped_x, clamped_y);
        }
    }
    center_upper_for_cursor(phys_w, phys_h)
}

fn near_cursor_for_reading(phys_w: i32, phys_h: i32, scale: f64) -> (i32, i32) {
    let (mx, my) = win_native::get_cursor_pos();
    let area = win_native::get_monitor_work_area_for_point(mx, my);
    let offset_y = (20.0 * scale).round() as i32;
    let estimated_expanded_h = (320.0 * scale).round() as i32;

    let x = (mx - phys_w / 2).clamp(area.left + 8, (area.right - phys_w - 8).max(area.left + 8));
    let y = if my + offset_y + estimated_expanded_h <= area.bottom - 12 {
        my + offset_y
    } else {
        (my - estimated_expanded_h).max(area.top + 12)
    };
    let clamped_y = y.clamp(area.top + 8, (area.bottom - phys_h - 8).max(area.top + 8));
    (x, clamped_y)
}

fn center_upper_for_cursor(phys_w: i32, phys_h: i32) -> (i32, i32) {
    let (mx, my) = win_native::get_cursor_pos();
    let area = win_native::get_monitor_work_area_for_point(mx, my);
    let x = area.left + ((area.right - area.left) - phys_w) / 2;
    let y = area.top + ((area.bottom - area.top) as f64 * 0.22).round() as i32;
    let clamped_x = x.clamp(area.left + 4, (area.right - phys_w - 4).max(area.left + 4));
    let clamped_y = y.clamp(area.top + 12, (area.bottom - phys_h - 12).max(area.top + 12));
    (clamped_x, clamped_y)
}

fn present_popup(
    app: &AppHandle,
    popup: &WebviewWindow,
    x: i32,
    y: i32,
    phys_w: i32,
    phys_h: i32,
    prefill: Option<PopupPrefillPayload>,
) {
    let state = app.state::<AppState>();
    state.popup_open.store(true, Ordering::SeqCst);
    *state.last_popup_shown.lock() = Instant::now();
    *state.last_popup_blur_hidden.lock() = None;

    let _ = popup.emit("popup:opened", ());
    if let Some(pf) = prefill {
        let _ = popup.emit("popup:prefill", pf);
    }

    let app_for_main = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(p) = app_for_main.get_webview_window("popup") {
            let _ = p.set_size(Size::Physical(PhysicalSize::new(phys_w as u32, phys_h as u32)));
            let _ = p.set_position(Position::Physical(PhysicalPosition::new(x, y)));
            let _ = p.show();
            if let Ok(hwnd) = p.hwnd() {
                win_native::show_and_focus_popup(hwnd.0 as isize);
            }
            let _ = p.set_focus();
        }
    });
}

pub fn hide_popup_and_clear(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.popup_open.store(false, Ordering::SeqCst);
    state.streams.abort_all();
    state.popup_pinned.store(false, Ordering::SeqCst);
    *state.pending_prefill.lock() = None;

    if let Some(popup) = app.get_webview_window("popup") {
        let _ = popup.hide();
        let scale = popup.scale_factor().unwrap_or(1.0);
        let phys_w = (POPUP_WIN_LOGICAL_WIDTH * scale).round() as u32;
        let phys_h = (POPUP_WIN_INITIAL_HEIGHT * scale).round() as u32;
        let _ = popup.set_size(Size::Physical(PhysicalSize::new(phys_w, phys_h)));
        let _ = popup.emit("popup:reset", ());
    }
}

pub fn resize_popup(app: &AppHandle, logical_content_height: f64) {
    let Some(popup) = app.get_webview_window("popup") else {
        return;
    };
    let clamped_h = (logical_content_height + POPUP_PAD_TOTAL_Y)
        .clamp(POPUP_WIN_MIN_HEIGHT, POPUP_WIN_MAX_HEIGHT);
    let scale = popup.scale_factor().unwrap_or(1.0);
    let phys_w = (POPUP_WIN_LOGICAL_WIDTH * scale).round() as i32;
    let phys_h = (clamped_h * scale).round() as i32;

    if let Ok(cur_size) = popup.outer_size() {
        if (cur_size.height as i32 - phys_h).abs() < 2 {
            return;
        }
    }

    if let Ok(pos) = popup.outer_position() {
        let area = win_native::get_monitor_work_area_for_point(pos.x + phys_w / 2, pos.y + 20);
        if pos.y + phys_h > area.bottom - 4 {
            let new_y = (area.bottom - phys_h - 4).max(area.top + 4);
            let _ = popup.set_position(Position::Physical(PhysicalPosition::new(pos.x, new_y)));
        }
    }

    let _ = popup.set_size(Size::Physical(PhysicalSize::new(phys_w as u32, phys_h as u32)));
}

pub fn set_popup_pin(app: &AppHandle, pinned: bool) {
    let state = app.state::<AppState>();
    state.popup_pinned.store(pinned, Ordering::SeqCst);
}

///Temporarily hides BTW's own windows synchronously via Win32 + DwmFlush so screenshots never include BTW.
fn hide_own_windows_for_capture(app: &AppHandle) {
    if app.state::<AppState>().popup_open.load(Ordering::SeqCst) {
        hide_popup_and_clear(app);
    }
    if let Some(ball) = app.get_webview_window("ball") {
        if let Ok(hwnd) = ball.hwnd() {
            win_native::hide_hwnd_now(hwnd.0 as isize);
        }
    }
    capture::wait_dwm_flush();
}

fn restore_ball_after_capture(app: &AppHandle) {
    let show_ball = app.state::<AppState>().config.lock().ui.show_ball;
    if show_ball {
        if let Some(ball) = app.get_webview_window("ball") {
            if let Ok(hwnd) = ball.hwnd() {
                win_native::show_hwnd_noactivate(hwnd.0 as isize);
            }
        }
    }
}

/// Launches the full-monitor frozen region selection overlay (`capture` window, created on-demand & destroyed after crop).
pub fn start_region_capture(app: &AppHandle) {
    if let Some(existing) = app.get_webview_window("capture") {
        let _ = existing.set_focus();
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        let (cx, cy) = win_native::get_cursor_pos();
        hide_own_windows_for_capture(&handle);

        let Some((mx, my, mw, mh)) = capture::freeze_monitor_at_point(cx, cy) else {
            restore_ball_after_capture(&handle);
            return;
        };

        if WebviewWindowBuilder::new(
            &handle,
            "capture",
            WebviewUrl::App("index.html".into()),
        )
        .title("BTW-Float Region Capture")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()
        .is_ok()
        {
            let h_main = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                if let Some(cwin) = h_main.get_webview_window("capture") {
                    let _ = cwin.set_position(Position::Physical(PhysicalPosition::new(mx, my)));
                    let _ = cwin.set_size(Size::Physical(PhysicalSize::new(mw as u32, mh as u32)));
                    let _ = cwin.show();
                    if let Ok(hwnd) = cwin.hwnd() {
                        win_native::show_and_focus_popup(hwnd.0 as isize);
                    }
                    let _ = cwin.set_focus();
                }
            });
        } else {
            capture::clear_frozen_capture();
            restore_ball_after_capture(&handle);
        }
    });
}

pub fn finish_region_capture(
    app: &AppHandle,
    x_ratio: f64,
    y_ratio: f64,
    w_ratio: f64,
    h_ratio: f64,
) {
    let cropped = capture::crop_frozen_by_ratio(x_ratio, y_ratio, w_ratio, h_ratio);
    if let Some(cwin) = app.get_webview_window("capture") {
        let _ = cwin.close();
    }
    restore_ball_after_capture(app);

    if let Some(data_url) = cropped {
        open_popup_with_prefill(
            app,
            false,
            Some(PopupPrefillPayload {
                text: None,
                image: Some(data_url),
                source: Some("region_capture".to_string()),
                auto_submit: false,
            }),
        );
    }
}

pub fn cancel_region_capture(app: &AppHandle) {
    capture::clear_frozen_capture();
    if let Some(cwin) = app.get_webview_window("capture") {
        let _ = cwin.close();
    }
    restore_ball_after_capture(app);
}

/// Captures the currently active foreground window and immediately opens the capsule with the image attached.
pub fn capture_active_window_and_ask(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        hide_own_windows_for_capture(&handle);
        let data_url = capture::capture_foreground_window_jpeg();
        restore_ball_after_capture(&handle);

        open_popup_with_prefill(
            &handle,
            false,
            data_url.map(|img| PopupPrefillPayload {
                text: None,
                image: Some(img),
                source: Some("window_capture".to_string()),
                auto_submit: false,
            }),
        );
    });
}

fn handle_dropped_files(app: &AppHandle, files: Vec<String>) {
    let Some(first) = files.first().cloned() else {
        return;
    };
    let handle = app.clone();
    std::thread::spawn(move || {
        let path = Path::new(&first);
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let is_img = matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif"
        );
        if is_img {
            if let Some(data_url) = capture::load_dropped_image_file(path) {
                open_popup_with_prefill(
                    &handle,
                    true,
                    Some(PopupPrefillPayload {
                        text: None,
                        image: Some(data_url),
                        source: Some("drop".to_string()),
                        auto_submit: false,
                    }),
                );
            }
            return;
        }

        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() <= 128 * 1024 {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        let snippet: String = trimmed.chars().take(12_000).collect();
                        open_popup_with_prefill(
                            &handle,
                            true,
                            Some(PopupPrefillPayload {
                                text: Some(snippet),
                                image: None,
                                source: Some("drop".to_string()),
                                auto_submit: false,
                            }),
                        );
                    }
                }
            }
        }
    });
}

pub fn toggle_ball_visibility(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut cfg = state.config.lock();
    cfg.ui.show_ball = !cfg.ui.show_ball;
    let visible = cfg.ui.show_ball;
    let _ = save_config(&cfg);
    drop(cfg);

    if let Some(ball) = app.get_webview_window("ball") {
        if visible {
            let _ = ball.show();
        } else {
            let _ = ball.hide();
        }
    }
    visible
}

pub fn sync_ball_after_drag(app: &AppHandle) {
    let Some(ball) = app.get_webview_window("ball") else {
        return;
    };
    let Ok(pos) = ball.outer_position() else {
        return;
    };
    let Ok(size) = ball.outer_size() else {
        return;
    };

    let cx = pos.x + size.width as i32 / 2;
    let cy = pos.y + size.height as i32 / 2;
    let area = win_native::get_monitor_work_area_for_point(cx, cy);

    let state = app.state::<AppState>();
    let auto_hide_edge = state.config.lock().ui.ball_auto_hide_edge;

    let mut final_x = pos.x.clamp(area.left, area.right - size.width as i32);
    let final_y = pos.y.clamp(area.top + 4, area.bottom - size.height as i32 - 4);
    let mut edge = "none";

    if auto_hide_edge {
        let threshold = 22;
        if final_x - area.left <= threshold {
            final_x = area.left;
            edge = "left";
        } else if area.right - (final_x + size.width as i32) <= threshold {
            final_x = area.right - size.width as i32;
            edge = "right";
        }
    }

    if final_x != pos.x || final_y != pos.y {
        let _ = ball.set_position(Position::Physical(PhysicalPosition::new(final_x, final_y)));
    }

    {
        let mut cfg = state.config.lock();
        cfg.ui.ball_x = Some(final_x);
        cfg.ui.ball_y = Some(final_y);
        let _ = save_config(&cfg);
    }

    let _ = ball.emit(
        "ball:edge-state",
        BallEdgePayload {
            edge: edge.to_string(),
        },
    );
}

/// Spawns settings window creation off the UI thread to prevent WebView2 creation deadlock on Windows.
pub fn open_settings_window_async(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        if let Ok(win) = WebviewWindowBuilder::new(
            &handle,
            "settings",
            WebviewUrl::App("index.html".into()),
        )
        .title("BTW-Float 设置")
        .inner_size(780.0, 600.0)
        .min_inner_size(640.0, 480.0)
        .center()
        .decorations(true)
        .transparent(false)
        .visible(true)
        .build()
        {
            if let Ok(hwnd) = win.hwnd() {
                win_native::apply_win11_settings_backdrop(hwnd.0 as isize);
            }
            let _ = win.set_focus();
        }
    });
}
