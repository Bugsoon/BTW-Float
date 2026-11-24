use crate::window_mgr;
use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

pub const MENU_TOGGLE_POPUP: &str = "tray_toggle_popup";
pub const MENU_CAPTURE_REGION: &str = "tray_capture_region";
pub const MENU_CAPTURE_WINDOW: &str = "tray_capture_window";
pub const MENU_TOGGLE_BALL: &str = "tray_toggle_ball";
pub const MENU_OPEN_SETTINGS: &str = "tray_open_settings";
pub const MENU_QUIT: &str = "tray_quit";

pub fn build_context_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let toggle_popup = MenuItemBuilder::with_id(MENU_TOGGLE_POPUP, "呼出 / 收起胶囊问答").build(app)?;
    let capture_region = MenuItemBuilder::with_id(MENU_CAPTURE_REGION, "框选截屏提问").build(app)?;
    let capture_window =
        MenuItemBuilder::with_id(MENU_CAPTURE_WINDOW, "截取当前窗口提问").build(app)?;
    let toggle_ball = MenuItemBuilder::with_id(MENU_TOGGLE_BALL, "显示 / 隐藏悬浮球").build(app)?;
    let open_settings = MenuItemBuilder::with_id(MENU_OPEN_SETTINGS, "设置...").build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "退出 BTW-Float").build(app)?;

    MenuBuilder::new(app)
        .item(&toggle_popup)
        .item(&capture_region)
        .item(&capture_window)
        .separator()
        .item(&toggle_ball)
        .item(&open_settings)
        .separator()
        .item(&quit)
        .build()
}

pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        MENU_TOGGLE_POPUP => window_mgr::toggle_popup(app, false),
        MENU_CAPTURE_REGION => window_mgr::start_region_capture(app),
        MENU_CAPTURE_WINDOW => window_mgr::capture_active_window_and_ask(app),
        MENU_TOGGLE_BALL => {
            let _ = window_mgr::toggle_ball_visibility(app);
        }
        MENU_OPEN_SETTINGS => {
            window_mgr::open_settings_window_async(app);
        }
        MENU_QUIT => {
            app.exit(0);
        }
        _ => {}
    }
}

pub fn init_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_context_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("btw-tray")
        .tooltip("BTW-Float · 轻量桌面 AI 助手")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id().as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window_mgr::toggle_popup(tray.app_handle(), false);
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
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
