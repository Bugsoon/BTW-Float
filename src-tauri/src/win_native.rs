#[cfg(not(windows))]
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone)]
pub enum BallInputEvent {
    Hover(bool),
    Press(bool),
    LeftClick,
    RightClick,
    DragEnded,
    DroppedFiles(Vec<String>),
}

#[cfg(windows)]
mod imp {
    use super::{BallInputEvent, WorkArea};
    use parking_lot::Mutex;
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::OnceLock;
    use tokio::sync::mpsc::UnboundedSender;
    use windows::Win32::Foundation::{
        BOOL, CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO,
        MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE,
    };
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        EnableWindow, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    };
    use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, CallWindowProcW, EnumChildWindows, GetClassNameW, GetCursorPos,
        GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, SetForegroundWindow,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_WNDPROC, MA_NOACTIVATE, SWP_NOACTIVATE,
        SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, WM_CREATE, WM_DROPFILES,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PARENTNOTIFY,
        WM_RBUTTONUP, WNDPROC,
    };

    const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

    struct BallSubclassState {
        orig_proc: isize,
        tx: Option<UnboundedSender<BallInputEvent>>,
        tracking_leave: bool,
        is_down: bool,
        is_dragging: bool,
        down_cursor: POINT,
        down_win: POINT,
    }

    static BALL_STATE: OnceLock<Mutex<BallSubclassState>> = OnceLock::new();

    fn ball_state() -> &'static Mutex<BallSubclassState> {
        BALL_STATE.get_or_init(|| {
            Mutex::new(BallSubclassState {
                orig_proc: 0,
                tx: None,
                tracking_leave: false,
                is_down: false,
                is_dragging: false,
                down_cursor: POINT::default(),
                down_win: POINT::default(),
            })
        })
    }

    unsafe extern "system" fn disable_child_proc(child: HWND, _: LPARAM) -> BOOL {
        let _ = EnableWindow(child, false);
        BOOL(1)
    }

    pub fn disable_ball_child_hit_test(hwnd_raw: isize) {
        unsafe {
            let hwnd = HWND(hwnd_raw as _);
            let _ = EnumChildWindows(hwnd, Some(disable_child_proc), LPARAM(0));
        }
    }

    /// Subclasses the `ball` top-level HWND and disables mouse hit-testing on its WebView2 child HWNDs.
    /// This guarantees 100% instant response on the VERY FIRST click after launch (WebView2 never eats
    /// cold-focus clicks), zero focus theft (`MA_NOACTIVATE`), and native 0ms `SetWindowPos` dragging.
    pub fn install_ball_input_handler(hwnd_raw: isize, tx: UnboundedSender<BallInputEvent>) {
        crate::mouse_hook::set_ball_hwnd(hwnd_raw);
        disable_ball_child_hit_test(hwnd_raw);

        unsafe {
            let hwnd = HWND(hwnd_raw as _);
            DragAcceptFiles(hwnd, true);
            let prev =
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, ball_wnd_proc as *const () as usize as isize);
            let mut st = ball_state().lock();
            st.orig_proc = prev;
            st.tx = Some(tx);
        }

        // WebView2 creates Chrome_WidgetWin_0 asynchronously (~100-400ms after setup);
        // re-disable child hit-testing so clicks always reach `ball_wnd_proc` directly.
        std::thread::spawn(move || {
            for delay_ms in [80u64, 220, 500, 1000] {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                disable_ball_child_hit_test(hwnd_raw);
            }
        });
    }

    unsafe extern "system" fn ball_wnd_proc(
        hwnd: HWND,
        msg: u32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => {
                return LRESULT(MA_NOACTIVATE as isize);
            }
            WM_PARENTNOTIFY => {
                if (w_param.0 & 0xFFFF) as u32 == WM_CREATE {
                    let _ = EnumChildWindows(hwnd, Some(disable_child_proc), LPARAM(0));
                }
            }
            WM_MOUSEMOVE => {
                let mut emit_hover = false;
                let mut move_target: Option<(i32, i32)> = None;
                let mut emit_unpress = false;
                let tx_clone;

                {
                    let mut st = ball_state().lock();
                    tx_clone = st.tx.clone();
                    if !st.tracking_leave {
                        st.tracking_leave = true;
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut tme);
                        emit_hover = true;
                    }

                    if st.is_down {
                        let mut pt = POINT::default();
                        if GetCursorPos(&mut pt).is_ok() {
                            let dx = pt.x - st.down_cursor.x;
                            let dy = pt.y - st.down_cursor.y;
                            if !st.is_dragging && (dx * dx + dy * dy) > 25 {
                                st.is_dragging = true;
                                emit_unpress = true;
                            }
                            if st.is_dragging {
                                move_target = Some((st.down_win.x + dx, st.down_win.y + dy));
                            }
                        }
                    }
                }

                if let Some((nx, ny)) = move_target {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        nx,
                        ny,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                if let Some(tx) = tx_clone {
                    if emit_hover {
                        let _ = tx.send(BallInputEvent::Hover(true));
                    }
                    if emit_unpress {
                        let _ = tx.send(BallInputEvent::Press(false));
                    }
                }
                return LRESULT(0);
            }
            WM_MOUSELEAVE_MSG => {
                let tx_clone = {
                    let mut st = ball_state().lock();
                    st.tracking_leave = false;
                    st.tx.clone()
                };
                if let Some(tx) = tx_clone {
                    let _ = tx.send(BallInputEvent::Hover(false));
                }
                return LRESULT(0);
            }
            WM_LBUTTONDOWN => {
                let _ = SetCapture(hwnd);
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);

                let tx_clone = {
                    let mut st = ball_state().lock();
                    st.is_down = true;
                    st.is_dragging = false;
                    st.down_cursor = pt;
                    st.down_win = POINT {
                        x: rect.left,
                        y: rect.top,
                    };
                    st.tx.clone()
                };
                if let Some(tx) = tx_clone {
                    let _ = tx.send(BallInputEvent::Press(true));
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let (was_down, was_dragging, tx_clone) = {
                    let mut st = ball_state().lock();
                    let wd = st.is_down;
                    let wdrag = st.is_dragging;
                    st.is_down = false;
                    st.is_dragging = false;
                    (wd, wdrag, st.tx.clone())
                };
                if was_down {
                    let _ = ReleaseCapture();
                    if let Some(tx) = tx_clone {
                        let _ = tx.send(BallInputEvent::Press(false));
                        if was_dragging {
                            let _ = tx.send(BallInputEvent::DragEnded);
                        } else {
                            let _ = tx.send(BallInputEvent::LeftClick);
                        }
                    }
                    return LRESULT(0);
                }
            }
            WM_RBUTTONUP => {
                let tx_clone = ball_state().lock().tx.clone();
                if let Some(tx) = tx_clone {
                    let _ = tx.send(BallInputEvent::RightClick);
                }
                return LRESULT(0);
            }
            WM_DROPFILES => {
                let hdrop = HDROP(w_param.0 as *mut c_void);
                let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
                let mut files = Vec::new();
                for idx in 0..count {
                    let len = DragQueryFileW(hdrop, idx, None);
                    if len > 0 {
                        let mut buf = vec![0u16; (len + 1) as usize];
                        let read = DragQueryFileW(hdrop, idx, Some(&mut buf));
                        if read > 0 {
                            files.push(String::from_utf16_lossy(&buf[..read as usize]));
                        }
                    }
                }
                DragFinish(hdrop);
                if !files.is_empty() {
                    if let Some(tx) = ball_state().lock().tx.clone() {
                        let _ = tx.send(BallInputEvent::DroppedFiles(files));
                    }
                }
                return LRESULT(0);
            }
            _ => {}
        }

        let orig = ball_state().lock().orig_proc;
        if orig != 0 {
            let proc: WNDPROC = std::mem::transmute(orig);
            CallWindowProcW(proc, hwnd, msg, w_param, l_param)
        } else {
            LRESULT(0)
        }
    }

    pub fn hide_hwnd_now(hwnd_raw: isize) {
        unsafe {
            let _ = ShowWindow(HWND(hwnd_raw as _), SW_HIDE);
        }
    }

    pub fn show_hwnd_noactivate(hwnd_raw: isize) {
        unsafe {
            let _ = ShowWindow(HWND(hwnd_raw as _), SW_SHOWNOACTIVATE);
        }
    }

    /// Synchronously shows `popup` and forces foreground keyboard focus on Windows 11.
    pub fn show_and_focus_popup(hwnd_raw: isize) {
        unsafe {
            let hwnd = HWND(hwnd_raw as _);
            let _ = ShowWindow(hwnd, SW_SHOW);

            let fg_hwnd = GetForegroundWindow();
            let cur_tid = GetCurrentThreadId();
            let fg_tid = if !fg_hwnd.0.is_null() {
                GetWindowThreadProcessId(fg_hwnd, None)
            } else {
                0
            };

            let attached = if fg_tid != 0 && fg_tid != cur_tid {
                AttachThreadInput(fg_tid, cur_tid, true).as_bool()
            } else {
                false
            };

            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);

            if attached {
                let _ = AttachThreadInput(fg_tid, cur_tid, false);
            }
        }
    }

    /// Enables Win11 native anti-aliased rounded corners (DWMWCP_ROUND).
    pub fn apply_win11_rounded_corners(hwnd_raw: isize) {
        unsafe {
            let hwnd = HWND(hwnd_raw as _);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const c_void,
                size_of::<u32>() as u32,
            );
        }
    }

    /// Applies Win11 dark immersive titlebar & rounded corners for the Settings window.
    pub fn apply_win11_settings_backdrop(hwnd_raw: isize) {
        unsafe {
            let hwnd = HWND(hwnd_raw as _);
            let dark: u32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );
            apply_win11_rounded_corners(hwnd_raw);
        }
    }

    pub fn get_cursor_pos() -> (i32, i32) {
        unsafe {
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_ok() {
                (pt.x, pt.y)
            } else {
                (960, 540)
            }
        }
    }

    pub fn get_monitor_work_area_for_point(x: i32, y: i32) -> WorkArea {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(hmon, &mut info).as_bool() {
                WorkArea {
                    left: info.rcWork.left,
                    top: info.rcWork.top,
                    right: info.rcWork.right,
                    bottom: info.rcWork.bottom,
                }
            } else {
                WorkArea {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                }
            }
        }
    }

    pub fn get_foreground_process_name() -> Option<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(handle);
            if !ok || len == 0 {
                return None;
            }
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit(['\\', '/']).next().map(String::from)
        }
    }

    pub fn is_foreground_fullscreen() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return false;
            }
            let mut class_buf = [0u16; 64];
            let class_len = GetClassNameW(hwnd, &mut class_buf);
            if class_len > 0 {
                let cls = String::from_utf16_lossy(&class_buf[..class_len as usize]);
                if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
                    return false;
                }
            }

            let mut win_rect = RECT::default();
            if GetWindowRect(hwnd, &mut win_rect).is_err() {
                return false;
            }
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(hmon, &mut info).as_bool() {
                return false;
            }
            let mr = info.rcMonitor;
            win_rect.left <= mr.left
                && win_rect.top <= mr.top
                && win_rect.right >= mr.right
                && win_rect.bottom >= mr.bottom
        }
    }

    pub fn list_running_process_names() -> Vec<String> {
        let mut names = BTreeSet::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return Vec::new();
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    let nul_pos = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..nul_pos]);
                    if name.to_ascii_lowercase().ends_with(".exe")
                        && !name.eq_ignore_ascii_case("btw-float.exe")
                        && !name.eq_ignore_ascii_case("svchost.exe")
                        && !name.eq_ignore_ascii_case("System")
                    {
                        names.insert(name);
                    }
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        names.into_iter().collect()
    }

    /// Reads the active Windows WinINet system proxy (`http://127.0.0.1:7897` etc.) when ProxyEnable == 1.
    pub fn get_system_proxy_url() -> Option<String> {
        unsafe {
            let subkey: Vec<u16> =
                "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0"
                    .encode_utf16()
                    .collect();
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                windows::core::PCWSTR(subkey.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            )
            .is_err()
            {
                return None;
            }

            let enable_name: Vec<u16> = "ProxyEnable\0".encode_utf16().collect();
            let mut enable_val: u32 = 0;
            let mut cb_enable = size_of::<u32>() as u32;
            let mut vtype = REG_VALUE_TYPE::default();
            let enabled = RegQueryValueExW(
                hkey,
                windows::core::PCWSTR(enable_name.as_ptr()),
                None,
                Some(&mut vtype),
                Some((&mut enable_val as *mut u32).cast::<u8>()),
                Some(&mut cb_enable),
            )
            .is_ok()
                && enable_val != 0;

            if !enabled {
                let _ = RegCloseKey(hkey);
                return None;
            }

            let server_name: Vec<u16> = "ProxyServer\0".encode_utf16().collect();
            let mut buf = [0u16; 256];
            let mut cb_server = (buf.len() * 2) as u32;
            let ok = RegQueryValueExW(
                hkey,
                windows::core::PCWSTR(server_name.as_ptr()),
                None,
                Some(&mut vtype),
                Some(buf.as_mut_ptr().cast::<u8>()),
                Some(&mut cb_server),
            )
            .is_ok();
            let _ = RegCloseKey(hkey);

            if !ok || cb_server < 2 {
                return None;
            }
            let len_u16 = (cb_server as usize) / 2;
            let nul = buf[..len_u16]
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(len_u16);
            let raw = String::from_utf16_lossy(&buf[..nul]).trim().to_string();
            if raw.is_empty() {
                return None;
            }
            // Handle "http=127.0.0.1:7890;https=..." or plain "127.0.0.1:7897"
            let candidate = raw
                .split(';')
                .find_map(|part| {
                    part.strip_prefix("https=")
                        .or_else(|| part.strip_prefix("http="))
                })
                .unwrap_or(raw.as_str())
                .trim();

            if candidate.starts_with("http://")
                || candidate.starts_with("https://")
                || candidate.starts_with("socks5://")
            {
                Some(candidate.to_string())
            } else {
                Some(format!("http://{candidate}"))
            }
        }
    }

    /// Registers or removes BTW-Float in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
    pub fn apply_autostart(enabled: bool) {
        unsafe {
            let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
                .encode_utf16()
                .collect();
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                windows::core::PCWSTR(subkey.as_ptr()),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
            .is_err()
            {
                return;
            }

            let val_name: Vec<u16> = "BTW-Float\0".encode_utf16().collect();
            if enabled {
                if let Ok(exe) = std::env::current_exe() {
                    let quoted = format!("\"{}\"\0", exe.to_string_lossy());
                    let wide: Vec<u16> = quoted.encode_utf16().collect();
                    let bytes =
                        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
                    let _ = RegSetValueExW(
                        hkey,
                        windows::core::PCWSTR(val_name.as_ptr()),
                        0,
                        REG_SZ,
                        Some(bytes),
                    );
                }
            } else {
                let _ = RegDeleteValueW(hkey, windows::core::PCWSTR(val_name.as_ptr()));
            }

            let _ = RegCloseKey(hkey);
        }
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub fn install_ball_input_handler(_: isize, _: UnboundedSender<BallInputEvent>) {}
#[cfg(not(windows))]
pub fn hide_hwnd_now(_: isize) {}
#[cfg(not(windows))]
pub fn show_hwnd_noactivate(_: isize) {}
#[cfg(not(windows))]
pub fn show_and_focus_popup(_: isize) {}
#[cfg(not(windows))]
pub fn apply_win11_rounded_corners(_: isize) {}
#[cfg(not(windows))]
pub fn apply_win11_settings_backdrop(_: isize) {}
#[cfg(not(windows))]
pub fn get_cursor_pos() -> (i32, i32) {
    (960, 540)
}
#[cfg(not(windows))]
pub fn get_monitor_work_area_for_point(_: i32, _: i32) -> WorkArea {
    WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    }
}
#[cfg(not(windows))]
pub fn get_foreground_process_name() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn is_foreground_fullscreen() -> bool {
    false
}
#[cfg(not(windows))]
pub fn list_running_process_names() -> Vec<String> {
    Vec::new()
}
#[cfg(not(windows))]
pub fn get_system_proxy_url() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn apply_autostart(_: bool) {}
