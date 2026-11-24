//! Reads currently highlighted/selected text from the foreground Windows application
//! BEFORE BTW-Float's popup takes focus.
//!
//! Strategy:
//! 1. Try Windows UI Automation (`IUIAutomationTextPattern` on focused element + up to 3 parent ancestors).
//! 2. Never synthesize `Ctrl+C` in pure terminal emulators (`pwsh`, `cmd`, `WindowsTerminal`, etc.) or UIA terminal widgets.
//! 3. In IDEs with integrated terminals (`T3 Code`, `VS Code`, `Cursor`, `JetBrains`), only allow `Ctrl+C` fallback
//!    when the user JUST performed a drag-select or double-click gesture (`mouse_hook::consume_recent_select_gesture()`),
//!    so idle terminals never receive `SIGINT` while highlighted text in Electron/IDEs is 100% captured!

pub fn capture_foreground_selection() -> Option<String> {
    #[cfg(windows)]
    {
        imp::capture_selection_win32()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn is_pure_terminal_process(proc_name: &str) -> bool {
    let lower = proc_name.trim().to_ascii_lowercase();
    if lower.contains("terminal") || lower.contains("powershell") {
        return true;
    }
    matches!(
        lower.as_str(),
        "btw-float.exe"
            | "cmd.exe"
            | "pwsh.exe"
            | "conhost.exe"
            | "wt.exe"
            | "wsl.exe"
            | "bash.exe"
            | "nu.exe"
            | "alacritty.exe"
            | "wezterm-gui.exe"
            | "mintty.exe"
            | "hyper.exe"
            | "tabby.exe"
            | "warp.exe"
    )
}

pub fn is_ide_with_integrated_terminal(proc_name: &str) -> bool {
    let lower = proc_name.trim().to_ascii_lowercase();
    if lower.contains("t3") {
        return true;
    }
    matches!(
        lower.as_str(),
        "code.exe"
            | "code - insiders.exe"
            | "cursor.exe"
            | "windsurf.exe"
            | "zed.exe"
            | "idea64.exe"
            | "pycharm64.exe"
            | "webstorm64.exe"
            | "goland64.exe"
            | "rustrover64.exe"
            | "clion64.exe"
            | "rider64.exe"
            | "phpstorm64.exe"
            | "devenv.exe"
    )
}

#[cfg(windows)]
mod imp {
    use super::{is_ide_with_integrated_terminal, is_pure_terminal_process};
    use crate::{clipboard_watch, mouse_hook, win_native};
    use std::mem::size_of;
    use std::time::{Duration, Instant};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        UIA_TextPatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    const MAX_SELECTION_CHARS: usize = 12_000;

    enum UiaSelectionOutcome {
        Selected(String),
        ConfirmedEmpty,
        TerminalElement,
        Unsupported,
    }

    pub fn capture_selection_win32() -> Option<String> {
        let had_select_gesture = mouse_hook::consume_recent_select_gesture();
        let proc_name = win_native::get_foreground_process_name()?;
        if proc_name.eq_ignore_ascii_case("btw-float.exe") {
            return None;
        }

        // 1. Zero-disruption UI Automation first
        match probe_uia_selection() {
            UiaSelectionOutcome::Selected(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(truncate_chars(trimmed, MAX_SELECTION_CHARS));
                }
                return None;
            }
            UiaSelectionOutcome::TerminalElement => {
                return None;
            }
            UiaSelectionOutcome::ConfirmedEmpty => {
                // If the user explicitly drag-selected in Electron/Chromium where child element had empty TextPattern,
                // allow fallback below; otherwise stop immediately.
                if !had_select_gesture {
                    return None;
                }
            }
            UiaSelectionOutcome::Unsupported => {}
        }

        // 2. Never send synthetic Ctrl+C to standalone terminal emulators
        if is_pure_terminal_process(&proc_name) {
            return None;
        }

        // 3. In IDEs with integrated terminals (T3 Code, VS Code, Cursor, JetBrains),
        // ONLY synthesize Ctrl+C when the user actually performed a drag-select or double-click!
        if is_ide_with_integrated_terminal(&proc_name) && !had_select_gesture {
            return None;
        }

        // 4. Safe clipboard backup + Ctrl+C fallback
        try_ctrl_c_selection().map(|s| truncate_chars(s.trim(), MAX_SELECTION_CHARS))
    }

    fn truncate_chars(s: &str, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            s.to_string()
        } else {
            let mut out: String = s.chars().take(max_chars).collect();
            out.push_str("\n...(已截断)");
            out
        }
    }

    unsafe fn looks_like_terminal_element(elem: &IUIAutomationElement) -> bool {
        let check = |s: String| {
            let l = s.to_ascii_lowercase();
            l.contains("xterm")
                || l.contains("terminal")
                || l.contains("终端")
                || l.contains("console")
        };
        if let Ok(cls) = elem.CurrentClassName() {
            if check(cls.to_string()) {
                return true;
            }
        }
        if let Ok(name) = elem.CurrentName() {
            if check(name.to_string()) {
                return true;
            }
        }
        if let Ok(aid) = elem.CurrentAutomationId() {
            if check(aid.to_string()) {
                return true;
            }
        }
        false
    }

    unsafe fn read_text_pattern_on(elem: &IUIAutomationElement) -> Option<UiaSelectionOutcome> {
        let pattern = elem
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok()?;
        let Ok(ranges) = pattern.GetSelection() else {
            return Some(UiaSelectionOutcome::ConfirmedEmpty);
        };
        let count = ranges.Length().unwrap_or(0);
        if count <= 0 {
            return Some(UiaSelectionOutcome::ConfirmedEmpty);
        }

        let mut combined = String::new();
        for i in 0..count {
            if let Ok(range) = ranges.GetElement(i) {
                if let Ok(bstr) = range.GetText(MAX_SELECTION_CHARS as i32) {
                    let piece = bstr.to_string();
                    if !piece.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&piece);
                    }
                }
            }
        }

        if combined.trim().is_empty() {
            Some(UiaSelectionOutcome::ConfirmedEmpty)
        } else {
            Some(UiaSelectionOutcome::Selected(combined))
        }
    }

    fn probe_uia_selection() -> UiaSelectionOutcome {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(uia) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            else {
                return UiaSelectionOutcome::Unsupported;
            };
            let Ok(focused) = uia.GetFocusedElement() else {
                return UiaSelectionOutcome::Unsupported;
            };

            if looks_like_terminal_element(&focused) {
                return UiaSelectionOutcome::TerminalElement;
            }

            if let Some(res) = read_text_pattern_on(&focused) {
                if matches!(res, UiaSelectionOutcome::Selected(_)) {
                    return res;
                }
            }

            // Walk up to 3 ancestors (e.g. span -> p -> Document in Chromium/Electron)
            if let Ok(walker) = uia.ControlViewWalker() {
                let mut current = focused;
                for _ in 0..3 {
                    let Ok(parent) = walker.GetParentElement(&current) else {
                        break;
                    };
                    if looks_like_terminal_element(&parent) {
                        return UiaSelectionOutcome::TerminalElement;
                    }
                    if let Some(UiaSelectionOutcome::Selected(txt)) = read_text_pattern_on(&parent) {
                        return UiaSelectionOutcome::Selected(txt);
                    }
                    current = parent;
                }
            }

            UiaSelectionOutcome::Unsupported
        }
    }

    fn key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if key_up {
                        KEYEVENTF_KEYUP
                    } else {
                        Default::default()
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn try_ctrl_c_selection() -> Option<String> {
        unsafe {
            clipboard_watch::suppress_for(Duration::from_millis(600));
            let backup_text = clipboard_watch::read_clipboard_text();
            let seq_before = GetClipboardSequenceNumber();

            // Release Alt/Shift if held so target app receives pure Ctrl+C
            let mut release_mods = Vec::new();
            if (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 {
                release_mods.push(key_input(VK_MENU, true));
            }
            if (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0 {
                release_mods.push(key_input(VK_SHIFT, true));
            }
            if !release_mods.is_empty() {
                let _ = SendInput(&release_mods, size_of::<INPUT>() as i32);
                std::thread::sleep(Duration::from_millis(10));
            }

            // Stepped SendInput (Ctrl DOWN -> 18ms -> C DOWN/UP -> 18ms -> Ctrl UP)
            // Required by Chromium, Electron, Firefox (Zotero), and PDF viewers so GetKeyState(VK_CONTROL) is active!
            let ctrl_down = [key_input(VK_CONTROL, false)];
            if SendInput(&ctrl_down, size_of::<INPUT>() as i32) == 0 {
                return None;
            }
            std::thread::sleep(Duration::from_millis(18));

            let c_press = [
                key_input(VIRTUAL_KEY(b'C' as u16), false),
                key_input(VIRTUAL_KEY(b'C' as u16), true),
            ];
            let _ = SendInput(&c_press, size_of::<INPUT>() as i32);
            std::thread::sleep(Duration::from_millis(18));

            let ctrl_up = [key_input(VK_CONTROL, true)];
            let _ = SendInput(&ctrl_up, size_of::<INPUT>() as i32);

            let start = Instant::now();
            let mut changed = false;
            while start.elapsed() < Duration::from_millis(200) {
                if GetClipboardSequenceNumber() != seq_before {
                    changed = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            if !changed {
                return None;
            }

            let captured = clipboard_watch::read_clipboard_text();

            if let Some(prev) = backup_text {
                clipboard_watch::suppress_for(Duration::from_millis(250));
                clipboard_watch::write_clipboard_text(&prev);
            }

            captured.filter(|s| !s.trim().is_empty())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_ide_with_integrated_terminal, is_pure_terminal_process};

    #[test]
    fn classifies_terminals_and_ides_correctly() {
        assert!(is_pure_terminal_process("WindowsTerminal.exe"));
        assert!(is_pure_terminal_process("pwsh.exe"));
        assert!(!is_pure_terminal_process("T3 Code.exe"));

        assert!(is_ide_with_integrated_terminal("T3 Code.exe"));
        assert!(is_ide_with_integrated_terminal("Code.exe"));
        assert!(!is_ide_with_integrated_terminal("notepad.exe"));
    }
}
