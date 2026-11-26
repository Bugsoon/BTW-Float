import { useCallback, useEffect, useRef, useState } from "react";
import {
  CalendarCheck2,
  Check,
  Copy,
  ExternalLink,
  KeyRound,
  LogIn,
  LogOut,
  RefreshCw,
  Server,
  UserCheck,
  X,
} from "lucide-react";
import {
  wbApplySettings,
  wbCheckinNow,
  wbGetStatus,
  wbLoginCancel,
  wbLoginPoll,
  wbLoginStart,
  wbLogout,
  wbRefreshStatus,
  wbSetAutoCheckin,
  wbSyncModels,
} from "../lib/commands";
import type { WorkBuddyLoginStart, WorkBuddyStatus } from "../types/app";

const SITE_OPTIONS = [
  { id: "cn", label: "国内版 (workbuddy.cn)" },
  { id: "intl", label: "国际版 (codebuddy.ai)" },
  { id: "workbuddy", label: "国际旧域 (workbuddy.ai)" },
] as const;

function formatExpiry(expiresAt: number): string {
  if (!expiresAt) return "未知";
  const diffSecs = expiresAt - Math.floor(Date.now() / 1000);
  if (diffSecs <= 0) return "已过期";
  const days = Math.floor(diffSecs / 86400);
  if (days >= 1) return `剩余 ${days} 天`;
  const hours = Math.max(1, Math.floor(diffSecs / 3600));
  return `剩余 ${hours} 小时`;
}

function accountStatusBadge(status: string) {
  switch (status) {
    case "session_dead":
      return {
        text: "需重登",
        cls: "border-rose-400/30 bg-rose-500/20 text-rose-200",
      };
    case "no_credit":
      return {
        text: "积分不足",
        cls: "border-amber-400/35 bg-amber-500/20 text-amber-200",
      };
    case "rate_limited":
      return {
        text: "限流中",
        cls: "border-amber-400/35 bg-amber-500/20 text-amber-200",
      };
    default:
      return {
        text: "正常可用",
        cls: "border-emerald-400/30 bg-emerald-500/20 text-emerald-200",
      };
  }
}

export function WorkBuddyPanel({
  onToast,
  onConfigChanged,
}: {
  onToast: (msg: string, duration?: number) => void;
  onConfigChanged: () => void;
}) {
  const [status, setStatus] = useState<WorkBuddyStatus | null>(null);
  const [portInput, setPortInput] = useState("18080");
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [saving, setSaving] = useState(false);

  const [site, setSite] = useState<string>("cn");
  const [loginSession, setLoginSession] = useState<WorkBuddyLoginStart | null>(null);
  const [loginHint, setLoginHint] = useState<string | null>(null);
  const [loginLoading, setLoginLoading] = useState(false);

  const [syncing, setSyncing] = useState(false);
  const [checkingIn, setCheckingIn] = useState(false);
  const [refreshingStatus, setRefreshingStatus] = useState(false);
  const [copiedUrl, setCopiedUrl] = useState(false);
  const pollTimerRef = useRef<number | null>(null);

  const reloadStatus = useCallback(async () => {
    const st = await wbGetStatus();
    setStatus(st);
    setPortInput(String(st.port));
    return st;
  }, []);

  useEffect(() => {
    void reloadStatus().then((st) => {
      if (st.account && st.account.creditsRemain === null && !st.account.creditsUnlimited) {
        void wbRefreshStatus()
          .then(setStatus)
          .catch(() => {
            // Silent on initial load; user can manually refresh
          });
      }
    });
    return () => {
      if (pollTimerRef.current) {
        window.clearInterval(pollTimerRef.current);
      }
    };
  }, [reloadStatus]);

  const stopPolling = useCallback(() => {
    if (pollTimerRef.current) {
      window.clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }
  }, []);

  const handleApplyGateway = async (nextEnabled?: boolean) => {
    if (!status) return;
    setSaving(true);
    try {
      const parsedPort = Math.min(65535, Math.max(1024, Number(portInput) || 18080));
      const updated = await wbApplySettings(
        nextEnabled ?? status.enabled,
        parsedPort,
        apiKeyInput
      );
      setStatus(updated);
      setPortInput(String(updated.port));
      onConfigChanged();
      if (updated.lastError) {
        onToast(`内核启动失败: ${updated.lastError}`, 3600);
      } else {
        onToast(updated.running ? `内核已运行于 ${updated.baseUrl}` : "内核已停止");
      }
    } catch (err) {
      onToast(`应用失败: ${String(err)}`, 3000);
    } finally {
      setSaving(false);
    }
  };

  const handleStartLogin = async () => {
    stopPolling();
    setLoginLoading(true);
    setLoginHint(null);
    try {
      const res = await wbLoginStart(site);
      setLoginSession(res);
      setLoginHint("已在浏览器打开授权页，等待登录完成...");

      const stateToken = res.state;
      pollTimerRef.current = window.setInterval(async () => {
        try {
          const poll = await wbLoginPoll(stateToken);
          if (poll.status === "done") {
            stopPolling();
            setLoginSession(null);
            setLoginHint(null);
            await reloadStatus();
            onConfigChanged();
            onToast(
              `登录成功：${poll.nickname || poll.uid}，已自动同步模型`,
              3000
            );
          } else if (poll.status === "fatal" || poll.status === "expired") {
            stopPolling();
            setLoginSession(null);
            setLoginHint(poll.message || "登录未完成");
          } else if (poll.message) {
            setLoginHint(poll.message);
          }
        } catch (err) {
          setLoginHint(`轮询重试中: ${String(err)}`);
        }
      }, 2500);
    } catch (err) {
      setLoginHint(`发起登录失败: ${String(err)}`);
    } finally {
      setLoginLoading(false);
    }
  };

  const handleCancelLogin = async () => {
    stopPolling();
    if (loginSession) {
      await wbLoginCancel(loginSession.state);
    }
    setLoginSession(null);
    setLoginHint(null);
  };

  const handleSyncModels = async () => {
    setSyncing(true);
    try {
      const models = await wbSyncModels(false);
      await reloadStatus();
      onConfigChanged();
      onToast(`已刷新 ${models.length} 个 WorkBuddy 模型`);
    } catch (err) {
      onToast(`刷新失败: ${String(err)}`, 3200);
    } finally {
      setSyncing(false);
    }
  };

  const handleRefreshStatus = async () => {
    setRefreshingStatus(true);
    try {
      const updated = await wbRefreshStatus();
      setStatus(updated);
      onToast("账号状态与积分已刷新");
    } catch (err) {
      onToast(`刷新失败: ${String(err)}`, 3200);
    } finally {
      setRefreshingStatus(false);
    }
  };

  const handleLogout = async () => {
    try {
      const updated = await wbLogout();
      setStatus(updated);
      onConfigChanged();
      onToast("已退出 WorkBuddy 账号");
    } catch (err) {
      onToast(`退出失败: ${String(err)}`, 3000);
    }
  };

  const handleToggleAutoCheckin = async () => {
    if (!status) return;
    try {
      const updated = await wbSetAutoCheckin(!status.autoCheckin);
      setStatus(updated);
      onToast(updated.autoCheckin ? "已开启每日自动签到" : "已关闭每日自动签到");
    } catch (err) {
      onToast(`设置失败: ${String(err)}`, 3000);
    }
  };

  const handleCheckin = async () => {
    setCheckingIn(true);
    try {
      const receipt = await wbCheckinNow();
      await reloadStatus();
      onToast(receipt.message, 3000);
    } catch (err) {
      onToast(`签到失败: ${String(err)}`, 3200);
    } finally {
      setCheckingIn(false);
    }
  };

  const handleCopyBaseUrl = async () => {
    if (!status) return;
    await navigator.clipboard.writeText(status.baseUrl);
    setCopiedUrl(true);
    window.setTimeout(() => setCopiedUrl(false), 1600);
  };

  if (!status) {
    return (
      <div className="py-10 text-center text-xs text-slate-400">
        正在加载 WorkBuddy 内核状态...
      </div>
    );
  }

  const acc = status.account;

  return (
    <div className="space-y-5">
      {/* Section 1: Single Account Login & Status */}
      <div className="space-y-3 rounded-2xl border border-white/12 bg-white/5 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <UserCheck className="h-4 w-4 text-sky-300" />
            <div>
              <div className="text-sm font-medium text-white">WorkBuddy 账号</div>
              <p className="text-[11.5px] text-slate-400">
                单账号直连，OAuth 令牌经 Windows DPAPI 本地加密保存；模型请在「模型与 API」中切换
              </p>
            </div>
          </div>
          {status.cachedModelCount > 0 && (
            <span className="text-[11px] text-slate-400">
              已同步 {status.cachedModelCount} 个模型
            </span>
          )}
        </div>

        {acc ? (() => {
          const badge = accountStatusBadge(acc.status);
          const creditText = acc.creditsUnlimited
            ? "企业版不限量"
            : acc.creditsRemain !== null
            ? acc.creditsTotal && acc.creditsTotal > 0
              ? `剩余积分 ${acc.creditsRemain.toLocaleString()} / ${acc.creditsTotal.toLocaleString()}`
              : `剩余积分 ${acc.creditsRemain.toLocaleString()}`
            : "积分待刷新";
          return (
          <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-xl border border-white/10 bg-black/25 px-3.5 py-3">
            <div className="min-w-0 space-y-1">
              <div className="flex flex-wrap items-center gap-1.5">
                <span
                  className="truncate text-[13.5px] font-medium text-white"
                  title={`UID: ${acc.uid}`}
                >
                  {acc.nickname || acc.uid}
                </span>
                <span className="rounded bg-white/10 px-1.5 py-0.5 font-mono text-[10px] uppercase text-slate-300">
                  {acc.realm}
                </span>
                <span
                  className={`rounded-full border px-2 py-0.5 text-[10px] ${badge.cls}`}
                >
                  {badge.text}
                </span>
                {acc.realm === "cn" && acc.checkedInToday && (
                  <span className="rounded-full border border-emerald-400/30 bg-emerald-500/15 px-2 py-0.5 text-[10px] text-emerald-200">
                    今日已签
                  </span>
                )}
                {acc.realm === "cn" && !acc.checkedInToday && acc.nextCheckinHint && (
                  <span className="rounded-full border border-sky-400/25 bg-sky-500/12 px-2 py-0.5 text-[10px] text-sky-200">
                    {acc.nextCheckinHint}
                  </span>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
                <span
                  className={
                    acc.creditsRemain !== null && !acc.creditsUnlimited && acc.creditsRemain <= 0
                      ? "font-medium text-amber-300"
                      : "text-slate-200"
                  }
                >
                  {creditText}
                </span>
                <span>·</span>
                <span>令牌 {formatExpiry(acc.expiresAt)}</span>
                {acc.lastCheckinMsg && (
                  <>
                    <span>·</span>
                    <span className="text-emerald-200/90">{acc.lastCheckinMsg}</span>
                  </>
                )}
                {acc.lastError && (
                  <>
                    <span>·</span>
                    <span className="text-amber-300">最近错误: {acc.lastError}</span>
                  </>
                )}
              </div>
            </div>

            <div className="flex shrink-0 items-center gap-1.5">
              <button
                type="button"
                disabled={refreshingStatus}
                onClick={() => void handleRefreshStatus()}
                title="查询最新剩余积分与账号状态"
                className="flex items-center gap-1 rounded-lg border border-white/10 bg-white/6 px-2.5 py-1 text-[11px] text-slate-200 hover:bg-white/14 disabled:opacity-50"
              >
                <RefreshCw
                  className={`h-3 w-3 ${refreshingStatus ? "animate-spin" : ""}`}
                />
                <span>刷新状态</span>
              </button>
              <button
                type="button"
                onClick={() => void handleLogout()}
                className="flex items-center gap-1 rounded-lg border border-rose-400/25 bg-rose-500/10 px-2.5 py-1 text-[11px] text-rose-200 hover:bg-rose-500/20"
              >
                <LogOut className="h-3 w-3" />
                <span>退出</span>
              </button>
            </div>
          </div>
          );
        })() : null}

        {/* Login / Re-login action bar */}
        <div className="flex flex-wrap items-center gap-2">
          <select
            value={site}
            onChange={(e) => setSite(e.target.value)}
            disabled={Boolean(loginSession)}
            className="rounded-xl border border-white/12 bg-black/30 px-3 py-1.5 text-xs text-slate-100 outline-none focus:border-sky-400/60"
          >
            {SITE_OPTIONS.map((opt) => (
              <option key={opt.id} value={opt.id} className="bg-[#0f131c]">
                {opt.label}
              </option>
            ))}
          </select>

          {!loginSession ? (
            <button
              type="button"
              disabled={loginLoading}
              onClick={() => void handleStartLogin()}
              className="flex items-center gap-1.5 rounded-xl bg-sky-500 px-3.5 py-1.5 text-xs font-medium text-white transition hover:bg-sky-400 disabled:opacity-50"
            >
              <LogIn className="h-3.5 w-3.5" />
              <span>
                {loginLoading
                  ? "获取授权链接..."
                  : acc
                  ? "重新登录 / 切换账号"
                  : "浏览器一键登录"}
              </span>
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void handleCancelLogin()}
              className="flex items-center gap-1 rounded-xl border border-rose-400/35 bg-rose-500/15 px-3 py-1.5 text-xs text-rose-200 hover:bg-rose-500/25"
            >
              <X className="h-3.5 w-3.5" />
              <span>取消等待</span>
            </button>
          )}
        </div>

        {loginSession && (
          <div className="space-y-2 rounded-xl border border-sky-400/30 bg-sky-500/10 p-3 text-xs">
            <div className="flex items-center justify-between gap-2 text-sky-100">
              <span className="flex items-center gap-1.5">
                <RefreshCw className="h-3.5 w-3.5 animate-spin" />
                {loginHint || "等待浏览器完成授权..."}
              </span>
              <button
                type="button"
                onClick={() => {
                  void navigator.clipboard.writeText(loginSession.authUrl);
                  onToast("已复制授权链接");
                }}
                className="flex items-center gap-1 rounded-md border border-sky-300/30 bg-sky-400/15 px-2 py-0.5 text-[11px] text-sky-100 hover:bg-sky-400/25"
              >
                <ExternalLink className="h-3 w-3" />
                <span>复制授权链接</span>
              </button>
            </div>
            <div className="truncate font-mono text-[11px] text-slate-400 select-text">
              {loginSession.authUrl}
            </div>
          </div>
        )}

        {!loginSession && loginHint && (
          <div className="rounded-xl border border-amber-400/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
            {loginHint}
          </div>
        )}
      </div>

      {/* Section 2: Kernel Gateway & Human-Like Daily Auto Check-in */}
      <div className="space-y-3.5 rounded-2xl border border-white/12 bg-white/5 p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-emerald-400/15 text-emerald-300">
              <Server className="h-4 w-4" />
            </div>
            <div>
              <div className="flex items-center gap-2 text-sm font-medium text-white">
                <span>本地 OpenAI 兼容内核</span>
                <span
                  className={`rounded-full border px-2 py-0.5 text-[10.5px] ${
                    status.running
                      ? "border-emerald-400/35 bg-emerald-500/20 text-emerald-200"
                      : "border-white/15 bg-white/8 text-slate-400"
                  }`}
                >
                  {status.running ? "运行中" : "已停止"}
                </span>
              </div>
              <p className="text-[11.5px] text-slate-400">
                仅监听本机 127.0.0.1，供 BTW-Float 及本机其他工具调用
              </p>
            </div>
          </div>

          <button
            type="button"
            disabled={saving}
            onClick={() => void handleApplyGateway(!status.enabled)}
            className={`rounded-xl px-3.5 py-1.5 text-xs font-medium transition disabled:opacity-50 ${
              status.enabled
                ? "border border-rose-400/35 bg-rose-500/15 text-rose-200 hover:bg-rose-500/25"
                : "bg-emerald-500 text-white hover:bg-emerald-400"
            }`}
          >
            {status.enabled ? "停止内核" : "启动内核"}
          </button>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-white/8 bg-black/25 px-3.5 py-2">
          <div className="min-w-0 text-xs">
            <span className="text-slate-400">本地 OpenAI 接口：</span>
            <code className="ml-1 font-mono text-sky-200 select-text">
              {status.baseUrl}
            </code>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={() => void handleCopyBaseUrl()}
              className="flex items-center gap-1 rounded-lg border border-white/12 bg-white/8 px-2.5 py-1 text-[11px] text-slate-200 hover:bg-white/14"
            >
              {copiedUrl ? (
                <>
                  <Check className="h-3 w-3 text-emerald-300" />
                  <span>已复制</span>
                </>
              ) : (
                <>
                  <Copy className="h-3 w-3" />
                  <span>复制地址</span>
                </>
              )}
            </button>
            <button
              type="button"
              disabled={syncing || !acc}
              onClick={() => void handleSyncModels()}
              className="flex items-center gap-1 rounded-lg border border-sky-400/35 bg-sky-500/15 px-2.5 py-1 text-[11px] text-sky-200 transition hover:bg-sky-500/25 disabled:opacity-45"
            >
              <RefreshCw className={`h-3 w-3 ${syncing ? "animate-spin" : ""}`} />
              <span>{syncing ? "刷新中..." : "刷新模型列表"}</span>
            </button>
          </div>
        </div>

        {status.lastError && (
          <div className="rounded-xl border border-rose-400/30 bg-rose-500/15 px-3 py-2 text-xs text-rose-200 select-text">
            {status.lastError}
          </div>
        )}

        {/* Daily Auto Check-in compact bar */}
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-white/8 bg-black/20 px-3.5 py-2">
          <div className="flex items-center gap-2">
            <CalendarCheck2 className="h-4 w-4 text-emerald-300" />
            <div>
              <div className="text-xs font-medium text-white">
                每日自动签到（拟人策略 · 国内版个人号）
              </div>
              <div className="text-[11px] text-slate-400">
                每日 08:15–10:45 随机择时签到（绝不零点抢签）；晚开机则启动后随机延迟自动补签
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={checkingIn || !acc || acc.realm !== "cn"}
              onClick={() => void handleCheckin()}
              className="flex items-center gap-1 rounded-lg border border-emerald-400/35 bg-emerald-500/15 px-2.5 py-1 text-[11px] text-emerald-200 transition hover:bg-emerald-500/25 disabled:opacity-45"
            >
              <CalendarCheck2 className="h-3 w-3" />
              <span>{checkingIn ? "签到中..." : "立即签到"}</span>
            </button>
            <button
              type="button"
              role="switch"
              aria-checked={status.autoCheckin}
              onClick={() => void handleToggleAutoCheckin()}
              className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors ${
                status.autoCheckin ? "bg-emerald-500" : "bg-white/18 hover:bg-white/25"
              }`}
            >
              <span
                className={`inline-block h-3.5 w-3.5 rounded-full bg-white shadow-sm transition-transform ${
                  status.autoCheckin ? "translate-x-[18px]" : "translate-x-[3px]"
                }`}
              />
            </button>
          </div>
        </div>

        {/* Port & Optional Local API Key */}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <div>
            <label className="mb-1 block text-[11px] text-slate-400">
              监听端口（仅 127.0.0.1）
            </label>
            <input
              type="number"
              min={1024}
              max={65535}
              value={portInput}
              onChange={(e) => setPortInput(e.target.value)}
              className="w-full rounded-lg border border-white/12 bg-black/25 px-2.5 py-1.5 text-xs outline-none focus:border-sky-400/60 select-text"
            />
          </div>
          <div className="sm:col-span-2">
            <label className="mb-1 block text-[11px] text-slate-400">
              本地访问 API Key（可选；留空表示本机免鉴权{status.hasApiKey ? "，当前已设置" : ""}）
            </label>
            <div className="flex gap-2">
              <div className="relative flex-1">
                <KeyRound className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-500" />
                <input
                  type="password"
                  value={apiKeyInput}
                  onChange={(e) => setApiKeyInput(e.target.value)}
                  placeholder={
                    status.hasApiKey
                      ? "输入新 Key 覆盖，或留空后点应用清除"
                      : "留空免 Key（推荐本机自用）"
                  }
                  className="w-full rounded-lg border border-white/12 bg-black/25 py-1.5 pl-8 pr-2.5 text-xs outline-none focus:border-sky-400/60 select-text"
                />
              </div>
              <button
                type="button"
                disabled={saving}
                onClick={() => void handleApplyGateway()}
                className="shrink-0 rounded-lg border border-white/14 bg-white/10 px-3 py-1.5 text-xs text-slate-100 hover:bg-white/18 disabled:opacity-50"
              >
                应用端口/Key
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
