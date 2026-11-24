use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, ExtendedColorType, RgbImage};
use parking_lot::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

struct TrackerState {
    last_image_seq: u32,
    last_image_at: Option<Instant>,
    consumed_seq: Option<u32>,
    suppress_until: Option<Instant>,
}

static TRACKER: OnceLock<Mutex<TrackerState>> = OnceLock::new();

fn tracker() -> &'static Mutex<TrackerState> {
    TRACKER.get_or_init(|| {
        Mutex::new(TrackerState {
            last_image_seq: 0,
            last_image_at: None,
            consumed_seq: None,
            suppress_until: None,
        })
    })
}

/// Temporarily suppresses clipboard-update tracking while BTW performs safe Ctrl+C backup/restore.
pub fn suppress_for(duration: Duration) {
    tracker().lock().suppress_until = Some(Instant::now() + duration);
}

pub fn encode_rgb_to_jpeg_data_url(img: RgbImage, max_edge: u32, quality: u8) -> Option<String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let dyn_img = DynamicImage::ImageRgb8(img);
    let resized = if w.max(h) > max_edge {
        dyn_img.resize(max_edge, max_edge, FilterType::Triangle)
    } else {
        dyn_img
    };
    let rgb = resized.to_rgb8();
    let (rw, rh) = rgb.dimensions();
    let mut buf = Vec::with_capacity(32 * 1024);
    let mut enc = JpegEncoder::new_with_quality(&mut buf, quality);
    enc.encode(rgb.as_raw(), rw, rh, ExtendedColorType::Rgb8)
        .ok()?;
    Some(format!("data:image/jpeg;base64,{}", BASE64.encode(&buf)))
}

pub fn start_clipboard_watcher() {
    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .name("btw-clipboard-watch".into())
            .spawn(|| unsafe {
                imp::run_clipboard_listener_loop();
            })
            .ok();
    }
}

/// Fast non-consuming check used by the mouse hook to see if a fresh screenshot is waiting on the clipboard.
pub fn has_fresh_clipboard_image(max_age_secs: u64) -> bool {
    let st = tracker().lock();
    let Some(added_at) = st.last_image_at else {
        return false;
    };
    if added_at.elapsed() > Duration::from_secs(max_age_secs) {
        return false;
    }
    st.last_image_seq != 0 && st.consumed_seq != Some(st.last_image_seq)
}

/// Returns a fresh, unconsumed clipboard screenshot (`data:image/jpeg;base64,...`) if copied within `max_age_secs`.
pub fn take_fresh_clipboard_image(max_age_secs: u64) -> Option<String> {
    #[cfg(windows)]
    {
        imp::take_fresh_clipboard_image_win32(max_age_secs)
    }
    #[cfg(not(windows))]
    {
        let _ = max_age_secs;
        None
    }
}

#[cfg(windows)]
pub fn read_clipboard_text() -> Option<String> {
    imp::read_clipboard_text_win32()
}

#[cfg(windows)]
pub fn write_clipboard_text(text: &str) {
    imp::write_clipboard_text_win32(text);
}

#[cfg(windows)]
mod imp {
    use super::{encode_rgb_to_jpeg_data_url, tracker};
    use image::RgbImage;
    use std::mem::size_of;
    use std::time::{Duration, Instant};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
        GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    const CF_DIB_FMT: u32 = 8;
    const CF_UNICODETEXT_FMT: u32 = 13;
    const WM_CLIPBOARDUPDATE_MSG: u32 = 0x031D;

    pub unsafe fn run_clipboard_listener_loop() {
        let class_name: Vec<u16> = "BTWFloatClipboardWatcher\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(clip_wnd_proc),
            hInstance: HINSTANCE::default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            HINSTANCE::default(),
            None,
        ) else {
            return;
        };

        let _ = AddClipboardFormatListener(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe extern "system" fn clip_wnd_proc(
        hwnd: HWND,
        msg: u32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        if msg == WM_CLIPBOARDUPDATE_MSG {
            let now = Instant::now();
            let mut st = tracker().lock();
            if let Some(until) = st.suppress_until {
                if now < until {
                    return LRESULT(0);
                }
            }
            if IsClipboardFormatAvailable(CF_DIB_FMT).is_ok() {
                let seq = GetClipboardSequenceNumber();
                if seq != 0 && seq != st.last_image_seq {
                    st.last_image_seq = seq;
                    st.last_image_at = Some(now);
                }
            }
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, w_param, l_param)
    }

    fn open_clipboard_retry() -> bool {
        for _ in 0..4 {
            if unsafe { OpenClipboard(HWND::default()).is_ok() } {
                return true;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        false
    }

    pub fn take_fresh_clipboard_image_win32(max_age_secs: u64) -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_DIB_FMT).is_err() {
                return None;
            }
            let seq = GetClipboardSequenceNumber();
            {
                let st = tracker().lock();
                let added_at = st.last_image_at?;
                if added_at.elapsed() > Duration::from_secs(max_age_secs) {
                    return None;
                }
                if st.consumed_seq == Some(seq) {
                    return None;
                }
            }

            if !open_clipboard_retry() {
                return None;
            }

            let rgb_opt = read_dib_from_open_clipboard();
            let _ = CloseClipboard();

            let rgb = rgb_opt?;
            let data_url = encode_rgb_to_jpeg_data_url(rgb, 1920, 85)?;
            tracker().lock().consumed_seq = Some(seq);
            Some(data_url)
        }
    }

    unsafe fn read_dib_from_open_clipboard() -> Option<RgbImage> {
        let handle: HANDLE = GetClipboardData(CF_DIB_FMT).ok()?;
        let hglobal = HGLOBAL(handle.0);
        let total_size = GlobalSize(hglobal);
        if total_size < size_of::<BITMAPINFOHEADER>() {
            return None;
        }
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return None;
        }

        let res = parse_dib_slice(std::slice::from_raw_parts(ptr, total_size));
        let _ = GlobalUnlock(hglobal);
        res
    }

    fn parse_dib_slice(bytes: &[u8]) -> Option<RgbImage> {
        if bytes.len() < size_of::<BITMAPINFOHEADER>() {
            return None;
        }
        let header: BITMAPINFOHEADER =
            unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const BITMAPINFOHEADER) };

        let width = header.biWidth;
        let height_raw = header.biHeight;
        let top_down = height_raw < 0;
        let height = height_raw.unsigned_abs() as usize;
        let w = width as usize;
        let bpp = header.biBitCount as usize;

        if width <= 0 || height == 0 || width > 12000 || height > 12000 {
            return None;
        }
        if bpp != 32 && bpp != 24 {
            return None;
        }

        // BI_BITFIELDS (3) with standard 40-byte BITMAPINFOHEADER appends 3 DWORD color masks (12 bytes)
        let extra_masks = if header.biCompression == 3 && header.biSize == 40 {
            12usize
        } else {
            0usize
        };
        let palette_bytes = (header.biClrUsed as usize) * 4;
        let pixel_offset = (header.biSize as usize) + extra_masks + palette_bytes;
        if pixel_offset >= bytes.len() {
            return None;
        }

        let stride = (w * bpp).div_ceil(32) * 4;
        let needed = stride.checked_mul(height)?;
        let pixel_data = bytes.get(pixel_offset..pixel_offset + needed)?;

        let mut rgb = Vec::with_capacity(w * height * 3);
        for y in 0..height {
            let src_y = if top_down { y } else { height - 1 - y };
            let row = &pixel_data[src_y * stride..(src_y + 1) * stride];
            if bpp == 32 {
                for x in 0..w {
                    let b = row[x * 4];
                    let g = row[x * 4 + 1];
                    let r = row[x * 4 + 2];
                    rgb.push(r);
                    rgb.push(g);
                    rgb.push(b);
                }
            } else {
                for x in 0..w {
                    let b = row[x * 3];
                    let g = row[x * 3 + 1];
                    let r = row[x * 3 + 2];
                    rgb.push(r);
                    rgb.push(g);
                    rgb.push(b);
                }
            }
        }

        RgbImage::from_raw(w as u32, height as u32, rgb)
    }

    pub fn read_clipboard_text_win32() -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT_FMT).is_err() {
                return None;
            }
            if !open_clipboard_retry() {
                return None;
            }
            let mut out = None;
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT_FMT) {
                let hglobal = HGLOBAL(handle.0);
                let byte_len = GlobalSize(hglobal);
                let ptr = GlobalLock(hglobal) as *const u16;
                if !ptr.is_null() && byte_len >= 2 {
                    let max_u16 = byte_len / 2;
                    let slice = std::slice::from_raw_parts(ptr, max_u16);
                    let nul = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
                    out = Some(String::from_utf16_lossy(&slice[..nul]));
                    let _ = GlobalUnlock(hglobal);
                }
            }
            let _ = CloseClipboard();
            out
        }
    }

    pub fn write_clipboard_text_win32(text: &str) {
        unsafe {
            if !open_clipboard_retry() {
                return;
            }
            let _ = EmptyClipboard();
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let byte_len = wide.len() * 2;
            if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, byte_len) {
                let dst = GlobalLock(hmem) as *mut u16;
                if !dst.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                    let _ = GlobalUnlock(hmem);
                    let _ = SetClipboardData(CF_UNICODETEXT_FMT, HANDLE(hmem.0));
                }
            }
            let _ = CloseClipboard();
        }
    }
}
