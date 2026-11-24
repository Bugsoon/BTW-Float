use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    TogglePopup,
    CaptureRegion,
    CaptureWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseBinding {
    /// 1 = XButton1 (Back), 2 = XButton2 (Forward), 0 = Either Side Button (Smart Dual)
    pub button: u16,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub super_key: bool,
}

impl MouseBinding {
    fn matches(&self, pressed_btn: u16, alt: bool, ctrl: bool, shift: bool, super_key: bool) -> bool {
        let btn_ok = self.button == 0 || self.button == pressed_btn;
        btn_ok
            && self.alt == alt
            && self.ctrl == ctrl
            && self.shift == shift
            && self.super_key == super_key
    }
}

#[derive(Debug, Clone)]
pub enum MouseHookEvent {
    Action(HotkeyAction),
    Captured(String),
}

struct HookState {
    bindings: Vec<(MouseBinding, HotkeyAction)>,
    passthrough_fullscreen: bool,
    blacklist_lower: HashSet<String>,
    freshness_secs: u64,
    recording: bool,
}

impl Default for HookState {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
            passthrough_fullscreen: true,
            blacklist_lower: HashSet::new(),
            freshness_secs: 90,
            recording: false,
        }
    }
}

static HOOK_STATE: OnceLock<RwLock<HookState>> = OnceLock::new();
static EVENT_TX: OnceLock<UnboundedSender<MouseHookEvent>> = OnceLock::new();

// Pending actions stored on XBUTTONDOWN and dispatched cleanly on XBUTTONUP (once button is released!)
static PENDING_X1: OnceLock<RwLock<Option<HotkeyAction>>> = OnceLock::new();
static PENDING_X2: OnceLock<RwLock<Option<HotkeyAction>>> = OnceLock::new();
static SWALLOW_UP_X1: AtomicBool = AtomicBool::new(false);
static SWALLOW_UP_X2: AtomicBool = AtomicBool::new(false);

// Tracks whether the user recently performed a drag-select or double-click text selection gesture
static BALL_HWND: AtomicIsize = AtomicIsize::new(0);
static LDOWN_X: AtomicI32 = AtomicI32::new(0);
static LDOWN_Y: AtomicI32 = AtomicI32::new(0);
static LAST_LUP_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SELECT_GESTURE_MS: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static RwLock<HookState> {
    HOOK_STATE.get_or_init(|| RwLock::new(HookState::default()))
}

fn pending_slot(btn: u16) -> &'static RwLock<Option<HotkeyAction>> {
    if btn == 1 {
        PENDING_X1.get_or_init(|| RwLock::new(None))
    } else {
        PENDING_X2.get_or_init(|| RwLock::new(None))
    }
}

pub fn set_ball_hwnd(hwnd: isize) {
    BALL_HWND.store(hwnd, Ordering::Relaxed);
}

pub fn peek_recent_select_gesture() -> bool {
    let ts = LAST_SELECT_GESTURE_MS.load(Ordering::SeqCst);
    ts != 0 && now_millis().saturating_sub(ts) <= 30_000
}

pub fn consume_recent_select_gesture() -> bool {
    let ts = LAST_SELECT_GESTURE_MS.swap(0, Ordering::SeqCst);
    ts != 0 && now_millis().saturating_sub(ts) <= 30_000
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn parse_mouse_binding(raw: &str) -> Option<MouseBinding> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('+').map(|s| s.trim()).collect();
    let last = parts.last()?.to_ascii_lowercase();
    let button = match last.as_str() {
        "xbutton1" | "mouse4" | "side1" => 1,
        "xbutton2" | "mouse5" | "side2" => 2,
        "xbutton1/xbutton2" | "xbutton" | "side" | "anyside" => 0,
        _ => return None,
    };
    let mut b = MouseBinding {
        button,
        alt: false,
        ctrl: false,
        shift: false,
        super_key: false,
    };
    for &mod_part in &parts[..parts.len() - 1] {
        match mod_part.to_ascii_lowercase().as_str() {
            "alt" => b.alt = true,
            "ctrl" | "control" => b.ctrl = true,
            "shift" => b.shift = true,
            "super" | "win" | "meta" | "cmd" => b.super_key = true,
            _ => return None,
        }
    }
    Some(b)
}

pub fn format_mouse_binding(b: &MouseBinding) -> String {
    let mut parts = Vec::new();
    if b.ctrl {
        parts.push("Ctrl");
    }
    if b.alt {
        parts.push("Alt");
    }
    if b.shift {
        parts.push("Shift");
    }
    if b.super_key {
        parts.push("Super");
    }
    parts.push(match b.button {
        1 => "XButton1",
        2 => "XButton2",
        _ => "XButton1/XButton2",
    });
    parts.join("+")
}

pub fn update_hook_config(
    bindings: Vec<(MouseBinding, HotkeyAction)>,
    passthrough_fullscreen: bool,
    blacklist_processes: &[String],
    freshness_secs: u64,
) {
    let mut guard = state().write();
    guard.bindings = bindings;
    guard.passthrough_fullscreen = passthrough_fullscreen;
    guard.freshness_secs = freshness_secs;
    guard.blacklist_lower = blacklist_processes
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
}

pub fn set_recording_mode(recording: bool) {
    state().write().recording = recording;
}

pub fn start_mouse_hook(tx: UnboundedSender<MouseHookEvent>) {
    let _ = EVENT_TX.set(tx);
    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .name("btw-mouse-hook".into())
            .spawn(|| unsafe {
                run_win32_hook_loop();
            })
            .ok();
    }
}

#[cfg(windows)]
fn is_browser_or_explorer(lower_proc: &str) -> bool {
    matches!(
        lower_proc,
        "msedge.exe"
            | "chrome.exe"
            | "firefox.exe"
            | "brave.exe"
            | "arc.exe"
            | "vivaldi.exe"
            | "opera.exe"
            | "explorer.exe"
    )
}

#[cfg(windows)]
unsafe fn run_win32_hook_loop() {
    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
        MSG, WH_MOUSE_LL,
    };

    let Ok(hook) =
        SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), HINSTANCE::default(), 0)
    else {
        return;
    };

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let _ = UnhookWindowsHookEx(hook);
}

#[cfg(windows)]
unsafe extern "system" fn low_level_mouse_proc(
    n_code: i32,
    w_param: windows::Win32::Foundation::WPARAM,
    l_param: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use crate::{clipboard_watch, win_native};
    use windows::Win32::Foundation::{HWND, LRESULT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetAncestor, WindowFromPoint, GA_ROOT, HHOOK, MSLLHOOKSTRUCT,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    if n_code >= 0 {
        let msg = w_param.0 as u32;
        let info = &*(l_param.0 as *const MSLLHOOKSTRUCT);

        // Track left-mouse drag-select / double-click text selection gestures passively
        if msg == WM_LBUTTONDOWN {
            LDOWN_X.store(info.pt.x, Ordering::Relaxed);
            LDOWN_Y.store(info.pt.y, Ordering::Relaxed);
        } else if msg == WM_LBUTTONUP {
            let ball_raw = BALL_HWND.load(Ordering::Relaxed);
            let hit_hwnd = WindowFromPoint(info.pt);
            let root_hwnd = if hit_hwnd.0.is_null() {
                HWND::default()
            } else {
                GetAncestor(hit_hwnd, GA_ROOT)
            };
            let is_ball_click = ball_raw != 0
                && (hit_hwnd.0 as isize == ball_raw || root_hwnd.0 as isize == ball_raw);

            if !is_ball_click {
                let dx = info.pt.x - LDOWN_X.load(Ordering::Relaxed);
                let dy = info.pt.y - LDOWN_Y.load(Ordering::Relaxed);
                let dist_sq = dx * dx + dy * dy;
                let now = now_millis();
                let prev = LAST_LUP_MS.swap(now, Ordering::Relaxed);
                let is_dbl_click = now.saturating_sub(prev) <= 420 && dist_sq <= 36;

                if dist_sq >= 36 || is_dbl_click {
                    LAST_SELECT_GESTURE_MS.store(now, Ordering::SeqCst);
                } else {
                    LAST_SELECT_GESTURE_MS.store(0, Ordering::SeqCst);
                }
            }
        } else if msg == WM_XBUTTONDOWN || msg == WM_XBUTTONUP {
            let btn = ((info.mouseData >> 16) & 0xFFFF) as u16;
            if btn == 1 || btn == 2 {
                if msg == WM_XBUTTONUP {
                    let swallow = if btn == 1 {
                        SWALLOW_UP_X1.swap(false, Ordering::SeqCst)
                    } else {
                        SWALLOW_UP_X2.swap(false, Ordering::SeqCst)
                    };
                    if let Some(action) = pending_slot(btn).write().take() {
                        if let Some(tx) = EVENT_TX.get() {
                            let _ = tx.send(MouseHookEvent::Action(action));
                        }
                    }
                    if swallow {
                        return LRESULT(1);
                    }
                    return CallNextHookEx(HHOOK::default(), n_code, w_param, l_param);
                }

                // WM_XBUTTONDOWN
                let alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                let shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                let super_key = ((GetAsyncKeyState(VK_LWIN.0 as i32) as u16 & 0x8000) != 0)
                    || ((GetAsyncKeyState(VK_RWIN.0 as i32) as u16 & 0x8000) != 0);

                let guard = state().read();
                if guard.recording {
                    let exact = MouseBinding {
                        button: btn,
                        alt,
                        ctrl,
                        shift,
                        super_key,
                    };
                    let label = format_mouse_binding(&exact);
                    drop(guard);
                    if let Some(tx) = EVENT_TX.get() {
                        let _ = tx.send(MouseHookEvent::Captured(label));
                    }
                    mark_swallow_up(btn);
                    return LRESULT(1);
                }

                if let Some((matched_binding, action)) = guard
                    .bindings
                    .iter()
                    .find(|(b, _)| b.matches(btn, alt, ctrl, shift, super_key))
                {
                    let action = *action;
                    let is_dual_mode = matched_binding.button == 0;
                    let passthrough_fs = guard.passthrough_fullscreen;
                    let freshness_secs = guard.freshness_secs;
                    let fg_lower = win_native::get_foreground_process_name()
                        .map(|s| s.to_ascii_lowercase())
                        .unwrap_or_default();

                    let is_own_app = fg_lower == "btw-float.exe";
                    let in_user_blacklist = !is_own_app && guard.blacklist_lower.contains(&fg_lower);
                    drop(guard);

                    if (passthrough_fs && !is_own_app && win_native::is_foreground_fullscreen())
                        || in_user_blacklist
                    {
                        return CallNextHookEx(HHOOK::default(), n_code, w_param, l_param);
                    }

                    // Smart Context Priority:
                    // If the user JUST highlighted text OR has a fresh screenshot waiting on clipboard,
                    // trigger BTW-Float! Otherwise, in dual-button mode inside browsers/Explorer,
                    // let the Back side button (XButton1) keep its native Back navigation!
                    let has_active_context = peek_recent_select_gesture()
                        || clipboard_watch::has_fresh_clipboard_image(freshness_secs);

                    if !is_own_app
                        && !has_active_context
                        && is_dual_mode
                        && btn == 1
                        && is_browser_or_explorer(&fg_lower)
                    {
                        return CallNextHookEx(HHOOK::default(), n_code, w_param, l_param);
                    }

                    // Store pending action and fire it on WM_XBUTTONUP so no mouse buttons are down during Ctrl+C!
                    *pending_slot(btn).write() = Some(action);
                    mark_swallow_up(btn);
                    return LRESULT(1);
                }
            }
        }
    }

    CallNextHookEx(HHOOK::default(), n_code, w_param, l_param)
}

#[cfg(windows)]
fn mark_swallow_up(btn: u16) {
    if btn == 1 {
        SWALLOW_UP_X1.store(true, Ordering::SeqCst);
    } else if btn == 2 {
        SWALLOW_UP_X2.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{format_mouse_binding, parse_mouse_binding};

    #[test]
    fn parses_and_formats_side_buttons() {
        let b1 = parse_mouse_binding("XButton1").expect("should parse XButton1");
        assert_eq!(b1.button, 1);
        assert!(!b1.alt && !b1.ctrl);
        assert_eq!(format_mouse_binding(&b1), "XButton1");

        let b_dual = parse_mouse_binding("XButton1/XButton2").expect("should parse dual");
        assert_eq!(b_dual.button, 0);
        assert!(b_dual.matches(1, false, false, false, false));
        assert!(b_dual.matches(2, false, false, false, false));

        let b2 = parse_mouse_binding("Alt+Ctrl+XButton2").expect("should parse combo");
        assert_eq!(b2.button, 2);
        assert!(b2.alt && b2.ctrl);
        assert_eq!(format_mouse_binding(&b2), "Ctrl+Alt+XButton2");

        assert!(parse_mouse_binding("Alt+Space").is_none());
    }
}
