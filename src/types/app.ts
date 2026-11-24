export interface ModelConfig {
  id: string;
  label: string;
  vision: boolean;
  toolCalling: boolean;
}

export interface FetchModelsResponse {
  normalizedBaseUrl: string;
  models: ModelConfig[];
}

export interface ProviderConfig {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  models: ModelConfig[];
}

export interface HotkeyConfig {
  togglePopup: string;
  mouseToggle: string;
  captureRegion: string;
  captureWindow: string;
}

export interface MousePriorityConfig {
  passthroughFullscreen: boolean;
  blacklistProcesses: string[];
  disabledProcesses: string[];
}

export interface UiConfig {
  showBall: boolean;
  ballX: number | null;
  ballY: number | null;
  ballAutoHideEdge: boolean;
  popupAutoHideOnBlur: boolean;
  clipboardImageFreshnessSecs: number;
  autoSubmitSelection: boolean;
  autostart: boolean;
  defaultWebSearch: boolean;
  showSearchSources: boolean;
  quickPrompts: string[];
}

export interface AppConfig {
  providers: ProviderConfig[];
  activeProviderId: string;
  activeModelId: string;
  hotkeys: HotkeyConfig;
  mousePriority: MousePriorityConfig;
  ui: UiConfig;
}

export interface PopupPrefillPayload {
  text?: string | null;
  image?: string | null;
  source?: string | null;
  autoSubmit?: boolean;
}

export interface HotkeyBindResult {
  action: string;
  binding: string;
  ok: boolean;
  error: string | null;
}

export interface SearchSource {
  title: string;
  url: string;
  snippet: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  images?: string[];
  sources?: SearchSource[];
}

export interface ChatStreamRequest {
  requestId: string;
  messages: Array<{
    role: string;
    content: string;
    images: string[];
  }>;
  providerId?: string;
  modelId?: string;
  webSearch: boolean;
}
