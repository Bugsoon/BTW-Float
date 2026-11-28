use crate::window_mgr::{self, AppState};
use parking_lot::Mutex;
use std::sync::OnceLock;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

const TRAY_ICON_LIGHT_PNG: &[u8] = include_bytes!("../icons/tray-light.png");

pub const MENU_TOGGLE_POPUP: &str = "tray_toggle_popup";
pub const MENU_CAPTURE_REGION: &str = "tray_capture_region";
pub const MENU_CAPTURE_WINDOW: &str = "tray_capture_window";
pub const MENU_TOGGLE_BALL: &str = "tray_toggle_ball";
pub const MENU_TOGGLE_AUTOSTART: &str = "tray_toggle_autostart";
pub const MENU_OPEN_SETTINGS: &str = "tray_open_settings";
pub const MENU_QUIT: &str = "tray_quit";

#[derive(Clone)]
struct TrayMenuHandles {
    show_ball: CheckMenuItem<tauri::Wry>,
    autostart: CheckMenuItem<tauri::Wry>,
}

static TRAY_HANDLES: OnceLock<Mutex<Option<TrayMenuHandles>>> = OnceLock::new();

fn tray_handles() -> &'static Mutex<Option<TrayMenuHandles>> {
    TRAY_HANDLES.get_or_init(|| Mutex::new(None))
}

fn build_context_menu_with_handles(
    app: &AppHandle,
) -> tauri::Result<(Menu<tauri::Wry>, TrayMenuHandles)> {
    let (show_ball_checked, autostart_checked) = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock();
        (cfg.ui.show_ball, cfg.ui.autostart)
    };

    let toggle_popup = MenuItemBuilder::with_id(MENU_TOGGLE_POPUP, "呼出 / 收起胶囊问答").build(app)?;
    let capture_region = MenuItemBuilder::with_id(MENU_CAPTURE_REGION, "框选截屏提问").build(app)?;
    let capture_window =
        MenuItemBuilder::with_id(MENU_CAPTURE_WINDOW, "截取当前窗口提问").build(app)?;
    let toggle_ball = CheckMenuItemBuilder::with_id(MENU_TOGGLE_BALL, "显示桌面悬浮球")
        .checked(show_ball_checked)
        .build(app)?;
    let toggle_autostart =
        CheckMenuItemBuilder::with_id(MENU_TOGGLE_AUTOSTART, "开机自动启动")
            .checked(autostart_checked)
            .build(app)?;
    let open_settings = MenuItemBuilder::with_id(MENU_OPEN_SETTINGS, "设置...").build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "退出 BTW-Float").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&toggle_popup)
        .item(&capture_region)
        .item(&capture_window)
        .separator()
        .item(&toggle_ball)
        .item(&toggle_autostart)
        .item(&open_settings)
        .separator()
        .item(&quit)
        .build()?;

    Ok((
        menu,
        TrayMenuHandles {
            show_ball: toggle_ball,
            autostart: toggle_autostart,
        },
    ))
}

pub fn build_context_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    build_context_menu_with_handles(app).map(|(menu, _)| menu)
}

pub fn sync_tray_state(app: &AppHandle) {
    let (show_ball_checked, autostart_checked) = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock();
        (cfg.ui.show_ball, cfg.ui.autostart)
    };
    let Some(handles) = tray_handles().lock().clone() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _ = handles.show_ball.set_checked(show_ball_checked);
        let _ = handles.autostart.set_checked(autostart_checked);
    });
}

pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        MENU_TOGGLE_POPUP => window_mgr::toggle_popup(app, false),
        MENU_CAPTURE_REGION => window_mgr::start_region_capture(app),
        MENU_CAPTURE_WINDOW => window_mgr::capture_active_window_and_ask(app),
        MENU_TOGGLE_BALL => {
            let _ = window_mgr::toggle_ball_visibility(app);
        }
        MENU_TOGGLE_AUTOSTART => {
            let _ = window_mgr::toggle_autostart(app);
        }
        MENU_OPEN_SETTINGS => {
            window_mgr::open_settings_window_async(app);
        }
        MENU_QUIT => {
            window_mgr::request_app_exit(app);
        }
        _ => {}
    }
}

pub fn build_tray_ball_icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(TRAY_ICON_LIGHT_PNG)
}

pub fn init_tray(app: &AppHandle) -> tauri::Result<()> {
    let (menu, handles) = build_context_menu_with_handles(app)?;
    *tray_handles().lock() = Some(handles);

    let mut builder = TrayIconBuilder::with_id("btw-tray")
        .tooltip("BTW-Float")
        .menu(&menu)
        .show_menu_on_left_click(true);

    if let Some(icon) = build_tray_ball_icon()
        .ok()
        .or_else(|| app.default_window_icon().cloned())
    {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

pub fn show_ball_context_menu(app: &AppHandle) -> Result<(), String> {
    let Some(ball) = app.get_webview_window("ball") else {
        return Ok(());
    };
    let menu = build_context_menu(app).map_err(|e| e.to_string())?;
    ball.popup_menu(&menu).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_tray_ball_icon;

    #[test]
    fn tray_ball_icon_decodes_with_transparent_corners_and_opaque_orb() {
        let icon = build_tray_ball_icon().expect("valid tray ball PNG");
        assert_eq!(icon.width(), 64);
        assert_eq!(icon.height(), 64);
        let rgba = icon.rgba();
        assert_eq!(rgba.len(), 64 * 64 * 4);
        assert_eq!(rgba[3], 0, "top-left corner should be transparent");
        let center_idx = (32 * 64 + 32) * 4;
        assert!(
            rgba[center_idx + 3] > 200,
            "center of orb should be opaque"
        );
    }

    #[test]
    fn tray_ball_icon_is_not_classified_as_monochrome_glyph() {
        let icon = build_tray_ball_icon().expect("valid tray ball PNG");
        let mut total: u64 = 0;
        let mut white: u64 = 0;
        let mut black: u64 = 0;
        let mut luma_sum: u64 = 0;

        for px in icon.rgba().chunks_exact(4) {
            let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
            if a < 64 {
                continue;
            }
            total += 1;
            let chroma = r.max(g).max(b) - r.min(g).min(b);
            let luma = (54 * r as u32 + 183 * g as u32 + 19 * b as u32) >> 8;
            luma_sum += luma as u64;
            if chroma < 40 {
                if luma > 200 {
                    white += 1;
                } else if luma < 60 {
                    black += 1;
                }
            }
        }

        assert!(total > 0, "icon should contain opaque pixels");
        assert!(
            white * 100 < total * 45,
            "white monochrome pixel ratio too high ({white}/{total}), risks monochrome recoloring"
        );
        assert!(
            black * 100 < total * 10,
            "dark pixel ratio too high ({black}/{total}) for light tray icon"
        );
        assert!(
            luma_sum / total > 195,
            "average luma too low ({}) for light frosted orb",
            luma_sum / total
        );
    }
}
