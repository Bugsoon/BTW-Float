use crate::capture;
use crate::clipboard_watch;
use crate::config::{save_config, AppConfig};
use crate::hotkey;
use crate::llm::StreamRegistry;
use crate::selection;
use crate::tray;
use crate::win_native::{self, BallInputEvent};
use parking_lot::Mutex;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

// Shadow gutter sized to exceed Chromium/Skia's exact 3*sigma Gaussian blur cutoff (zero box clipping)
pub const BALL_WIN_LOGICAL_SIZE: f64 = 86.0;
const BALL_ORB_LOGICAL_SIZE: f64 = 52.0;
const BALL_GUTTER_LOGICAL: f64 = (BALL_WIN_LOGICAL_SIZE - BALL_ORB_LOGICAL_SIZE) / 2.0; // 17.0
const BALL_EDGE_SNAP_THRESHOLD_LOGICAL: f64 = 26.0;
const BALL_EDGE_MARGIN_LOGICAL: f64 = 4.0;
const BALL_PEEK_VISIBLE_LOGICAL: f64 = 18.0;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BallEdge {
    #[default]
    None,
    Left,
    Right,
}

impl BallEdge {
    fn as_str(self) -> &'static str {
        match self {
            BallEdge::None => "none",
            BallEdge::Left => "left",
            BallEdge::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BallDockState {
    pub edge: BallEdge,
    pub css_slide: bool,
    pub expanded_x: i32,
    pub tucked_x: i32,
    pub y: i32,
    pub hovered: bool,
    pub dragging: bool,
    pub tucked: bool,
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub streams: Arc<StreamRegistry>,
    pub popup_open: AtomicBool,
    pub popup_pinned: AtomicBool,
    pub last_popup_shown: Mutex<Instant>,
    pub last_popup_blur_hidden: Mutex<Option<Instant>>,
    pub pending_prefill: Mutex<Option<PopupPrefillPayload>>,
    pub ball_dock: Mutex<BallDockState>,
    pub ball_anim_gen: AtomicU64,
    pub suppressed_by_app: AtomicBool,
    pub explicit_exit: AtomicBool,
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
            ball_dock: Mutex::new(BallDockState::default()),
            ball_anim_gen: AtomicU64::new(0),
            suppressed_by_app: AtomicBool::new(false),
            explicit_exit: AtomicBool::new(false),
        }
    }
}

pub fn request_app_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.explicit_exit.store(true, Ordering::SeqCst);
    state.streams.abort_all();
    app.exit(0);
}

pub fn wake_from_second_instance(app: &AppHandle) {
    if is_suppressed_by_app(app) {
        return;
    }
    let state = app.state::<AppState>();
    if state.popup_open.load(Ordering::SeqCst) {
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(p) = app_for_main.get_webview_window("popup") {
                let _ = p.show();
                if let Ok(hwnd) = p.hwnd() {
                    win_native::show_and_focus_popup(hwnd.0 as isize);
                }
                let _ = p.set_focus();
            }
        });
    } else {
        open_popup_with_prefill(app, false, None);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BallEdgePayload {
    pub edge: String, // "none" | "left" | "right"
    pub css_slide: bool,
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
                            {
                                let state = ball_app.state::<AppState>();
                                state.ball_dock.lock().hovered = h;
                            }
                            if let Some(b) = ball_app.get_webview_window("ball") {
                                let _ = b.emit(
                                    "ball:ui-state",
                                    BallUiStatePayload {
                                        hovered: Some(h),
                                        pressed: None,
                                    },
                                );
                            }
                            if h {
                                ball_app
                                    .state::<AppState>()
                                    .ball_anim_gen
                                    .fetch_add(1, Ordering::SeqCst);
                                expand_ball_if_tucked(&ball_app);
                            } else {
                                schedule_ball_tuck(&ball_app, 240);
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
                        BallInputEvent::DragStarted => {
                            let state = ball_app.state::<AppState>();
                            state.ball_anim_gen.fetch_add(1, Ordering::SeqCst);
                            {
                                let mut dock = state.ball_dock.lock();
                                dock.dragging = true;
                                dock.edge = BallEdge::None;
                                dock.css_slide = false;
                                dock.tucked = false;
                            }
                            if let Some(b) = ball_app.get_webview_window("ball") {
                                let _ = b.emit(
                                    "ball:edge-state",
                                    BallEdgePayload {
                                        edge: "none".to_string(),
                                        css_slide: false,
                                    },
                                );
                            }
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

        let ball_close_app = app.clone();
        ball.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let explicit = ball_close_app
                    .state::<AppState>()
                    .explicit_exit
                    .load(Ordering::SeqCst);
                if !explicit {
                    api.prevent_close();
                    if let Some(b) = ball_close_app.get_webview_window("ball") {
                        let _ = b.hide();
                    }
                }
            }
        });

        evaluate_and_apply_ball_dock(app, false, true);

        let suppressed = is_foreground_process_disabled(app);
        if cfg.ui.show_ball && !suppressed {
            let _ = ball.show();
        }
    }

    if let Some(popup) = app.get_webview_window("popup") {
        if let Ok(hwnd) = popup.hwnd() {
            win_native::install_tool_window_style_lock(hwnd.0 as isize);
        }
        let scale = popup.scale_factor().unwrap_or(1.0);
        let phys_w = (POPUP_WIN_LOGICAL_WIDTH * scale).round() as u32;
        let phys_h = (POPUP_WIN_INITIAL_HEIGHT * scale).round() as u32;
        let _ = popup.set_size(Size::Physical(PhysicalSize::new(phys_w, phys_h)));
        let _ = popup.hide();

        let app_handle = app.clone();
        popup.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let explicit = app_handle
                    .state::<AppState>()
                    .explicit_exit
                    .load(Ordering::SeqCst);
                if !explicit {
                    api.prevent_close();
                    hide_popup_and_clear(&app_handle);
                }
            }
            WindowEvent::Focused(false) => {
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
            _ => {}
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
        expand_ball_if_tucked(app);
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
            let dock = *app.state::<AppState>().ball_dock.lock();
            let (ref_x, ref_y) = if dock.edge != BallEdge::None {
                (dock.expanded_x, dock.y)
            } else {
                (pos.x, pos.y)
            };
            let cx = ref_x + (size.width as i32) / 2;
            let cy = ref_y + (size.height as i32) / 2;
            let area = win_native::get_monitor_work_area_for_point(cx, cy);
            let overlap = (6.0 * scale).round() as i32;

            let x = if cx > (area.left + area.right) / 2 {
                ref_x - phys_w + overlap
            } else {
                ref_x + size.width as i32 - overlap
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
    schedule_ball_tuck(app, 240);
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
    let state = app.state::<AppState>();
    let show_ball =
        state.config.lock().ui.show_ball && !state.suppressed_by_app.load(Ordering::SeqCst);
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
                    if let Ok(hwnd) = cwin.hwnd() {
                        win_native::install_tool_window_style_lock(hwnd.0 as isize);
                    }
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
    let (visible, updated_cfg) = {
        let mut cfg = state.config.lock();
        cfg.ui.show_ball = !cfg.ui.show_ball;
        let visible = cfg.ui.show_ball;
        let _ = save_config(&cfg);
        (visible, cfg.clone())
    };

    if let Some(ball) = app.get_webview_window("ball") {
        if visible && !state.suppressed_by_app.load(Ordering::SeqCst) {
            evaluate_and_apply_ball_dock(app, false, false);
            let _ = ball.show();
        } else {
            let _ = ball.hide();
        }
    }
    let _ = app.emit("config:updated", &updated_cfg);
    tray::sync_tray_state(app);
    visible
}

pub fn toggle_autostart(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let (enabled, updated_cfg) = {
        let mut cfg = state.config.lock();
        cfg.ui.autostart = !cfg.ui.autostart;
        let enabled = cfg.ui.autostart;
        let _ = save_config(&cfg);
        (enabled, cfg.clone())
    };

    win_native::apply_autostart(enabled);
    let _ = app.emit("config:updated", &updated_cfg);
    tray::sync_tray_state(app);
    enabled
}

pub fn is_suppressed_by_app(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .suppressed_by_app
        .load(Ordering::SeqCst)
}

fn normalize_proc_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn is_foreground_process_disabled(app: &AppHandle) -> bool {
    let disabled_list: Vec<String> = app
        .state::<AppState>()
        .config
        .lock()
        .mouse_priority
        .disabled_processes
        .clone();
    if disabled_list.is_empty() {
        return false;
    }
    let Some(fg) = win_native::get_foreground_process_name() else {
        return false;
    };
    let fg_lower = fg.to_ascii_lowercase();
    if fg_lower == "btw-float.exe" {
        return false;
    }
    let fg_key = normalize_proc_key(&fg_lower);
    disabled_list
        .iter()
        .any(|p| !p.trim().is_empty() && normalize_proc_key(p) == fg_key)
}

fn apply_suppression_state(app: &AppHandle, suppressed: bool) {
    let state = app.state::<AppState>();
    let prev = state.suppressed_by_app.swap(suppressed, Ordering::SeqCst);
    if prev == suppressed {
        return;
    }

    if suppressed {
        if state.popup_open.load(Ordering::SeqCst) {
            hide_popup_and_clear(app);
        }
        if let Some(ball) = app.get_webview_window("ball") {
            if let Ok(hwnd) = ball.hwnd() {
                win_native::hide_hwnd_now(hwnd.0 as isize);
            }
        }
        hotkey::set_keyboard_shortcuts_enabled(app, false);
    } else {
        hotkey::set_keyboard_shortcuts_enabled(app, true);
        let show_ball = state.config.lock().ui.show_ball;
        if show_ball {
            if let Some(ball) = app.get_webview_window("ball") {
                if let Ok(hwnd) = ball.hwnd() {
                    win_native::show_hwnd_noactivate(hwnd.0 as isize);
                }
            }
        }
    }
}

pub fn refresh_app_suppression(app: &AppHandle) {
    let should_suppress = is_foreground_process_disabled(app);
    apply_suppression_state(app, should_suppress);
}

/// Lightweight foreground window watcher (~250ms) that auto-hides ball & popup and pauses shortcuts
/// whenever the active foreground window belongs to a user-configured disabled process (e.g. games).
pub fn start_foreground_watcher(app: &AppHandle) {
    let handle = app.clone();
    std::thread::Builder::new()
        .name("btw-fg-watcher".into())
        .spawn(move || {
            let mut last_hwnd: isize = 0;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let has_rules = !handle
                    .state::<AppState>()
                    .config
                    .lock()
                    .mouse_priority
                    .disabled_processes
                    .is_empty();
                if !has_rules {
                    if is_suppressed_by_app(&handle) {
                        apply_suppression_state(&handle, false);
                    }
                    last_hwnd = 0;
                    continue;
                }

                let cur_hwnd = win_native::get_foreground_hwnd();
                if cur_hwnd == last_hwnd {
                    continue;
                }
                last_hwnd = cur_hwnd;

                let should_suppress = is_foreground_process_disabled(&handle);
                apply_suppression_state(&handle, should_suppress);
            }
        })
        .ok();
}

pub fn get_ball_edge_state(app: &AppHandle) -> BallEdgePayload {
    let dock = *app.state::<AppState>().ball_dock.lock();
    BallEdgePayload {
        edge: dock.edge.as_str().to_string(),
        css_slide: dock.css_slide,
    }
}

fn animate_ball_x(app: &AppHandle, target_x: i32, y: i32, duration_ms: u64, gen: u64) {
    let Some(ball) = app.get_webview_window("ball") else {
        return;
    };
    let Ok(pos) = ball.outer_position() else {
        return;
    };
    let Ok(hwnd) = ball.hwnd() else {
        return;
    };
    let start_x = pos.x;
    if start_x == target_x {
        return;
    }
    let hwnd_raw = hwnd.0 as isize;
    let app_clone = app.clone();

    tauri::async_runtime::spawn(async move {
        let steps = ((duration_ms as f64) / 12.0).round().max(1.0) as u32;
        let step_dur = Duration::from_millis((duration_ms / steps as u64).max(8));
        for i in 1..=steps {
            if app_clone
                .state::<AppState>()
                .ball_anim_gen
                .load(Ordering::SeqCst)
                != gen
            {
                return;
            }
            let t = i as f64 / steps as f64;
            let eased = 1.0 - (1.0 - t).powi(3);
            let cur_x = start_x + ((target_x - start_x) as f64 * eased).round() as i32;
            win_native::set_hwnd_position(hwnd_raw, cur_x, y);
            if i < steps {
                tokio::time::sleep(step_dur).await;
            }
        }
    });
}

fn expand_ball_if_tucked(app: &AppHandle) {
    let state = app.state::<AppState>();
    let target = {
        let mut dock = state.ball_dock.lock();
        if dock.edge != BallEdge::None && !dock.css_slide && dock.tucked {
            dock.tucked = false;
            Some((dock.expanded_x, dock.y))
        } else {
            None
        }
    };
    if let Some((expanded_x, y)) = target {
        let gen = state.ball_anim_gen.fetch_add(1, Ordering::SeqCst) + 1;
        animate_ball_x(app, expanded_x, y, 150, gen);
    }
}

fn schedule_ball_tuck(app: &AppHandle, delay_ms: u64) {
    let state = app.state::<AppState>();
    let auto_hide_edge = state.config.lock().ui.ball_auto_hide_edge;
    if !auto_hide_edge {
        return;
    }
    {
        let dock = state.ball_dock.lock();
        if dock.edge == BallEdge::None || dock.css_slide || dock.hovered || dock.dragging {
            return;
        }
    }
    if state.popup_open.load(Ordering::SeqCst) {
        return;
    }

    let gen = state.ball_anim_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        let state = app_clone.state::<AppState>();
        if state.ball_anim_gen.load(Ordering::SeqCst) != gen {
            return;
        }
        if !state.config.lock().ui.ball_auto_hide_edge || state.popup_open.load(Ordering::SeqCst) {
            return;
        }
        let target = {
            let mut dock = state.ball_dock.lock();
            if dock.edge == BallEdge::None || dock.css_slide || dock.hovered || dock.dragging {
                return;
            }
            dock.tucked = true;
            (dock.tucked_x, dock.y)
        };
        animate_ball_x(&app_clone, target.0, target.1, 200, gen);
    });
}

fn evaluate_and_apply_ball_dock(
    app: &AppHandle,
    save_pos: bool,
    allow_immediate_tuck: bool,
) {
    let Some(ball) = app.get_webview_window("ball") else {
        return;
    };
    let Ok(pos) = ball.outer_position() else {
        return;
    };
    let Ok(size) = ball.outer_size() else {
        return;
    };
    let scale = ball.scale_factor().unwrap_or(1.0);
    let phys_w = size.width as i32;
    let phys_h = size.height as i32;

    let state = app.state::<AppState>();
    state.ball_anim_gen.fetch_add(1, Ordering::SeqCst);

    // If ball was previously tucked off-edge, evaluate using its expanded anchor position
    let prev_dock = *state.ball_dock.lock();
    let eval_x = if prev_dock.tucked && prev_dock.edge != BallEdge::None {
        prev_dock.expanded_x
    } else {
        pos.x
    };

    let cx = eval_x + phys_w / 2;
    let cy = pos.y + phys_h / 2;
    let area = win_native::get_monitor_work_area_for_point(cx, cy);
    let auto_hide_edge = state.config.lock().ui.ball_auto_hide_edge;

    let gutter = (BALL_GUTTER_LOGICAL * scale).round() as i32;
    let margin = (BALL_EDGE_MARGIN_LOGICAL * scale).round() as i32;
    let threshold = (BALL_EDGE_SNAP_THRESHOLD_LOGICAL * scale).round() as i32;
    let peek_visible = (BALL_PEEK_VISIBLE_LOGICAL * scale).round() as i32;
    let orb_phys = (BALL_ORB_LOGICAL_SIZE * scale).round() as i32;

    let min_x = area.left - gutter + margin;
    let max_x = area.right - phys_w + gutter - margin;
    let clamped_x = eval_x.clamp(min_x, max_x.max(min_x));
    let final_y = pos.y.clamp(area.top + 4, (area.bottom - phys_h - 4).max(area.top + 4));

    let orb_left = clamped_x + gutter;
    let orb_right = clamped_x + phys_w - gutter;

    let mut edge = BallEdge::None;
    let mut css_slide = false;
    let mut expanded_x = clamped_x;
    let mut tucked_x = clamped_x;

    if auto_hide_edge {
        if orb_left - area.left <= threshold {
            edge = BallEdge::Left;
            css_slide = win_native::has_adjacent_monitor_on_side(cx, cy, false);
            if css_slide {
                expanded_x = area.left;
                tucked_x = area.left;
            } else {
                expanded_x = area.left - gutter + margin;
                tucked_x = area.left - (orb_phys - peek_visible + gutter);
            }
        } else if area.right - orb_right <= threshold {
            edge = BallEdge::Right;
            css_slide = win_native::has_adjacent_monitor_on_side(cx, cy, true);
            if css_slide {
                expanded_x = area.right - phys_w;
                tucked_x = area.right - phys_w;
            } else {
                expanded_x = area.right - phys_w + gutter - margin;
                tucked_x = area.right - (peek_visible + gutter);
            }
        }
    }

    let (mx, my) = win_native::get_cursor_pos();
    let cursor_inside = mx >= expanded_x
        && mx <= expanded_x + phys_w
        && my >= final_y
        && my <= final_y + phys_h;

    let should_tuck_now = allow_immediate_tuck
        && edge != BallEdge::None
        && !css_slide
        && !cursor_inside
        && !state.popup_open.load(Ordering::SeqCst);

    let target_x = if should_tuck_now { tucked_x } else { expanded_x };

    {
        let mut dock = state.ball_dock.lock();
        dock.edge = edge;
        dock.css_slide = css_slide;
        dock.expanded_x = expanded_x;
        dock.tucked_x = tucked_x;
        dock.y = final_y;
        dock.dragging = false;
        dock.hovered = cursor_inside;
        dock.tucked = should_tuck_now;
    }

    if target_x != pos.x || final_y != pos.y {
        let _ = ball.set_position(Position::Physical(PhysicalPosition::new(target_x, final_y)));
    }

    if save_pos {
        let mut cfg = state.config.lock();
        cfg.ui.ball_x = Some(expanded_x);
        cfg.ui.ball_y = Some(final_y);
        let _ = save_config(&cfg);
    }

    let _ = ball.emit(
        "ball:ui-state",
        BallUiStatePayload {
            hovered: Some(cursor_inside),
            pressed: Some(false),
        },
    );
    let _ = ball.emit(
        "ball:edge-state",
        BallEdgePayload {
            edge: edge.as_str().to_string(),
            css_slide,
        },
    );

    if !should_tuck_now && edge != BallEdge::None && !css_slide && !cursor_inside {
        schedule_ball_tuck(app, 220);
    }
}

pub fn apply_ball_edge_setting(app: &AppHandle) {
    evaluate_and_apply_ball_dock(app, true, false);
}

pub fn sync_ball_after_drag(app: &AppHandle) {
    evaluate_and_apply_ball_dock(app, true, false);
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
        .theme(Some(tauri::Theme::Light))
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
