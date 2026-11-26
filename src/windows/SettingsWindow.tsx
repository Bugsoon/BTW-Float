import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Check,
  Eye,
  EyeOff,
  Keyboard,
  MousePointerClick,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Server,
  ShieldCheck,
  Sliders,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { WorkBuddyPanel } from "../components/WorkBuddyPanel";
import {
  fetchProviderModels,
  getConfig,
  listRunningProcesses,
  saveAppConfig,
  setActiveModel,
  setMouseRecording,
} from "../lib/commands";
import type {
  AppConfig,
  HotkeyBindResult,
  ModelConfig,
  ProviderConfig,
} from "../types/app";

type TabId = "models" | "workbuddy" | "hotkeys" | "ui";
type HotkeyField =
  | "togglePopup"
  | "mouseToggle"
  | "captureRegion"
  | "captureWindow";

const PROVIDER_PRESETS = [
  {
    name: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    modelId: "deepseek-chat",
    modelLabel: "DeepSeek Chat",
    vision: false,
  },
  {
    name: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    modelId: "google/gemini-2.5-flash",
    modelLabel: "Gemini 2.5 Flash",
    vision: true,
  },
  {
    name: "硅基流动",
    baseUrl: "https://api.siliconflow.cn/v1",
    modelId: "Qwen/Qwen2.5-72B-Instruct",
    modelLabel: "Qwen 2.5 72B",
    vision: false,
  },
  {
    name: "本地 Ollama",
    baseUrl: "http://127.0.0.1:11434/v1",
    modelId: "qwen2.5:7b",
    modelLabel: "Ollama Qwen2.5",
    vision: false,
  },
];

function formatKeyboardEvent(e: KeyboardEvent): string | null {
  const ignore = new Set(["Control", "Alt", "Shift", "Meta"]);
  if (ignore.has(e.key)) return null;

  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");

  let key = e.key;
  if (key === " ") key = "Space";
  else if (key.length === 1) key = key.toUpperCase();
  else if (key.startsWith("Arrow")) key = key.replace("Arrow", "");

  if (mods.length === 0 && !/^F\d+$/.test(key)) {
    return null; // Require modifier for regular keys
  }
  return [...mods, key].join("+");
}

const MOUSE_SIDE_MODES = [
  {
    value: "XButton1/XButton2",
    title: "前、后侧键都能用",
    badge: "推荐",
    desc: "按任意侧键呼出；逛网页未选字时自动保留「后退」功能",
  },
  {
    value: "XButton2",
    title: "仅前侧键（靠指尖）",
    badge: null,
    desc: "只用靠指尖那颗侧键呼出，后侧键完全留给其他软件",
  },
  {
    value: "XButton1",
    title: "仅后侧键（靠掌心）",
    badge: null,
    desc: "只用靠掌心那颗侧键呼出",
  },
  {
    value: "",
    title: "不使用鼠标侧键",
    badge: null,
    desc: "关闭侧键响应，仅用键盘快捷键或桌面悬浮球",
  },
] as const;

const KEYBOARD_SHORTCUT_ITEMS = [
  {
    field: "togglePopup" as const,
    title: "打开 / 收起 AI 输入框",
    desc: "随时唤醒或隐藏胶囊问答框",
    defaultValue: "Alt+Space",
  },
  {
    field: "captureRegion" as const,
    title: "框选屏幕区域提问",
    desc: "拖动框选屏幕上任意一块画面发给 AI",
    defaultValue: "Alt+A",
  },
  {
    field: "captureWindow" as const,
    title: "截取当前窗口提问",
    desc: "一键截取当前正在看的软件窗口发给 AI",
    defaultValue: "Alt+S",
  },
] as const;

function formatMouseComboLabel(raw: string): string {
  if (!raw) return "未启用";
  if (raw.toLowerCase() === "xbutton1/xbutton2") {
    return "前、后侧键都能用";
  }
  return raw
    .split("+")
    .map((part) => {
      const lower = part.trim().toLowerCase();
      if (lower === "xbutton2") return "前侧键";
      if (lower === "xbutton1") return "后侧键";
      if (lower === "super") return "Win";
      return part.trim();
    })
    .join(" + ");
}

function fuzzyMatchScore(query: string, target: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 1;
  const t = target.toLowerCase();
  if (t === q) return 4;
  if (t.startsWith(q)) return 3;
  if (t.includes(q)) return 2;
  let qi = 0;
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) qi++;
  }
  return qi === q.length ? 1 : 0;
}

function ProcessPickerSection({
  title,
  desc,
  accent = "sky",
  processes,
  runningProcs,
  onRefreshRunning,
  onAdd,
  onRemove,
}: {
  title: string;
  desc: string;
  accent?: "sky" | "rose";
  processes: string[];
  runningProcs: string[];
  onRefreshRunning: () => Promise<void>;
  onAdd: (proc: string) => void;
  onRemove: (proc: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [refreshing, setRefreshing] = useState(false);

  const handleToggle = async () => {
    if (!open) {
      setRefreshing(true);
      await onRefreshRunning();
      setRefreshing(false);
    }
    setOpen((v) => !v);
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    await onRefreshRunning();
    setRefreshing(false);
  };

  const normalizedSet = new Set(
    processes.map((p) => p.trim().toLowerCase().replace(/\.exe$/, ""))
  );

  const filteredProcs = runningProcs
    .map((proc) => ({ proc, score: fuzzyMatchScore(query, proc) }))
    .filter((item) => item.score > 0)
    .sort((a, b) => b.score - a.score || a.proc.localeCompare(b.proc));

  const submitInput = () => {
    const clean = query.trim();
    if (!clean) return;
    onAdd(clean);
    setQuery("");
  };

  const chipStyle =
    accent === "rose"
      ? "border-rose-400/25 bg-rose-500/12 text-rose-100"
      : "border-white/12 bg-white/8 text-slate-200";

  return (
    <div className="space-y-2.5 rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="text-[13px] text-white">{title}</div>
          <div className="text-[11px] text-slate-400">{desc}</div>
        </div>
        <button
          type="button"
          onClick={() => void handleToggle()}
          className="flex items-center gap-1 rounded-lg border border-white/14 bg-white/8 px-2.5 py-1 text-xs text-sky-200 transition hover:bg-white/14"
        >
          <Plus className="h-3.5 w-3.5" />
          <span>{open ? "收起选择" : "添加软件"}</span>
        </button>
      </div>

      {processes.length > 0 && (
        <div className="flex flex-wrap gap-1.5 pt-0.5">
          {processes.map((proc) => (
            <span
              key={proc}
              className={`inline-flex items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs ${chipStyle}`}
            >
              {proc}
              <button
                type="button"
                onClick={() => onRemove(proc)}
                className="text-slate-400 hover:text-rose-300"
                title="移除"
              >
                <X className="h-3 w-3" />
              </button>
            </span>
          ))}
        </div>
      )}

      {open && (
        <div className="space-y-2 rounded-xl border border-sky-400/25 bg-black/35 p-2.5">
          <div className="flex gap-2">
            <div className="relative flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-400" />
              <input
                type="text"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    submitInput();
                  }
                }}
                placeholder="输入关键词模糊匹配当前进程，或直接输入程序名回车添加"
                className="w-full rounded-lg border border-white/12 bg-black/30 py-1.5 pl-8 pr-2.5 text-xs outline-none focus:border-sky-400/60 select-text"
              />
            </div>
            <button
              type="button"
              onClick={submitInput}
              className="rounded-lg bg-white/12 px-3 py-1.5 text-xs hover:bg-white/20"
            >
              添加
            </button>
            <button
              type="button"
              onClick={() => void handleRefresh()}
              disabled={refreshing}
              title="刷新当前运行的进程列表"
              className="rounded-lg border border-white/10 bg-white/6 px-2 py-1.5 text-slate-300 transition hover:bg-white/14 disabled:opacity-50"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${refreshing ? "animate-spin" : ""}`}
              />
            </button>
          </div>

          <div>
            <div className="mb-1.5 flex items-center justify-between text-[11px] text-slate-400">
              <span>
                {query.trim()
                  ? `模糊匹配到 ${filteredProcs.length} 个运行中进程（点击直接添加）：`
                  : `点击当前正在运行的软件（共 ${runningProcs.length} 个）：`}
              </span>
            </div>
            {filteredProcs.length > 0 ? (
              <div className="glass-scroll flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
                {filteredProcs.map(({ proc }) => {
                  const key = proc.toLowerCase().replace(/\.exe$/, "");
                  const already = normalizedSet.has(key);
                  return (
                    <button
                      key={proc}
                      type="button"
                      disabled={already}
                      onClick={() => {
                        onAdd(proc);
                        setQuery("");
                      }}
                      className={`rounded-md border px-2 py-0.5 text-[11px] transition ${
                        already
                          ? "border-sky-400/30 bg-sky-500/15 text-sky-200 opacity-60"
                          : "border-white/10 bg-white/6 text-slate-200 hover:border-sky-400/40 hover:bg-sky-500/20"
                      }`}
                    >
                      {already ? `✓ ${proc}` : `+ ${proc}`}
                    </button>
                  );
                })}
              </div>
            ) : (
              <div className="py-1 text-[11px] text-slate-500">
                未找到匹配的运行中进程，可直接按回车或点击「添加」手动录入程序名
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function ToggleSwitch({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors ${
        checked ? "bg-sky-500" : "bg-white/18 hover:bg-white/25"
      }`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 rounded-full bg-white shadow-sm transition-transform ${
          checked ? "translate-x-[18px]" : "translate-x-[3px]"
        }`}
      />
    </button>
  );
}

export function SettingsWindow() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [tab, setTab] = useState<TabId>("models");
  const [saving, setSaving] = useState(false);
  const [savedToast, setSavedToast] = useState<string | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const [bindResults, setBindResults] = useState<HotkeyBindResult[]>([]);
  const [showKeys, setShowKeys] = useState<Record<string, boolean>>({});

  // Hotkey recording state
  const [recordingField, setRecordingField] = useState<HotkeyField | null>(null);

  // App exception list UI
  const [runningProcs, setRunningProcs] = useState<string[]>([]);

  // Per-provider model auto-discovery state
  const [scanningPid, setScanningPid] = useState<string | null>(null);
  const [scannedModels, setScannedModels] = useState<
    Record<string, ModelConfig[]>
  >({});
  const [scanErrors, setScanErrors] = useState<Record<string, string>>({});
  const [modelSearchQuery, setModelSearchQuery] = useState<
    Record<string, string>
  >({});
  const [openPickerPid, setOpenPickerPid] = useState<string | null>(null);

  const showToast = (msg: string, duration = 2200) => {
    setSavedToast(msg);
    if (toastTimerRef.current) {
      window.clearTimeout(toastTimerRef.current);
    }
    toastTimerRef.current = window.setTimeout(() => {
      setSavedToast(null);
    }, duration);
  };

  const commitConfig = (next: AppConfig, toastMsg = "已自动生效") => {
    setConfig(next);
    void saveAppConfig(next).then((res) => {
      setBindResults(res);
      const hasErr = res.some((r) => !r.ok);
      showToast(hasErr ? "已保存，但存在快捷键冲突" : toastMsg);
    });
  };

  useEffect(() => {
    void getConfig().then(setConfig);
  }, []);

  // Separate mouse side-button detection vs keyboard shortcut recording
  useEffect(() => {
    const isMouseRec = recordingField === "mouseToggle";
    void setMouseRecording(isMouseRec);

    if (!recordingField) {
      return;
    }

    const applyCapturedBinding = (val: string, msg: string) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const next: AppConfig = {
          ...prev,
          hotkeys: { ...prev.hotkeys, [recordingField]: val },
        };
        void saveAppConfig(next).then((res) => {
          setBindResults(res);
          const hasErr = res.some((r) => !r.ok);
          showToast(hasErr ? "快捷键被其他软件占用，请换一个" : msg);
        });
        return next;
      });
      setRecordingField(null);
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setRecordingField(null);
        return;
      }
      if (isMouseRec) {
        // Allow holding Alt/Ctrl/Shift while clicking mouse side button, ignore plain key presses
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      const combo = formatKeyboardEvent(e);
      if (combo) {
        applyCapturedBinding(combo, "键盘快捷键已生效");
      }
    };

    window.addEventListener("keydown", onKeyDown, true);
    const unlistenPromise = isMouseRec
      ? listen<string>("hotkey:mouse-captured", (ev) => {
          applyCapturedBinding(
            ev.payload,
            `已切换为「${formatMouseComboLabel(ev.payload)}」`
          );
        })
      : null;

    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      void setMouseRecording(false);
      if (unlistenPromise) {
        void unlistenPromise.then((fn) => fn());
      }
    };
  }, [recordingField]);

  const handleSave = async () => {
    if (!config) return;
    setSaving(true);
    try {
      const res = await saveAppConfig(config);
      setBindResults(res);
      const hasErr = res.some((r) => !r.ok);
      showToast(hasErr ? "已保存，但部分快捷键冲突" : "配置已保存并生效", 2600);
    } catch (err) {
      showToast(`保存失败: ${String(err)}`, 3000);
    } finally {
      setSaving(false);
    }
  };

  const handleSelectActiveModel = async (
    providerId: string,
    modelId: string,
    modelLabel?: string
  ) => {
    if (!config || !modelId.trim()) return;
    const next: AppConfig = {
      ...config,
      activeProviderId: providerId,
      activeModelId: modelId,
    };
    setConfig(next);
    try {
      await setActiveModel(providerId, modelId);
      showToast(`已切换当前模型：${modelLabel || modelId}`);
    } catch (err) {
      showToast(`切换模型失败: ${String(err)}`, 2800);
    }
  };

  const addProvider = (preset?: (typeof PROVIDER_PRESETS)[number]) => {
    if (!config) return;
    const id = `provider-${Date.now()}`;
    const nextProvider: ProviderConfig = {
      id,
      name: preset?.name ?? "自定义供应商",
      baseUrl: preset?.baseUrl ?? "https://api.openai.com/v1",
      apiKey: "",
      models: [
        {
          id: preset?.modelId ?? "gpt-4o-mini",
          label: preset?.modelLabel ?? "新模型",
          vision: preset?.vision ?? true,
          toolCalling: true,
        },
      ],
    };
    setConfig({
      ...config,
      providers: [...config.providers, nextProvider],
      activeProviderId: config.activeProviderId || id,
      activeModelId: config.activeModelId || nextProvider.models[0].id,
    });
  };

  const updateProvider = (
    pid: string,
    patch: Partial<ProviderConfig>
  ) => {
    if (!config) return;
    setConfig({
      ...config,
      providers: config.providers.map((p) =>
        p.id === pid ? { ...p, ...patch } : p
      ),
    });
  };

  const deleteProvider = (pid: string) => {
    if (!config || config.providers.length <= 1) return;
    const remaining = config.providers.filter((p) => p.id !== pid);
    const activeRemoved = config.activeProviderId === pid;
    setConfig({
      ...config,
      providers: remaining,
      activeProviderId: activeRemoved ? remaining[0].id : config.activeProviderId,
      activeModelId: activeRemoved
        ? remaining[0].models[0]?.id ?? ""
        : config.activeModelId,
    });
  };

  const addModel = (pid: string) => {
    if (!config) return;
    setConfig({
      ...config,
      providers: config.providers.map((p) =>
        p.id === pid
          ? {
              ...p,
              models: [
                ...p.models,
                {
                  id: "",
                  label: "新模型",
                  vision: false,
                  toolCalling: true,
                },
              ],
            }
          : p
      ),
    });
  };

  const scanModelsForProvider = async (
    provider: ProviderConfig,
    silentIfIncomplete = false
  ) => {
    const base = provider.baseUrl.trim();
    const key = provider.apiKey.trim();
    const isLocal =
      base.includes("127.0.0.1") || base.includes("localhost");

    if (!base || (silentIfIncomplete && !key && !isLocal)) {
      return;
    }

    setScanningPid(provider.id);
    setScanErrors((prev) => ({ ...prev, [provider.id]: "" }));

    try {
      const resp = await fetchProviderModels(base, key);
      setScannedModels((prev) => ({ ...prev, [provider.id]: resp.models }));
      setOpenPickerPid(provider.id);

      setConfig((prev) => {
        if (!prev) return prev;
        return {
          ...prev,
          providers: prev.providers.map((p) => {
            if (p.id !== provider.id) return p;
            // Auto-populate directly when provider returns a small clean list (e.g. DeepSeek / Ollama)
            // or replace empty placeholder models
            const nonEmptyExisting = p.models.filter((m) => m.id.trim() !== "");
            let nextModels = nonEmptyExisting;
            if (resp.models.length <= 6 && nonEmptyExisting.length <= 1) {
              const merged = [...nonEmptyExisting];
              for (const dm of resp.models) {
                if (!merged.some((em) => em.id === dm.id)) {
                  merged.push(dm);
                }
              }
              nextModels = merged;
            } else if (nextModels.length === 0 && resp.models.length > 0) {
              nextModels = [resp.models[0]];
            }
            return {
              ...p,
              baseUrl: resp.normalizedBaseUrl || p.baseUrl,
              models: nextModels.length > 0 ? nextModels : p.models,
            };
          }),
        };
      });
    } catch (err) {
      if (!silentIfIncomplete) {
        setScanErrors((prev) => ({
          ...prev,
          [provider.id]: String(err),
        }));
      }
    } finally {
      setScanningPid((cur) => (cur === provider.id ? null : cur));
    }
  };

  const toggleScannedModel = (pid: string, dm: ModelConfig) => {
    if (!config) return;
    setConfig((prev) => {
      if (!prev) return prev;
      let nextActiveProvider = prev.activeProviderId;
      let nextActiveModel = prev.activeModelId;

      const nextProviders = prev.providers.map((p) => {
        if (p.id !== pid) return p;
        const exists = p.models.some((m) => m.id === dm.id);
        if (exists) {
          if (p.models.length <= 1) return p;
          const filtered = p.models.filter((m) => m.id !== dm.id);
          if (nextActiveProvider === pid && nextActiveModel === dm.id) {
            nextActiveModel = filtered[0]?.id ?? "";
          }
          return { ...p, models: filtered };
        } else {
          // Replace blank placeholder if present
          const cleaned = p.models.filter((m) => m.id.trim() !== "");
          if (!nextActiveModel) {
            nextActiveProvider = pid;
            nextActiveModel = dm.id;
          }
          return { ...p, models: [...cleaned, dm] };
        }
      });

      return {
        ...prev,
        providers: nextProviders,
        activeProviderId: nextActiveProvider,
        activeModelId: nextActiveModel,
      };
    });
  };

  const refreshRunningProcesses = async () => {
    const procs = await listRunningProcesses();
    setRunningProcs(procs);
  };

  const addBlacklistProcess = (procName: string) => {
    if (!config) return;
    const clean = procName.trim();
    if (!clean) return;
    const key = clean.toLowerCase().replace(/\.exe$/, "");
    const exists = config.mousePriority.blacklistProcesses.some(
      (p) => p.trim().toLowerCase().replace(/\.exe$/, "") === key
    );
    if (exists) return;
    commitConfig(
      {
        ...config,
        mousePriority: {
          ...config.mousePriority,
          blacklistProcesses: [
            ...config.mousePriority.blacklistProcesses,
            clean,
          ],
        },
      },
      `已在 ${clean} 中停用侧键`
    );
  };

  const removeBlacklistProcess = (procName: string) => {
    if (!config) return;
    commitConfig(
      {
        ...config,
        mousePriority: {
          ...config.mousePriority,
          blacklistProcesses: config.mousePriority.blacklistProcesses.filter(
            (p) => p !== procName
          ),
        },
      },
      "已移除侧键例外软件"
    );
  };

  const addDisabledProcess = (procName: string) => {
    if (!config) return;
    const clean = procName.trim();
    if (!clean) return;
    const key = clean.toLowerCase().replace(/\.exe$/, "");
    const exists = (config.mousePriority.disabledProcesses ?? []).some(
      (p) => p.trim().toLowerCase().replace(/\.exe$/, "") === key
    );
    if (exists) return;
    commitConfig(
      {
        ...config,
        mousePriority: {
          ...config.mousePriority,
          disabledProcesses: [
            ...(config.mousePriority.disabledProcesses ?? []),
            clean,
          ],
        },
      },
      `将在 ${clean} 中完全禁用 BTW-Float`
    );
  };

  const removeDisabledProcess = (procName: string) => {
    if (!config) return;
    commitConfig(
      {
        ...config,
        mousePriority: {
          ...config.mousePriority,
          disabledProcesses: (
            config.mousePriority.disabledProcesses ?? []
          ).filter((p) => p !== procName),
        },
      },
      "已移除完全禁用软件"
    );
  };

  if (!config) {
    return (
      <div className="flex h-full items-center justify-center bg-[#0f131c] text-sm text-slate-300">
        正在加载配置...
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col bg-[#0f131c] text-slate-100">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-white/10 px-6 py-4">
        <div className="flex items-center gap-2.5">
          <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-sky-400/20 text-sky-300">
            <Sparkles className="h-4 w-4" />
          </div>
          <div>
            <h1 className="text-[15px] font-semibold leading-tight">
              BTW-Float 设置
            </h1>
            <p className="text-[11.5px] text-slate-400">
              API Key 使用 Windows DPAPI 本地加密保存
            </p>
          </div>
        </div>

        <div className="flex items-center gap-3">
          {savedToast && (
            <span className="flex items-center gap-1 rounded-full bg-emerald-500/20 px-3 py-1 text-xs text-emerald-200">
              <Check className="h-3.5 w-3.5" />
              {savedToast}
            </span>
          )}
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={saving}
            className="flex items-center gap-1.5 rounded-xl bg-sky-500 px-4 py-2 text-xs font-medium text-white shadow-sm transition hover:bg-sky-400 disabled:opacity-50"
          >
            <Save className="h-3.5 w-3.5" />
            <span>{saving ? "保存中..." : "保存并生效"}</span>
          </button>
        </div>
      </div>

      {/* Main layout */}
      <div className="flex min-h-0 flex-1">
        {/* Sidebar tabs */}
        <nav className="w-44 shrink-0 space-y-1 border-r border-white/10 p-3">
          {[
            { id: "models" as const, label: "模型与 API", icon: Sparkles },
            { id: "workbuddy" as const, label: "WorkBuddy 内核", icon: Server },
            {
              id: "hotkeys" as const,
              label: "快捷键与侧键",
              icon: MousePointerClick,
            },
            { id: "ui" as const, label: "悬浮球与交互", icon: Sliders },
          ].map((item) => {
            const Icon = item.icon;
            const active = tab === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setTab(item.id)}
                className={`flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-left text-[13px] transition ${
                  active
                    ? "bg-white/14 font-medium text-white"
                    : "text-slate-300 hover:bg-white/8 hover:text-white"
                }`}
              >
                <Icon className="h-4 w-4" />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>

        {/* Content pane */}
        <div className="glass-scroll min-w-0 flex-1 overflow-y-auto p-6">
          {tab === "models" && (
            <div className="space-y-5">
              {/* Prominent Active Model Selector */}
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-sky-400/30 bg-sky-500/10 p-4">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm font-medium text-white">
                    <Sparkles className="h-4 w-4 text-sky-300" />
                    <span>当前对话使用的模型</span>
                  </div>
                  <p className="mt-0.5 text-[11.5px] text-slate-300/80">
                    悬浮球提问、划词解答和截屏提问默认使用这里选中的模型（点选即刻生效）
                  </p>
                </div>
                <select
                  value={`${config.activeProviderId}::${config.activeModelId}`}
                  onChange={(e) => {
                    const [pid, ...rest] = e.target.value.split("::");
                    const mid = rest.join("::");
                    const prov = config.providers.find((p) => p.id === pid);
                    const mod = prov?.models.find((m) => m.id === mid);
                    void handleSelectActiveModel(pid, mid, mod?.label || mid);
                  }}
                  className="min-w-[230px] max-w-full rounded-xl border border-sky-400/40 bg-[#0b101b] px-3 py-2 text-xs font-medium text-white outline-none focus:border-sky-300"
                >
                  {config.providers.map((p) => {
                    const validModels = p.models.filter((m) => m.id.trim() !== "");
                    if (validModels.length === 0) return null;
                    return (
                      <optgroup key={p.id} label={p.name || p.id}>
                        {validModels.map((m) => (
                          <option
                            key={`${p.id}::${m.id}`}
                            value={`${p.id}::${m.id}`}
                          >
                            {m.label && m.label !== m.id
                              ? `${m.label} (${m.id})`
                              : m.id}
                          </option>
                        ))}
                      </optgroup>
                    );
                  })}
                </select>
              </div>

              {/* Quick presets */}
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-xs text-slate-400">快速添加模板：</span>
                {PROVIDER_PRESETS.map((preset) => (
                  <button
                    key={preset.name}
                    type="button"
                    onClick={() => addProvider(preset)}
                    className="rounded-lg border border-white/12 bg-white/6 px-2.5 py-1 text-xs text-slate-200 transition hover:bg-white/14"
                  >
                    + {preset.name}
                  </button>
                ))}
                <button
                  type="button"
                  onClick={() => addProvider()}
                  className="rounded-lg border border-sky-400/30 bg-sky-500/15 px-2.5 py-1 text-xs text-sky-200 transition hover:bg-sky-500/25"
                >
                  + 自定义
                </button>
              </div>

              {config.providers.map((provider) => (
                <div
                  key={provider.id}
                  className="space-y-3 rounded-2xl border border-white/12 bg-white/5 p-4"
                >
                  <div className="flex items-center justify-between gap-3">
                    <input
                      type="text"
                      value={provider.name}
                      onChange={(e) =>
                        updateProvider(provider.id, { name: e.target.value })
                      }
                      className="w-48 rounded-lg border border-white/12 bg-black/25 px-2.5 py-1.5 text-sm font-medium outline-none focus:border-sky-400/60 select-text"
                      placeholder="供应商名称"
                    />
                    {config.providers.length > 1 && (
                      <button
                        type="button"
                        onClick={() => deleteProvider(provider.id)}
                        className="rounded-lg p-1.5 text-slate-400 transition hover:bg-rose-500/20 hover:text-rose-300"
                        title="删除供应商"
                      >
                        <Trash2 className="h-4 w-4" />
                      </button>
                    )}
                  </div>

                  <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <div>
                      <label className="mb-1 block text-[11.5px] text-slate-400">
                        Base URL（填完失焦自动修正 /v1 并扫描模型）
                      </label>
                      <input
                        type="text"
                        value={provider.baseUrl}
                        onChange={(e) =>
                          updateProvider(provider.id, {
                            baseUrl: e.target.value,
                          })
                        }
                        onBlur={() =>
                          void scanModelsForProvider(provider, true)
                        }
                        className="w-full rounded-lg border border-white/12 bg-black/25 px-2.5 py-1.5 text-xs outline-none focus:border-sky-400/60 select-text"
                        placeholder="https://api.openai.com/v1"
                      />
                    </div>
                    <div>
                      <label className="mb-1 block text-[11.5px] text-slate-400">
                        API Key（填完自动从接口拉取可用模型）
                      </label>
                      <div className="relative">
                        <input
                          type={showKeys[provider.id] ? "text" : "password"}
                          value={provider.apiKey}
                          onChange={(e) =>
                            updateProvider(provider.id, {
                              apiKey: e.target.value,
                            })
                          }
                          onBlur={() =>
                            void scanModelsForProvider(provider, true)
                          }
                          className="w-full rounded-lg border border-white/12 bg-black/25 py-1.5 pl-2.5 pr-8 text-xs outline-none focus:border-sky-400/60 select-text"
                          placeholder="sk-..."
                        />
                        <button
                          type="button"
                          onClick={() =>
                            setShowKeys((prev) => ({
                              ...prev,
                              [provider.id]: !prev[provider.id],
                            }))
                          }
                          className="absolute right-2 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-200"
                        >
                          {showKeys[provider.id] ? (
                            <EyeOff className="h-3.5 w-3.5" />
                          ) : (
                            <Eye className="h-3.5 w-3.5" />
                          )}
                        </button>
                      </div>
                    </div>
                  </div>

                  {/* Models list + Auto-discovery scanner */}
                  <div className="space-y-2.5 pt-1">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <span className="text-[11.5px] text-slate-400">
                          已启用模型列表
                        </span>
                        {scannedModels[provider.id] && (
                          <button
                            type="button"
                            onClick={() =>
                              setOpenPickerPid((cur) =>
                                cur === provider.id ? null : provider.id
                              )
                            }
                            className="rounded-full bg-emerald-500/18 px-2 py-0.5 text-[11px] text-emerald-200 hover:bg-emerald-500/28"
                          >
                            已发现 {scannedModels[provider.id].length} 个可用模型{" "}
                            {openPickerPid === provider.id ? "▲" : "▼"}
                          </button>
                        )}
                      </div>

                      <div className="flex items-center gap-1.5">
                        <button
                          type="button"
                          onClick={() =>
                            void scanModelsForProvider(provider, false)
                          }
                          disabled={scanningPid === provider.id}
                          className="flex items-center gap-1 rounded-lg border border-sky-400/35 bg-sky-500/15 px-2.5 py-1 text-[11.5px] font-medium text-sky-200 transition hover:bg-sky-500/25 disabled:opacity-50"
                        >
                          <RefreshCw
                            className={`h-3 w-3 ${
                              scanningPid === provider.id ? "animate-spin" : ""
                            }`}
                          />
                          <span>
                            {scanningPid === provider.id
                              ? "扫描中..."
                              : "从 URL 扫描可用模型"}
                          </span>
                        </button>
                        <button
                          type="button"
                          onClick={() => addModel(provider.id)}
                          className="flex items-center gap-1 rounded-lg px-2 py-1 text-[11.5px] text-slate-300 hover:bg-white/10"
                        >
                          <Plus className="h-3 w-3" /> 手动添加
                        </button>
                      </div>
                    </div>

                    {scanErrors[provider.id] && (
                      <div className="rounded-xl border border-rose-400/30 bg-rose-500/15 px-3 py-2 text-xs text-rose-200 select-text">
                        {scanErrors[provider.id]}
                      </div>
                    )}

                    {/* Searchable Discovered Models Drawer */}
                    {openPickerPid === provider.id &&
                      scannedModels[provider.id] && (
                        <div className="space-y-2 rounded-xl border border-sky-400/30 bg-black/35 p-3">
                          <div className="flex items-center gap-2">
                            <div className="relative flex-1">
                              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-400" />
                              <input
                                type="text"
                                value={modelSearchQuery[provider.id] ?? ""}
                                onChange={(e) =>
                                  setModelSearchQuery((prev) => ({
                                    ...prev,
                                    [provider.id]: e.target.value,
                                  }))
                                }
                                placeholder="搜索扫描到的模型（如 deepseek / gemini / 4o / vl / qwen）..."
                                className="w-full rounded-lg border border-white/12 bg-black/30 py-1.5 pl-8 pr-3 text-xs outline-none focus:border-sky-400/60 select-text"
                              />
                            </div>
                            <button
                              type="button"
                              onClick={() => setOpenPickerPid(null)}
                              className="rounded-lg px-2 py-1 text-xs text-slate-400 hover:bg-white/10 hover:text-white"
                            >
                              收起
                            </button>
                          </div>

                          <div className="glass-scroll max-h-48 space-y-1 overflow-y-auto pr-1">
                            {scannedModels[provider.id]
                              .filter((dm) => {
                                const q = (
                                  modelSearchQuery[provider.id] ?? ""
                                )
                                  .trim()
                                  .toLowerCase();
                                if (!q) return true;
                                return (
                                  dm.id.toLowerCase().includes(q) ||
                                  dm.label.toLowerCase().includes(q)
                                );
                              })
                              .map((dm) => {
                                const added = provider.models.some(
                                  (m) => m.id === dm.id
                                );
                                const isDefault =
                                  config.activeProviderId === provider.id &&
                                  config.activeModelId === dm.id;
                                return (
                                  <div
                                    key={dm.id}
                                    className={`flex items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-xs transition ${
                                      added
                                        ? "bg-sky-500/18 text-white"
                                        : "bg-white/4 text-slate-300 hover:bg-white/10"
                                    }`}
                                  >
                                    <button
                                      type="button"
                                      onClick={() =>
                                        toggleScannedModel(provider.id, dm)
                                      }
                                      className="flex min-w-0 flex-1 items-center gap-2 text-left"
                                    >
                                      <span
                                        className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border ${
                                          added
                                            ? "border-sky-400 bg-sky-500 text-white"
                                            : "border-white/25"
                                        }`}
                                      >
                                        {added && (
                                          <Check className="h-2.5 w-2.5" />
                                        )}
                                      </span>
                                      <span className="truncate font-mono">
                                        {dm.id}
                                      </span>
                                      {dm.vision && (
                                        <span className="shrink-0 rounded bg-violet-400/20 px-1.5 py-0.5 text-[10px] text-violet-200">
                                          识图
                                        </span>
                                      )}
                                    </button>

                                    <button
                                      type="button"
                                      onClick={() => {
                                        if (!added) {
                                          toggleScannedModel(provider.id, dm);
                                        }
                                        const nextProviders = config.providers.map(
                                          (p) => {
                                            if (p.id !== provider.id) return p;
                                            if (
                                              p.models.some((m) => m.id === dm.id)
                                            ) {
                                              return p;
                                            }
                                            const cleaned = p.models.filter(
                                              (m) => m.id.trim() !== ""
                                            );
                                            return {
                                              ...p,
                                              models: [...cleaned, dm],
                                            };
                                          }
                                        );
                                        commitConfig(
                                          {
                                            ...config,
                                            providers: nextProviders,
                                            activeProviderId: provider.id,
                                            activeModelId: dm.id,
                                          },
                                          `已切换当前模型：${dm.label || dm.id}`
                                        );
                                      }}
                                      className={`shrink-0 rounded-md px-2 py-0.5 text-[11px] transition ${
                                        isDefault
                                          ? "bg-sky-500/35 text-sky-100"
                                          : "bg-white/8 text-slate-300 hover:bg-sky-500/25 hover:text-sky-100"
                                      }`}
                                    >
                                      {isDefault ? "✓ 当前使用中" : "直接选用"}
                                    </button>
                                  </div>
                                );
                              })}
                          </div>
                        </div>
                      )}

                    {/* Datalist for manual input autocomplete */}
                    {scannedModels[provider.id] && (
                      <datalist id={`models-datalist-${provider.id}`}>
                        {scannedModels[provider.id].map((dm) => (
                          <option key={dm.id} value={dm.id}>
                            {dm.label}
                          </option>
                        ))}
                      </datalist>
                    )}

                    {provider.models.map((m, idx) => {
                      const isActive =
                        config.activeProviderId === provider.id &&
                        config.activeModelId === m.id;
                      return (
                        <div
                          key={`${provider.id}-${idx}`}
                          className={`flex flex-wrap items-center gap-2 rounded-xl border p-2 transition ${
                            isActive
                              ? "border-sky-400/50 bg-sky-500/12"
                              : "border-white/10 bg-black/20"
                          }`}
                        >
                          <input
                            type="text"
                            list={`models-datalist-${provider.id}`}
                            value={m.id}
                            onChange={(e) => {
                              const nextModels = [...provider.models];
                              nextModels[idx] = { ...m, id: e.target.value };
                              updateProvider(provider.id, {
                                models: nextModels,
                              });
                            }}
                            placeholder="模型 ID（如 gpt-4o-mini）"
                            className="min-w-[140px] flex-1 rounded-md border border-white/10 bg-black/25 px-2 py-1 text-xs outline-none focus:border-sky-400/60 select-text"
                          />
                          <input
                            type="text"
                            value={m.label}
                            onChange={(e) => {
                              const nextModels = [...provider.models];
                              nextModels[idx] = { ...m, label: e.target.value };
                              updateProvider(provider.id, {
                                models: nextModels,
                              });
                            }}
                            placeholder="显示名称"
                            className="w-28 rounded-md border border-white/10 bg-black/25 px-2 py-1 text-xs outline-none focus:border-sky-400/60 select-text"
                          />
                          <label className="flex cursor-pointer items-center gap-1 text-[11px] text-slate-300">
                            <input
                              type="checkbox"
                              checked={m.vision}
                              onChange={(e) => {
                                const nextModels = [...provider.models];
                                nextModels[idx] = {
                                  ...m,
                                  vision: e.target.checked,
                                };
                                updateProvider(provider.id, {
                                  models: nextModels,
                                });
                              }}
                            />
                            识图
                          </label>
                          <label
                            className="flex cursor-pointer items-center gap-1 text-[11px] text-slate-300"
                            title="模型支持 OpenAI Function Calling（供 Agent 自主多轮联网检索）"
                          >
                            <input
                              type="checkbox"
                              checked={m.toolCalling}
                              onChange={(e) => {
                                const nextModels = [...provider.models];
                                nextModels[idx] = {
                                  ...m,
                                  toolCalling: e.target.checked,
                                };
                                updateProvider(provider.id, {
                                  models: nextModels,
                                });
                              }}
                            />
                            Agent工具
                          </label>
                          <button
                            type="button"
                            disabled={!m.id.trim()}
                            onClick={() =>
                              commitConfig(
                                {
                                  ...config,
                                  activeProviderId: provider.id,
                                  activeModelId: m.id,
                                },
                                `已切换当前模型：${m.label || m.id}`
                              )
                            }
                            className={`rounded-md px-2.5 py-1 text-[11px] font-medium transition disabled:opacity-40 ${
                              isActive
                                ? "bg-sky-500/35 text-sky-100"
                                : "bg-white/8 text-slate-300 hover:bg-sky-500/25 hover:text-white"
                            }`}
                          >
                            {isActive ? "✓ 当前使用中" : "选用此模型"}
                          </button>
                          {provider.models.length > 1 && (
                            <button
                              type="button"
                              onClick={() => {
                                const nextModels = provider.models.filter(
                                  (_, i) => i !== idx
                                );
                                updateProvider(provider.id, {
                                  models: nextModels,
                                });
                              }}
                              className="rounded p-1 text-slate-400 hover:text-rose-300"
                            >
                              <X className="h-3.5 w-3.5" />
                            </button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))}
            </div>
          )}

          {tab === "workbuddy" && (
            <WorkBuddyPanel
              onToast={showToast}
              onConfigChanged={() => {
                void getConfig().then(setConfig);
              }}
            />
          )}

          {tab === "hotkeys" && (
            <div className="space-y-5">
              {/* Section 1: Mouse Side Button */}
              <div className="space-y-3.5 rounded-2xl border border-white/12 bg-white/5 p-4">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="flex items-center gap-2 text-sm font-medium text-white">
                      <MousePointerClick className="h-4 w-4 text-sky-300" />
                      <span>鼠标侧键快捷问 AI</span>
                    </div>
                    <p className="mt-0.5 text-xs text-slate-400">
                      选择用哪颗大拇指侧键呼出（点击即刻生效）
                    </p>
                  </div>

                  <button
                    type="button"
                    onClick={() =>
                      setRecordingField(
                        recordingField === "mouseToggle" ? null : "mouseToggle"
                      )
                    }
                    className={`rounded-xl border px-3 py-1.5 text-xs transition ${
                      recordingField === "mouseToggle"
                        ? "border-sky-400 bg-sky-500/25 text-sky-100 animate-pulse"
                        : "border-white/14 bg-white/8 text-slate-200 hover:bg-white/14"
                    }`}
                  >
                    {recordingField === "mouseToggle"
                      ? "请直接按一下你想用的鼠标侧键... (Esc 取消)"
                      : "分不清前/后？点此按一下侧键指定"}
                  </button>
                </div>

                {/* 2x2 Visual Mode Selector */}
                <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                  {MOUSE_SIDE_MODES.map((mode) => {
                    const currentRaw = config.hotkeys.mouseToggle.trim();
                    const isSelected =
                      currentRaw.toLowerCase() === mode.value.toLowerCase();
                    return (
                      <button
                        key={mode.title}
                        type="button"
                        onClick={() => {
                          setRecordingField(null);
                          commitConfig(
                            {
                              ...config,
                              hotkeys: {
                                ...config.hotkeys,
                                mouseToggle: mode.value,
                              },
                            },
                            mode.value
                              ? `已启用「${mode.title}」`
                              : "已关闭鼠标侧键触发"
                          );
                        }}
                        className={`flex flex-col items-start gap-1 rounded-xl border p-3 text-left transition ${
                          isSelected
                            ? "border-sky-400/70 bg-sky-500/16 shadow-sm"
                            : "border-white/10 bg-black/20 hover:border-white/20 hover:bg-white/6"
                        }`}
                      >
                        <div className="flex w-full items-center justify-between gap-2">
                          <div className="flex items-center gap-1.5">
                            <span className="text-[13px] font-medium text-white">
                              {mode.title}
                            </span>
                            {mode.badge && (
                              <span className="rounded-full bg-sky-400/20 px-1.5 py-0.5 text-[10px] font-medium text-sky-200">
                                {mode.badge}
                              </span>
                            )}
                          </div>
                          <span
                            className={`flex h-4 w-4 shrink-0 items-center justify-center rounded-full border ${
                              isSelected
                                ? "border-sky-400 bg-sky-500 text-white"
                                : "border-white/25"
                            }`}
                          >
                            {isSelected && <Check className="h-2.5 w-2.5" />}
                          </span>
                        </div>
                        <p className="text-[11.5px] leading-relaxed text-slate-400">
                          {mode.desc}
                        </p>
                      </button>
                    );
                  })}
                </div>

                {/* Custom modifier + side-button banner if user recorded e.g. Alt+XButton1 */}
                {!MOUSE_SIDE_MODES.some(
                  (m) =>
                    m.value.toLowerCase() ===
                    config.hotkeys.mouseToggle.trim().toLowerCase()
                ) && (
                  <div className="flex items-center justify-between rounded-xl border border-sky-400/40 bg-sky-500/15 px-3.5 py-2 text-xs text-sky-100">
                    <span>
                      当前已绑定组合侧键：
                      <strong className="ml-1 font-semibold text-white">
                        {formatMouseComboLabel(config.hotkeys.mouseToggle)}
                      </strong>
                    </span>
                    <button
                      type="button"
                      onClick={() =>
                        commitConfig(
                          {
                            ...config,
                            hotkeys: {
                              ...config.hotkeys,
                              mouseToggle: "XButton1/XButton2",
                            },
                          },
                          "已恢复默认双侧键模式"
                        )
                      }
                      className="text-sky-200 underline hover:text-white"
                    >
                      切回默认
                    </button>
                  </div>
                )}

                {/* Clear 3-Scene Explanation + Inline Auto-Ask Switch */}
                {config.hotkeys.mouseToggle.trim() !== "" && (
                  <div className="space-y-2 rounded-xl border border-white/8 bg-black/25 p-3">
                    <div className="text-[11.5px] font-medium text-slate-300">
                      按下鼠标侧键时会自动做什么？
                    </div>
                    <div className="space-y-1.5 text-xs">
                      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-white/4 px-2.5 py-2">
                        <div className="flex items-center gap-2">
                          <span className="rounded bg-sky-500/20 px-1.5 py-0.5 text-[11px] font-medium text-sky-200">
                            1. 选中文字后按
                          </span>
                          <span className="text-slate-200">
                            {config.ui.autoSubmitSelection
                              ? "自动提取选中词句，直接让 AI 翻译或解释"
                              : "自动提取选中词句填入输入框，等你输入问题"}
                          </span>
                        </div>
                        <div className="flex items-center gap-1.5">
                          <span className="text-[11px] text-slate-400">
                            直接解答
                          </span>
                          <ToggleSwitch
                            checked={config.ui.autoSubmitSelection}
                            onChange={(checked) =>
                              commitConfig(
                                {
                                  ...config,
                                  ui: {
                                    ...config.ui,
                                    autoSubmitSelection: checked,
                                  },
                                },
                                checked
                                  ? "划词按侧键将直接解答"
                                  : "划词按侧键将先等你输入问题"
                              )
                            }
                          />
                        </div>
                      </div>

                      <div className="flex items-center gap-2 rounded-lg bg-white/4 px-2.5 py-1.5">
                        <span className="rounded bg-violet-500/20 px-1.5 py-0.5 text-[11px] font-medium text-violet-200">
                          2. 刚截完图后按
                        </span>
                        <span className="text-slate-300">
                          自动带上刚截好的图弹出输入框，等你提问
                        </span>
                      </div>

                      <div className="flex items-center gap-2 rounded-lg bg-white/4 px-2.5 py-1.5">
                        <span className="rounded bg-emerald-500/20 px-1.5 py-0.5 text-[11px] font-medium text-emerald-200">
                          3. 平时直接按
                        </span>
                        <span className="text-slate-300">
                          在悬浮球旁随时打开 / 收起 AI 输入框
                        </span>
                      </div>
                    </div>
                  </div>
                )}
              </div>

              {/* Section 2: Keyboard Shortcuts */}
              <div className="space-y-3 rounded-2xl border border-white/12 bg-white/5 p-4">
                <div>
                  <div className="flex items-center gap-2 text-sm font-medium text-white">
                    <Keyboard className="h-4 w-4 text-sky-300" />
                    <span>键盘快捷键</span>
                  </div>
                  <p className="mt-0.5 text-xs text-slate-400">
                    点击右侧按键框即可按下新组合键替换（按 Esc 取消）
                  </p>
                </div>

                <div className="space-y-2">
                  {KEYBOARD_SHORTCUT_ITEMS.map((item) => {
                    const isRec = recordingField === item.field;
                    const rawVal = config.hotkeys[item.field].trim();
                    const bindErr = bindResults.find(
                      (r) => r.action === item.field && !r.ok
                    );
                    const keys = rawVal ? rawVal.split("+") : [];
                    return (
                      <div
                        key={item.field}
                        className="flex items-center justify-between gap-3 rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5"
                      >
                        <div className="min-w-0">
                          <div className="text-[13px] text-white">
                            {item.title}
                          </div>
                          <div className="text-[11px] text-slate-400">
                            {bindErr?.error ? (
                              <span className="text-rose-300">
                                {bindErr.error}
                              </span>
                            ) : (
                              item.desc
                            )}
                          </div>
                        </div>

                        <div className="flex shrink-0 items-center gap-1.5">
                          <button
                            type="button"
                            onClick={() =>
                              setRecordingField(isRec ? null : item.field)
                            }
                            className={`flex min-w-[136px] items-center justify-center gap-1 rounded-xl border px-3 py-1.5 text-xs transition ${
                              isRec
                                ? "border-sky-400 bg-sky-500/25 text-sky-100 animate-pulse"
                                : "border-white/14 bg-white/8 text-slate-200 hover:bg-white/14"
                            }`}
                          >
                            {isRec ? (
                              <span>请按下组合键...</span>
                            ) : keys.length > 0 ? (
                              keys.map((k, i) => (
                                <span
                                  key={`${k}-${i}`}
                                  className="inline-flex items-center gap-1"
                                >
                                  {i > 0 && (
                                    <span className="text-slate-500">+</span>
                                  )}
                                  <kbd className="rounded border border-white/18 bg-black/35 px-1.5 py-0.5 font-mono text-[11px] text-slate-100">
                                    {k}
                                  </kbd>
                                </span>
                              ))
                            ) : (
                              <span className="text-slate-500">未设置</span>
                            )}
                          </button>

                          {rawVal !== item.defaultValue && (
                            <button
                              type="button"
                              onClick={() =>
                                commitConfig(
                                  {
                                    ...config,
                                    hotkeys: {
                                      ...config.hotkeys,
                                      [item.field]: item.defaultValue,
                                    },
                                  },
                                  `已恢复默认 ${item.defaultValue}`
                                )
                              }
                              title={`恢复默认 (${item.defaultValue})`}
                              className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-sky-200"
                            >
                              <RotateCcw className="h-3.5 w-3.5" />
                            </button>
                          )}

                          {rawVal && (
                            <button
                              type="button"
                              onClick={() =>
                                commitConfig(
                                  {
                                    ...config,
                                    hotkeys: {
                                      ...config.hotkeys,
                                      [item.field]: "",
                                    },
                                  },
                                  "已清空该快捷键"
                                )
                              }
                              title="清空此快捷键"
                              className="rounded-lg p-1.5 text-slate-400 hover:bg-white/10 hover:text-rose-300"
                            >
                              <X className="h-3.5 w-3.5" />
                            </button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>

              {/* Section 3: Anti-conflict & App Exceptions */}
              <div className="space-y-3 rounded-2xl border border-white/12 bg-white/5 p-4">
                <div className="flex items-center gap-2 text-sm font-medium text-white">
                  <ShieldCheck className="h-4 w-4 text-sky-300" />
                  <span>游戏与软件防误触</span>
                </div>

                <div className="flex items-center justify-between gap-4 rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                  <div>
                    <div className="text-[13px] text-white">
                      全屏游戏 / 全屏看视频时自动暂停侧键
                    </div>
                    <div className="text-[11px] text-slate-400">
                      全屏画面下自动忽略鼠标侧键，绝不切出游戏或打断观影
                    </div>
                  </div>
                  <ToggleSwitch
                    checked={config.mousePriority.passthroughFullscreen}
                    onChange={(checked) =>
                      commitConfig({
                        ...config,
                        mousePriority: {
                          ...config.mousePriority,
                          passthroughFullscreen: checked,
                        },
                      })
                    }
                  />
                </div>

                <ProcessPickerSection
                  title="在指定软件里完全禁用 BTW-Float（游戏免打扰）"
                  desc="切换到这些软件时自动隐藏悬浮球与悬浮窗，并暂停全部快捷键与侧键；切回其他软件后自动恢复"
                  accent="rose"
                  processes={config.mousePriority.disabledProcesses ?? []}
                  runningProcs={runningProcs}
                  onRefreshRunning={refreshRunningProcesses}
                  onAdd={addDisabledProcess}
                  onRemove={removeDisabledProcess}
                />

                <ProcessPickerSection
                  title="在指定软件里仅禁用鼠标侧键"
                  desc="若某个软件本身要用侧键，加到这里即可放行侧键，悬浮球与键盘快捷键仍可正常使用"
                  accent="sky"
                  processes={config.mousePriority.blacklistProcesses}
                  runningProcs={runningProcs}
                  onRefreshRunning={refreshRunningProcesses}
                  onAdd={addBlacklistProcess}
                  onRemove={removeBlacklistProcess}
                />
              </div>
            </div>
          )}

          {tab === "ui" && (
            <div className="space-y-3 rounded-2xl border border-white/12 bg-white/5 p-4">
              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">显示桌面悬浮球</div>
                  <div className="text-[11px] text-slate-400">
                    隐藏后仍可通过鼠标侧键、键盘快捷键或系统托盘呼出
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.showBall}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, showBall: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">悬浮球贴边半隐藏</div>
                  <div className="text-[11px] text-slate-400">
                    拖至屏幕左右边缘时自动收起大半，鼠标移过去再滑出
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.ballAutoHideEdge}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, ballAutoHideEdge: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">
                    点击外部自动收起并清空对话
                  </div>
                  <div className="text-[11px] text-slate-400">
                    未点击图钉固定时，点窗口外面自动隐藏并清空本次提问
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.popupAutoHideOnBlur}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, popupAutoHideOnBlur: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">
                    划词后按侧键直接自动问 AI
                  </div>
                  <div className="text-[11px] text-slate-400">
                    选中文字按侧键立刻翻译/解释（关闭则先填入输入框等你提问）
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.autoSubmitSelection}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, autoSubmitSelection: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">
                    默认开启免费联网搜索
                  </div>
                  <div className="text-[11px] text-slate-400">
                    AI 需要查最新资料时自动联网检索并整合进回答
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.defaultWebSearch}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, defaultWebSearch: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">
                    显示联网搜索的参考网站来源
                  </div>
                  <div className="text-[11px] text-slate-400">
                    默认关闭以保持回答界面清爽；开启后在回答底部显示参考网页链接
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.showSearchSources}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, showSearchSources: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">开机自动启动</div>
                  <div className="text-[11px] text-slate-400">
                    进入系统后自动在后台运行
                  </div>
                </div>
                <ToggleSwitch
                  checked={config.ui.autostart}
                  onChange={(checked) =>
                    commitConfig({
                      ...config,
                      ui: { ...config.ui, autostart: checked },
                    })
                  }
                />
              </div>

              <div className="flex items-center justify-between rounded-xl border border-white/8 bg-black/20 px-3.5 py-2.5">
                <div>
                  <div className="text-[13px] text-white">
                    截图自动附带有效时间（秒）
                  </div>
                  <div className="text-[11px] text-slate-400">
                    刚截完图多少秒内按侧键会自动挂上这张图（每张图只自动挂一次）
                  </div>
                </div>
                <input
                  type="number"
                  min={5}
                  max={600}
                  value={config.ui.clipboardImageFreshnessSecs}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      ui: {
                        ...config.ui,
                        clipboardImageFreshnessSecs: Math.max(
                          5,
                          Number(e.target.value) || 60
                        ),
                      },
                    })
                  }
                  onBlur={() => commitConfig(config)}
                  className="w-20 rounded-lg border border-white/14 bg-black/30 px-2.5 py-1 text-center text-xs outline-none select-text"
                />
              </div>

              {/* Customizable quick suggestion prompts */}
              <div className="space-y-2 rounded-xl border border-white/8 bg-black/20 p-3.5">
                <div className="flex items-center justify-between">
                  <div>
                    <div className="text-[13px] text-white">常用快捷提问词</div>
                    <div className="text-[11px] text-slate-400">
                      平时直接打开输入框时，在下方显示的快捷问题按钮
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() =>
                      setConfig({
                        ...config,
                        ui: {
                          ...config.ui,
                          quickPrompts: [...config.ui.quickPrompts, ""],
                        },
                      })
                    }
                    className="flex items-center gap-1 rounded-lg px-2 py-1 text-[11.5px] text-sky-300 hover:bg-sky-400/15"
                  >
                    <Plus className="h-3 w-3" /> 添加
                  </button>
                </div>

                <div className="space-y-1.5">
                  {config.ui.quickPrompts.map((prompt, idx) => (
                    <div key={idx} className="flex items-center gap-2">
                      <input
                        type="text"
                        value={prompt}
                        onChange={(e) => {
                          const next = [...config.ui.quickPrompts];
                          next[idx] = e.target.value;
                          setConfig({
                            ...config,
                            ui: { ...config.ui, quickPrompts: next },
                          });
                        }}
                        placeholder="输入快捷提问..."
                        className="flex-1 rounded-lg border border-white/12 bg-black/25 px-2.5 py-1.5 text-xs outline-none focus:border-sky-400/60 select-text"
                      />
                      {config.ui.quickPrompts.length > 1 && (
                        <button
                          type="button"
                          onClick={() =>
                            commitConfig({
                              ...config,
                              ui: {
                                ...config.ui,
                                quickPrompts: config.ui.quickPrompts.filter(
                                  (_, i) => i !== idx
                                ),
                              },
                            })
                          }
                          className="rounded p-1 text-slate-400 hover:text-rose-300"
                        >
                          <X className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
