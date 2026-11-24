import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowUp,
  Check,
  Copy,
  FileText,
  Globe,
  MoreHorizontal,
  Pin,
  Sparkles,
  Square,
  X,
} from "lucide-react";
import {
  abortChatStream,
  getConfig,
  hidePopup,
  openSettings,
  resizePopup,
  setDefaultWebSearch,
  setPopupPin,
  startChatStream,
  takePendingPrefill,
} from "../lib/commands";
import {
  buildAutoSelectionPrompt,
  compressImageFileToDataUrl,
  computeSmartSuggestions,
} from "../lib/suggestions";
import { useSystemDark } from "../lib/theme";
import { MarkdownView } from "../components/MarkdownView";
import { SourceList } from "../components/SourceList";
import type {
  AppConfig,
  ChatMessage,
  PopupPrefillPayload,
  SearchSource,
} from "../types/app";

interface DeltaEvent {
  requestId: string;
  delta: string;
}

interface StatusEvent {
  requestId: string;
  status: string;
  detail?: string | null;
}

interface DoneEvent {
  requestId: string;
}

interface ErrorEvent {
  requestId: string;
  message: string;
}

interface SourcesEvent {
  requestId: string;
  sources: SearchSource[];
}

interface AttachedImage {
  id: string;
  dataUrl: string;
  label: string;
}

const CAPSULE_COLLAPSED_H = 58;

function sourceLabel(source?: string | null): string {
  switch (source) {
    case "clipboard_image":
      return "刚截的图";
    case "region_capture":
      return "框选截图";
    case "window_capture":
      return "窗口截图";
    case "drop":
      return "拖入内容";
    default:
      return "图片附件";
  }
}

export function PopupWindow() {
  const isDark = useSystemDark();
  const [config, setConfig] = useState<AppConfig | null>(null);

  const [input, setInput] = useState("");
  const [attachedText, setAttachedText] = useState<string | null>(null);
  const [showTextPreview, setShowTextPreview] = useState(false);
  const [attachedImages, setAttachedImages] = useState<AttachedImage[]>([]);
  const [dragOver, setDragOver] = useState(false);

  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const llmHistoryRef = useRef<
    Array<{ role: string; content: string; images: string[] }>
  >([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [needKeyNotice, setNeedKeyNotice] = useState(false);
  const [statusHint, setStatusHint] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [pinned, setPinned] = useState(false);
  const [webSearch, setWebSearch] = useState(false);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const activeReqIdRef = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const capsuleRef = useRef<HTMLDivElement | null>(null);
  const scrollEndRef = useRef<HTMLDivElement | null>(null);
  const lastHeightRef = useRef<number>(CAPSULE_COLLAPSED_H);

  const hasAttachments =
    Boolean(attachedText) || attachedImages.length > 0;

  const suggestions = useMemo(
    () =>
      computeSmartSuggestions({
        attachedText,
        imageCount: attachedImages.length,
        quickPrompts: config?.ui.quickPrompts ?? [],
      }),
    [attachedText, attachedImages.length, config?.ui.quickPrompts]
  );

  // Show suggestions automatically when context is attached (or while typing/starting fresh)
  const showSuggestionsBar =
    messages.length === 0 && (hasAttachments || input.trim().length > 0);

  const isExpanded =
    messages.length > 0 ||
    Boolean(errorMsg) ||
    Boolean(statusHint) ||
    needKeyNotice ||
    hasAttachments ||
    showSuggestionsBar;

  const activeProvider = useMemo(() => {
    if (!config) return null;
    return (
      config.providers.find((p) => p.id === config.activeProviderId) ??
      config.providers[0] ??
      null
    );
  }, [config]);

  const needsApiSetup =
    activeProvider !== null &&
    activeProvider.apiKey.trim() === "" &&
    !activeProvider.baseUrl.includes("localhost") &&
    !activeProvider.baseUrl.includes("127.0.0.1");

  const applyPrefill = useCallback(async (pf: PopupPrefillPayload | null) => {
    if (!pf) return;

    if (pf.text && pf.autoSubmit) {
      const cfg = await getConfig();
      setConfig(cfg);
      const prov =
        cfg.providers.find((p) => p.id === cfg.activeProviderId) ??
        cfg.providers[0];
      const missingKey =
        !prov ||
        (prov.apiKey.trim() === "" &&
          !prov.baseUrl.includes("localhost") &&
          !prov.baseUrl.includes("127.0.0.1"));

      if (missingKey) {
        setAttachedText(pf.text);
        setShowTextPreview(false);
        setNeedKeyNotice(true);
        return;
      }

      // Immediately ask AI to explain/translate the highlighted passage!
      if (activeReqIdRef.current) {
        void abortChatStream(activeReqIdRef.current);
      }
      const { displayLabel, llmContent } = buildAutoSelectionPrompt(pf.text);
      const userMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "user",
        content: displayLabel,
      };
      const assistantMsg: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
      };
      const nextLlm = [{ role: "user", content: llmContent, images: [] }];
      llmHistoryRef.current = nextLlm;

      setAttachedText(null);
      setAttachedImages([]);
      setNeedKeyNotice(false);
      setErrorMsg(null);
      setMessages([userMsg, assistantMsg]);
      setIsStreaming(true);
      setStatusHint("正在解读选中内容...");

      const reqId = crypto.randomUUID();
      activeReqIdRef.current = reqId;
      try {
        await startChatStream({
          requestId: reqId,
          messages: nextLlm,
          providerId: cfg.activeProviderId,
          modelId: cfg.activeModelId,
          webSearch: cfg.ui.defaultWebSearch,
        });
      } catch (err) {
        activeReqIdRef.current = null;
        setIsStreaming(false);
        setStatusHint(null);
        setErrorMsg(String(err));
      }
      return;
    }

    if (pf.text) {
      setAttachedText(pf.text);
      setShowTextPreview(false);
    }
    if (pf.image) {
      setAttachedImages([
        {
          id: crypto.randomUUID(),
          dataUrl: pf.image,
          label: sourceLabel(pf.source),
        },
      ]);
    }
  }, []);

  const resetSession = useCallback(() => {
    if (activeReqIdRef.current) {
      void abortChatStream(activeReqIdRef.current);
      activeReqIdRef.current = null;
    }
    llmHistoryRef.current = [];
    setInput("");
    setAttachedText(null);
    setShowTextPreview(false);
    setAttachedImages([]);
    setNeedKeyNotice(false);
    setMessages([]);
    setIsStreaming(false);
    setStatusHint(null);
    setErrorMsg(null);
    setPinned(false);
    lastHeightRef.current = CAPSULE_COLLAPSED_H;
  }, []);

  useEffect(() => {
    void getConfig().then((cfg) => {
      setConfig(cfg);
      setWebSearch(cfg.ui.defaultWebSearch);
    });
    void takePendingPrefill().then(applyPrefill);

    const listeners = [
      listen<AppConfig>("config:updated", (ev) => {
        setConfig(ev.payload);
        setWebSearch(ev.payload.ui.defaultWebSearch);
      }),
      listen("popup:opened", () => {
        lastHeightRef.current = CAPSULE_COLLAPSED_H;
        void takePendingPrefill().then(applyPrefill);
        window.setTimeout(() => {
          inputRef.current?.focus();
        }, 10);
      }),
      listen<PopupPrefillPayload>("popup:prefill", (ev) => {
        applyPrefill(ev.payload);
        // Clear pending slot so we don't apply twice
        void takePendingPrefill();
      }),
      listen("popup:reset", () => {
        resetSession();
      }),
      listen<DeltaEvent>("chat:delta", (ev) => {
        if (ev.payload.requestId !== activeReqIdRef.current) return;
        setStatusHint(null);
        setMessages((prev) => {
          if (prev.length === 0) return prev;
          const next = [...prev];
          const last = next[next.length - 1];
          if (last && last.role === "assistant") {
            const updatedContent = last.content + ev.payload.delta;
            next[next.length - 1] = {
              ...last,
              content: updatedContent,
            };
            // Keep llmHistoryRef synced with assistant reply for multi-turn follow-ups
            const hist = llmHistoryRef.current;
            if (hist.length > 0 && hist[hist.length - 1].role === "assistant") {
              hist[hist.length - 1].content = updatedContent;
            } else {
              hist.push({
                role: "assistant",
                content: updatedContent,
                images: [],
              });
            }
          }
          return next;
        });
      }),
      listen<StatusEvent>("chat:status", (ev) => {
        if (ev.payload.requestId !== activeReqIdRef.current) return;
        if (ev.payload.detail) {
          setStatusHint(ev.payload.detail);
        } else if (ev.payload.status === "thinking") {
          setStatusHint("正在整理答案...");
        } else if (ev.payload.status === "connecting") {
          setStatusHint("连接模型中...");
        }
      }),
      listen<SourcesEvent>("chat:sources", (ev) => {
        if (ev.payload.requestId !== activeReqIdRef.current) return;
        setMessages((prev) => {
          if (prev.length === 0) return prev;
          const next = [...prev];
          const last = next[next.length - 1];
          if (last && last.role === "assistant") {
            next[next.length - 1] = {
              ...last,
              sources: ev.payload.sources,
            };
          }
          return next;
        });
      }),
      listen<DoneEvent>("chat:done", (ev) => {
        if (ev.payload.requestId !== activeReqIdRef.current) return;
        activeReqIdRef.current = null;
        setIsStreaming(false);
        setStatusHint(null);
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === "assistant" && !last.content.trim()) {
            const hist = llmHistoryRef.current;
            if (hist.length > 0 && hist[hist.length - 1].role === "user") {
              hist.push({
                role: "assistant",
                content: "（未返回文本内容）",
                images: [],
              });
            }
            setErrorMsg("模型未返回有效回答内容，请重试或检查当前模型是否兼容");
          }
          return prev;
        });
      }),
      listen<ErrorEvent>("chat:error", (ev) => {
        if (ev.payload.requestId !== activeReqIdRef.current) return;
        activeReqIdRef.current = null;
        setIsStreaming(false);
        setStatusHint(null);
        const hist = llmHistoryRef.current;
        if (hist.length > 0 && hist[hist.length - 1].role === "user") {
          hist.push({
            role: "assistant",
            content: "（请求异常中断）",
            images: [],
          });
        }
        setErrorMsg(ev.payload.message);
      }),
    ];

    return () => {
      listeners.forEach((p) => {
        void p.then((fn) => fn());
      });
    };
  }, [applyPrefill, resetSession]);

  // Global Esc shortcut inside popup: immediately hide & clear session
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void hidePopup();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // Dynamically sync native window height only when height genuinely changes
  useEffect(() => {
    const el = capsuleRef.current;
    if (!el) return;

    const syncHeight = () => {
      const targetH = isExpanded
        ? Math.min(Math.max(el.scrollHeight, 92), 564)
        : CAPSULE_COLLAPSED_H;
      if (Math.abs(targetH - lastHeightRef.current) >= 2) {
        lastHeightRef.current = targetH;
        void resizePopup(targetH);
      }
    };

    syncHeight();
    const ro = new ResizeObserver(syncHeight);
    ro.observe(el);
    return () => ro.disconnect();
  }, [
    isExpanded,
    messages,
    errorMsg,
    statusHint,
    needsApiSetup,
    attachedText,
    showTextPreview,
    attachedImages,
    showSuggestionsBar,
  ]);

  useEffect(() => {
    if (isStreaming) {
      scrollEndRef.current?.scrollIntoView({ block: "end" });
    }
  }, [messages, isStreaming]);

  const handleSend = async (overridePrompt?: string) => {
    if (isStreaming) return;
    const rawPrompt = (overridePrompt ?? input).trim();
    if (!rawPrompt && !hasAttachments) return;

    const effectivePrompt =
      rawPrompt || suggestions[0] || "请帮我分析并处理以上内容";

    // Compose prompt with attached selection text if present
    const fullContent = attachedText
      ? `${effectivePrompt}\n\n---\n${attachedText}`
      : effectivePrompt;
    const imageUrls = attachedImages.map((img) => img.dataUrl);

    setErrorMsg(null);
    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content: effectivePrompt,
      images: imageUrls.length > 0 ? imageUrls : undefined,
    };
    // Keep full content (including selection) for LLM request while keeping bubble clean
    const llmUserMessage = {
      role: "user",
      content: fullContent,
      images: imageUrls,
    };
    const assistantMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: "assistant",
      content: "",
    };

    const nextLlm = [
      ...llmHistoryRef.current,
      llmUserMessage,
    ];
    llmHistoryRef.current = nextLlm;

    setMessages([...messages, userMsg, assistantMsg]);
    setInput("");
    setAttachedText(null);
    setShowTextPreview(false);
    setAttachedImages([]);
    setNeedKeyNotice(false);
    setIsStreaming(true);
    setStatusHint("连接模型中...");

    const reqId = crypto.randomUUID();
    activeReqIdRef.current = reqId;

    try {
      await startChatStream({
        requestId: reqId,
        messages: nextLlm,
        providerId: config?.activeProviderId,
        modelId: config?.activeModelId,
        webSearch,
      });
    } catch (err) {
      activeReqIdRef.current = null;
      setIsStreaming(false);
      setStatusHint(null);
      setErrorMsg(String(err));
    }
  };

  const handlePaste = async (e: React.ClipboardEvent) => {
    const items = Array.from(e.clipboardData.items);
    const imgItem = items.find((it) => it.type.startsWith("image/"));
    if (!imgItem) return;
    e.preventDefault();
    const file = imgItem.getAsFile();
    if (!file) return;
    try {
      const dataUrl = await compressImageFileToDataUrl(file);
      setAttachedImages((prev) => [
        ...prev.slice(-2),
        {
          id: crypto.randomUUID(),
          dataUrl,
          label: "粘贴图片",
        },
      ]);
    } catch {
      // ignore invalid image paste
    }
  };

  const handleDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);

    const files = Array.from(e.dataTransfer.files);
    const imgFile = files.find((f) => f.type.startsWith("image/"));
    if (imgFile) {
      try {
        const dataUrl = await compressImageFileToDataUrl(imgFile);
        setAttachedImages((prev) => [
          ...prev.slice(-2),
          {
            id: crypto.randomUUID(),
            dataUrl,
            label: imgFile.name || "拖入图片",
          },
        ]);
      } catch {
        // ignore
      }
      return;
    }

    const droppedText = e.dataTransfer.getData("text/plain");
    if (droppedText && droppedText.trim()) {
      setAttachedText(droppedText.trim().slice(0, 12000));
    }
  };

  const handleStop = () => {
    if (activeReqIdRef.current) {
      void abortChatStream(activeReqIdRef.current);
      activeReqIdRef.current = null;
    }
    setIsStreaming(false);
    setStatusHint(null);
  };

  const handleTogglePin = () => {
    const next = !pinned;
    setPinned(next);
    void setPopupPin(next);
  };

  const handleCopyMessage = async (msg: ChatMessage) => {
    try {
      await navigator.clipboard.writeText(msg.content);
      setCopiedId(msg.id);
      setTimeout(() => setCopiedId(null), 1400);
    } catch {
      // ignore
    }
  };

  const canSend = Boolean(input.trim()) || hasAttachments;

  const iconBtn = isDark
    ? "text-white/65 hover:bg-white/12 hover:text-white"
    : "text-slate-500 hover:bg-slate-900/8 hover:text-slate-900";
  const activeBtn = isDark
    ? "bg-sky-400/25 text-sky-200"
    : "bg-sky-500/15 text-sky-700";

  // Drag the capsule by its orb or any empty area (never hijack inputs / buttons / selects)
  const handleDragMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("input, textarea, button, select, a")) return;
    e.preventDefault();
    void getCurrentWindow().startDragging();
  };

  return (
    <div
      className="h-full w-full overflow-hidden bg-transparent px-[22px] pb-[27px] pt-[15px]"
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => void handleDrop(e)}
    >
      <div
        ref={capsuleRef}
        style={{
          borderRadius: isExpanded ? "22px" : "29px",
        }}
        className={`flex min-h-[58px] w-full flex-col overflow-hidden transition-[border-radius] duration-200 ${
          isDark ? "frosted-capsule-dark" : "frosted-capsule-light"
        } ${dragOver ? "ring-2 ring-sky-400/70" : ""}`}
      >
        {/* Top Capsule Input Row (58px) — drag empty areas or the left orb to move */}
        <div
          className="flex h-[58px] shrink-0 items-center gap-1.5 pl-3 pr-2.5"
          onMouseDown={handleDragMouseDown}
        >
          <div
            title="按住拖动可移动输入框"
            className={`flex h-7 w-7 shrink-0 cursor-grab items-center justify-center rounded-full active:cursor-grabbing ${
              isDark
                ? "bg-white/12 text-sky-300"
                : "bg-sky-500/12 text-sky-600"
            }`}
          >
            <Sparkles
              className={`pointer-events-none h-3.5 w-3.5 ${
                isStreaming ? "animate-pulse-soft" : ""
              }`}
            />
          </div>

          <input
            ref={inputRef}
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onPaste={(e) => void handlePaste(e)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void handleSend();
              } else if (
                e.key === "Tab" &&
                !input.trim() &&
                suggestions.length > 0
              ) {
                e.preventDefault();
                setInput(suggestions[0]);
              }
            }}
            placeholder={
              messages.length > 0
                ? "继续追问…"
                : hasAttachments
                ? `直接提问，或按 Tab：${suggestions[0] ?? ""}`
                : "问点什么…"
            }
            className={`ml-1 min-w-0 flex-1 bg-transparent text-[14px] font-[450] outline-none select-text ${
              isDark
                ? "text-white placeholder:text-white/45"
                : "text-slate-900 placeholder:text-slate-400"
            }`}
          />

          {/* Minimal controls */}
          <div className="flex shrink-0 items-center gap-0.5">
            {pinned && (
              <button
                type="button"
                onClick={handleTogglePin}
                title="已固定（点击取消）"
                className={`flex h-7 w-7 items-center justify-center rounded-full transition ${activeBtn}`}
              >
                <Pin className="h-3.5 w-3.5" />
              </button>
            )}

            <button
              type="button"
              onClick={() => {
                const next = !webSearch;
                setWebSearch(next);
                void setDefaultWebSearch(next);
              }}
              title={webSearch ? "联网搜索：已开启（点击关闭）" : "联网搜索：已关闭（点击开启）"}
              className={`flex h-7 w-7 items-center justify-center rounded-full transition ${
                webSearch ? activeBtn : iconBtn
              }`}
            >
              <Globe className="h-3.5 w-3.5" />
            </button>

            <button
              type="button"
              onClick={() => void openSettings()}
              title="设置（模型 / 快捷键 / 偏好）"
              className={`flex h-7 w-7 items-center justify-center rounded-full transition ${iconBtn}`}
            >
              <MoreHorizontal className="h-4 w-4" />
            </button>

            {isStreaming ? (
              <button
                type="button"
                onClick={handleStop}
                title="停止生成"
                className="ml-0.5 flex h-7 w-7 items-center justify-center rounded-full bg-rose-500 text-white shadow-xs transition hover:bg-rose-400"
              >
                <Square className="h-2.5 w-2.5 fill-current" />
              </button>
            ) : (
              canSend && (
                <button
                  type="button"
                  onClick={() => void handleSend()}
                  title="发送 (Enter)"
                  className={`ml-0.5 flex h-7 w-7 items-center justify-center rounded-full shadow-xs transition hover:scale-105 ${
                    isDark ? "bg-white text-slate-900" : "bg-slate-900 text-white"
                  }`}
                >
                  <ArrowUp className="h-3.5 w-3.5" />
                </button>
              )
            )}
          </div>
        </div>

        {/* Attached Context Bar (Selected Text / Fresh Screenshot / Cropped Image) */}
        {hasAttachments && (
          <div
            className={`flex flex-col gap-2 border-t px-4 py-2.5 ${
              isDark ? "border-white/10" : "border-slate-900/8"
            }`}
          >
            <div className="flex flex-wrap items-center gap-2">
              {attachedText && (
                <div
                  className={`inline-flex items-center gap-1.5 rounded-xl border px-2.5 py-1 text-xs ${
                    isDark
                      ? "border-sky-400/25 bg-sky-400/12 text-sky-200"
                      : "border-sky-500/25 bg-sky-500/10 text-sky-800"
                  }`}
                >
                  <FileText className="h-3.5 w-3.5 shrink-0" />
                  <button
                    type="button"
                    onClick={() => setShowTextPreview((v) => !v)}
                    className="font-medium hover:underline"
                    title="点击展开/收起选中文本预览"
                  >
                    已捕获选中 {attachedText.length} 字
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setAttachedText(null);
                      setShowTextPreview(false);
                    }}
                    title="移除选中文本"
                    className="ml-0.5 rounded-full p-0.5 opacity-70 hover:opacity-100"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              )}

              {attachedImages.map((img) => (
                <div
                  key={img.id}
                  className={`group relative inline-flex items-center gap-2 rounded-xl border p-1 pr-2.5 text-xs ${
                    isDark
                      ? "border-white/14 bg-white/8 text-slate-200"
                      : "border-slate-900/10 bg-slate-900/5 text-slate-700"
                  }`}
                >
                  <img
                    src={img.dataUrl}
                    alt={img.label}
                    className="h-9 w-14 rounded-lg object-cover"
                  />
                  <span className="max-w-[90px] truncate text-[11.5px]">
                    {img.label}
                  </span>
                  <button
                    type="button"
                    onClick={() =>
                      setAttachedImages((prev) =>
                        prev.filter((item) => item.id !== img.id)
                      )
                    }
                    title="移除图片"
                    className="rounded-full p-0.5 opacity-70 hover:opacity-100"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              ))}
            </div>

            {attachedText && showTextPreview && (
              <div
                className={`glass-scroll max-h-24 overflow-y-auto rounded-xl px-3 py-2 font-mono text-[11.5px] leading-relaxed select-text ${
                  isDark
                    ? "bg-black/25 text-slate-200"
                    : "bg-slate-900/6 text-slate-700"
                }`}
              >
                {attachedText}
              </div>
            )}
          </div>
        )}

        {/* 0ms Smart Input Suggestions Bar */}
        {showSuggestionsBar && suggestions.length > 0 && (
          <div
            className={`flex flex-wrap items-center gap-1.5 border-t px-4 py-2.5 ${
              isDark ? "border-white/10" : "border-slate-900/8"
            }`}
          >
            {suggestions.map((sug, i) => (
              <button
                key={sug}
                type="button"
                onClick={() => void handleSend(sug)}
                className={`inline-flex items-center gap-1 rounded-full border px-2.5 py-1 text-[11.5px] transition ${
                  isDark
                    ? "border-white/12 bg-white/8 text-slate-200 hover:border-sky-400/40 hover:bg-sky-400/18 hover:text-white"
                    : "border-slate-900/10 bg-slate-900/5 text-slate-700 hover:border-sky-500/40 hover:bg-sky-500/12 hover:text-sky-900"
                }`}
              >
                {i === 0 && !input.trim() && (
                  <span className="rounded bg-current/15 px-1 text-[9.5px] font-mono opacity-75">
                    Tab
                  </span>
                )}
                <span>{sug}</span>
              </button>
            ))}
          </div>
        )}

        {/* Expanded Conversation Body */}
        {(messages.length > 0 ||
          Boolean(errorMsg) ||
          Boolean(statusHint) ||
          needKeyNotice) && (
          <div
            className={`glass-scroll max-h-[450px] overflow-y-auto border-t px-4 pb-4 pt-3 ${
              isDark ? "border-white/10" : "border-slate-900/8"
            }`}
          >
            {(needsApiSetup || needKeyNotice) && messages.length === 0 && (
              <div
                className={`mb-2 flex items-center justify-between rounded-xl px-3 py-2 text-xs ${
                  isDark
                    ? "bg-amber-400/18 text-amber-100"
                    : "bg-amber-500/14 text-amber-950"
                }`}
              >
                <span>
                  {attachedText
                    ? "已抓取选中内容！先配置 API Key，之后划词按侧键将直接自动解答。"
                    : "尚未填写 API Key，先配置模型接口即可开始提问。"}
                </span>
                <button
                  type="button"
                  onClick={() => void openSettings()}
                  className="ml-2 shrink-0 rounded-lg bg-amber-400/30 px-2.5 py-1 font-medium hover:bg-amber-400/40"
                >
                  去配置
                </button>
              </div>
            )}

            <div className="space-y-3">
              {messages.map((msg) =>
                msg.role === "user" ? (
                  <div key={msg.id} className="flex flex-col items-end gap-1.5">
                    {msg.images && msg.images.length > 0 && (
                      <div className="flex flex-wrap justify-end gap-1.5">
                        {msg.images.map((src, idx) => (
                          <img
                            key={idx}
                            src={src}
                            alt=""
                            className="max-h-24 max-w-[180px] rounded-xl border border-white/15 object-cover"
                          />
                        ))}
                      </div>
                    )}
                    <div
                      className={`max-w-[85%] rounded-2xl rounded-tr-sm px-3.5 py-2 text-[13.5px] leading-relaxed select-text ${
                        isDark
                          ? "bg-white/14 text-white"
                          : "bg-slate-900/7 text-slate-900"
                      }`}
                    >
                      {msg.content}
                    </div>
                  </div>
                ) : (
                  <div key={msg.id} className="group relative">
                    {msg.content.trim() ? (
                      <>
                        <MarkdownView content={msg.content} isDark={isDark} />
                        {config?.ui.showSearchSources &&
                          msg.sources &&
                          msg.sources.length > 0 && (
                            <SourceList sources={msg.sources} isDark={isDark} />
                          )}
                        <div className="mt-1.5 flex items-center gap-2 opacity-0 transition-opacity group-hover:opacity-100">
                          <button
                            type="button"
                            onClick={() => void handleCopyMessage(msg)}
                            className={`flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] ${
                              isDark
                                ? "text-white/70 hover:bg-white/12"
                                : "text-slate-600 hover:bg-slate-900/8"
                            }`}
                          >
                            {copiedId === msg.id ? (
                              <Check className="h-3 w-3" />
                            ) : (
                              <Copy className="h-3 w-3" />
                            )}
                            <span>
                              {copiedId === msg.id ? "已复制" : "复制"}
                            </span>
                          </button>
                        </div>
                      </>
                    ) : null}
                  </div>
                )
              )}

              {statusHint && (
                <div
                  className={`flex items-center gap-2 text-xs ${
                    isDark ? "text-sky-200/90" : "text-sky-700/90"
                  }`}
                >
                  <span className="inline-block h-2 w-2 rounded-full bg-current animate-pulse-soft" />
                  <span>{statusHint}</span>
                </div>
              )}

              {errorMsg && (
                <div
                  className={`rounded-xl border px-3 py-2 text-xs select-text ${
                    isDark
                      ? "border-rose-400/35 bg-rose-500/20 text-rose-100"
                      : "border-rose-500/30 bg-rose-500/12 text-rose-950"
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <span>{errorMsg}</span>
                    {needsApiSetup && (
                      <button
                        type="button"
                        onClick={() => void openSettings()}
                        className="shrink-0 rounded-md bg-rose-500/30 px-2 py-0.5 font-medium hover:bg-rose-500/40"
                      >
                        去设置
                      </button>
                    )}
                  </div>
                </div>
              )}
            </div>
            <div ref={scrollEndRef} />
          </div>
        )}
      </div>
    </div>
  );
}
