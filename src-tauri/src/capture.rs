use crate::clipboard_watch::encode_rgb_to_jpeg_data_url;
use image::{GenericImageView, RgbImage};
use parking_lot::Mutex;
use std::sync::OnceLock;

pub struct FrozenMonitorCapture {
    pub raw_rgb: RgbImage,
    pub preview_data_url: String,
}

static FROZEN_CAPTURE: OnceLock<Mutex<Option<FrozenMonitorCapture>>> = OnceLock::new();

fn frozen_slot() -> &'static Mutex<Option<FrozenMonitorCapture>> {
    FROZEN_CAPTURE.get_or_init(|| Mutex::new(None))
}

pub fn get_frozen_preview() -> Option<String> {
    frozen_slot()
        .lock()
        .as_ref()
        .map(|f| f.preview_data_url.clone())
}

pub fn clear_frozen_capture() {
    *frozen_slot().lock() = None;
}

/// Crops the stored frozen monitor image using normalized `[0.0 .. 1.0]` ratios,
/// completely immune to Windows 125% / 150% / 200% DPI scaling mismatches.
pub fn crop_frozen_by_ratio(
    x_ratio: f64,
    y_ratio: f64,
    w_ratio: f64,
    h_ratio: f64,
) -> Option<String> {
    let frozen = frozen_slot().lock().take()?;
    let (img_w, img_h) = frozen.raw_rgb.dimensions();

    let rx = x_ratio.clamp(0.0, 1.0);
    let ry = y_ratio.clamp(0.0, 1.0);
    let rw = w_ratio.clamp(0.0, 1.0 - rx);
    let rh = h_ratio.clamp(0.0, 1.0 - ry);

    let px = ((rx * img_w as f64).round() as u32).min(img_w.saturating_sub(1));
    let py = ((ry * img_h as f64).round() as u32).min(img_h.saturating_sub(1));
    let pw = ((rw * img_w as f64).round() as u32)
        .max(4)
        .min(img_w - px);
    let ph = ((rh * img_h as f64).round() as u32)
        .max(4)
        .min(img_h - py);

    let cropped = frozen.raw_rgb.view(px, py, pw, ph).to_image();
    encode_rgb_to_jpeg_data_url(cropped, 1920, 88)
}

/// Captures the full monitor containing `(cursor_x, cursor_y)` into `FROZEN_CAPTURE`
/// and returns `(monitor_x, monitor_y, monitor_w, monitor_h)` in physical pixels.
pub fn freeze_monitor_at_point(cursor_x: i32, cursor_y: i32) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        let (mx, my, mw, mh) = imp::get_monitor_full_rect(cursor_x, cursor_y)?;
        let raw = imp::bitblt_screen_rect(mx, my, mw, mh)?;
        let preview = encode_rgb_to_jpeg_data_url(raw.clone(), 2560, 88)?;
        *frozen_slot().lock() = Some(FrozenMonitorCapture {
            raw_rgb: raw,
            preview_data_url: preview,
        });
        Some((mx, my, mw, mh))
    }
    #[cfg(not(windows))]
    {
        let _ = (cursor_x, cursor_y);
        None
    }
}

/// Captures the foreground active window cleanly using DWM extended frame bounds.
pub fn capture_foreground_window_jpeg() -> Option<String> {
    #[cfg(windows)]
    {
        let (x, y, w, h) = imp::get_foreground_window_bounds()?;
        let rgb = imp::bitblt_screen_rect(x, y, w, h)?;
        encode_rgb_to_jpeg_data_url(rgb, 1920, 86)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Loads a dropped local image file and encodes it as a resized JPEG data URL.
pub fn load_dropped_image_file(path: &std::path::Path) -> Option<String> {
    let img = image::open(path).ok()?;
    encode_rgb_to_jpeg_data_url(img.to_rgb8(), 1920, 86)
}

#[cfg(windows)]
pub fn wait_dwm_flush() {
    unsafe {
        let _ = windows::Win32::Graphics::Dwm::DwmFlush();
    }
}

#[cfg(not(windows))]
pub fn wait_dwm_flush() {}

#[cfg(test)]
mod tests {
    use super::{crop_frozen_by_ratio, frozen_slot, FrozenMonitorCapture};
    use image::{Rgb, RgbImage};

    #[test]
    fn crops_normalized_ratios_accurately() {
        let mut img = RgbImage::new(200, 100);
        for y in 0..100 {
            for x in 0..200 {
                img.put_pixel(x, y, Rgb([(x % 255) as u8, (y % 255) as u8, 128]));
            }
        }
        *frozen_slot().lock() = Some(FrozenMonitorCapture {
            raw_rgb: img,
            preview_data_url: String::new(),
        });

        let data_url = crop_frozen_by_ratio(0.25, 0.2, 0.5, 0.5).expect("should encode cropped jpeg");
        assert!(data_url.starts_with("data:image/jpeg;base64,"));
        assert!(frozen_slot().lock().is_none());
    }
}

#[cfg(windows)]
mod imp {
    use image::RgbImage;
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    pub fn get_monitor_full_rect(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(hmon, &mut info).as_bool() {
                return None;
            }
            let r = info.rcMonitor;
            let w = r.right - r.left;
            let h = r.bottom - r.top;
            if w <= 0 || h <= 0 {
                return None;
            }
            Some((r.left, r.top, w, h))
        }
    }

    pub fn get_foreground_window_bounds() -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut rect = RECT::default();
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut rect as *mut RECT).cast::<c_void>(),
                size_of::<RECT>() as u32,
            );
            if hr.is_err() && GetWindowRect(hwnd, &mut rect).is_err() {
                return None;
            }
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w <= 16 || h <= 16 {
                return None;
            }
            Some((rect.left, rect.top, w, h))
        }
    }

    pub fn bitblt_screen_rect(x: i32, y: i32, width: i32, height: i32) -> Option<RgbImage> {
        if width <= 0 || height <= 0 {
            return None;
        }
        unsafe {
            let screen_dc = GetDC(HWND(std::ptr::null_mut()));
            if screen_dc.is_invalid() {
                return None;
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_invalid() {
                let _ = ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
                return None;
            }
            let bmp = CreateCompatibleBitmap(screen_dc, width, height);
            if bmp.is_invalid() {
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
                return None;
            }

            let old = SelectObject(mem_dc, bmp);
            let ok = BitBlt(mem_dc, 0, 0, width, height, screen_dc, x, y, SRCCOPY).is_ok();

            let mut out = None;
            if ok {
                let mut bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height, // top-down
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bgra = vec![0u8; (width as usize) * (height as usize) * 4];
                let lines = GetDIBits(
                    mem_dc,
                    bmp,
                    0,
                    height as u32,
                    Some(bgra.as_mut_ptr().cast()),
                    &mut bmi,
                    DIB_RGB_COLORS,
                );
                if lines > 0 {
                    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
                    for px in bgra.as_chunks::<4>().0 {
                        rgb.push(px[2]);
                        rgb.push(px[1]);
                        rgb.push(px[0]);
                    }
                    out = RgbImage::from_raw(width as u32, height as u32, rgb);
                }
            }

            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            out
        }
    }
}
