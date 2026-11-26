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

export interface WorkBuddyAccountView {
  uid: string;
  nickname: string;
  realm: string;
  site: string;
  domain: string;
  expiresAt: number;
  status: "ready" | "no_credit" | "rate_limited" | "session_dead" | string;
  lastError: string | null;
  checkedInToday: boolean;
  lastCheckinDate: string;
  lastCheckinMsg: string | null;
  nextCheckinHint: string | null;
  creditsRemain: number | null;
  creditsTotal: number | null;
  creditsUnlimited: boolean;
}

export interface WorkBuddyStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  baseUrl: string;
  hasApiKey: boolean;
  autoCheckin: boolean;
  lastError: string | null;
  account: WorkBuddyAccountView | null;
  cachedModelCount: number;
}

export interface WorkBuddyCheckinReceipt {
  uid: string;
  state: "checked" | "already" | "pending" | "skipped" | "failed" | string;
  credit: number | null;
  streakDays: number | null;
  message: string;
}

export interface WorkBuddyLoginStart {
  state: string;
  authUrl: string;
  site: string;
  realm: string;
}

export interface WorkBuddyLoginPoll {
  status: "pending" | "done" | "fatal" | "expired" | string;
  message: string | null;
  uid: string | null;
  nickname: string | null;
  realm: string | null;
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
