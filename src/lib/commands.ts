import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  ChatStreamRequest,
  FetchModelsResponse,
  HotkeyBindResult,
  ModelConfig,
  PopupPrefillPayload,
  WorkBuddyCheckinReceipt,
  WorkBuddyLoginPoll,
  WorkBuddyLoginStart,
  WorkBuddyStatus,
} from "../types/app";

export const getConfig = () => invoke<AppConfig>("get_config");

export const saveAppConfig = (newConfig: AppConfig) =>
  invoke<HotkeyBindResult[]>("save_app_config", { newConfig });

export const setActiveModel = (providerId: string, modelId: string) =>
  invoke<void>("set_active_model", { providerId, modelId });

export const setDefaultWebSearch = (enabled: boolean) =>
  invoke<void>("set_default_web_search", { enabled });

export const togglePopupFromBall = () => invoke<void>("toggle_popup_from_ball");

export const hidePopup = () => invoke<void>("hide_popup");

export const resizePopup = (height: number) =>
  invoke<void>("resize_popup", { height });

export const setPopupPin = (pinned: boolean) =>
  invoke<void>("set_popup_pin", { pinned });

export const ballDragFinished = () => invoke<void>("ball_drag_finished");

export const getBallEdgeState = () =>
  invoke<{ edge: "none" | "left" | "right"; cssSlide: boolean }>(
    "get_ball_edge_state"
  );

export const showBallMenu = () => invoke<void>("show_ball_menu");

export const openSettings = () => invoke<void>("open_settings");

export const setMouseRecording = (recording: boolean) =>
  invoke<void>("set_mouse_recording", { recording });

export const listRunningProcesses = () =>
  invoke<string[]>("list_running_processes");

export const takePendingPrefill = () =>
  invoke<PopupPrefillPayload | null>("take_pending_prefill");

export const startRegionCapture = () => invoke<void>("start_region_capture_cmd");

export const captureActiveWindow = () =>
  invoke<void>("capture_active_window_cmd");

export const getFrozenCapturePreview = () =>
  invoke<string | null>("get_frozen_capture_preview");

export const finishRegionCapture = (
  xRatio: number,
  yRatio: number,
  wRatio: number,
  hRatio: number
) =>
  invoke<void>("finish_region_capture", {
    xRatio,
    yRatio,
    wRatio,
    hRatio,
  });

export const cancelRegionCapture = () => invoke<void>("cancel_region_capture");

export const fetchProviderModels = (baseUrl: string, apiKey: string) =>
  invoke<FetchModelsResponse>("fetch_provider_models", { baseUrl, apiKey });

export const startChatStream = (request: ChatStreamRequest) =>
  invoke<void>("start_chat_stream", { request });

export const abortChatStream = (requestId: string) =>
  invoke<void>("abort_chat_stream", { requestId });

export const wbGetStatus = () => invoke<WorkBuddyStatus>("wb_get_status");

export const wbApplySettings = (enabled: boolean, port: number, apiKey: string) =>
  invoke<WorkBuddyStatus>("wb_apply_settings", { enabled, port, apiKey });

export const wbLoginStart = (site: string) =>
  invoke<WorkBuddyLoginStart>("wb_login_start", { site });

export const wbLoginPoll = (loginState: string) =>
  invoke<WorkBuddyLoginPoll>("wb_login_poll", { loginState });

export const wbLoginCancel = (loginState: string) =>
  invoke<void>("wb_login_cancel", { loginState });

export const wbLogout = () => invoke<WorkBuddyStatus>("wb_logout");

export const wbRefreshToken = () => invoke<WorkBuddyStatus>("wb_refresh_token");

export const wbRefreshStatus = () => invoke<WorkBuddyStatus>("wb_refresh_status");

export const wbSyncModels = (setAsDefault: boolean) =>
  invoke<ModelConfig[]>("wb_sync_models", { setAsDefault });

export const wbSetAutoCheckin = (enabled: boolean) =>
  invoke<WorkBuddyStatus>("wb_set_auto_checkin", { enabled });

export const wbCheckinNow = () =>
  invoke<WorkBuddyCheckinReceipt>("wb_checkin_now");
