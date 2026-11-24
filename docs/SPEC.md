# BTW-Float 技术规格与架构文档（SPEC）

> 本文档记录 **BTW-Float** 当前已落地实现的架构设计、交互行为与 Windows 11 原生实现细节。

---

## 1. 产品定位与设计原则

- **定位**：面向 Windows 11 的轻量、无打扰、`/btw` 风格桌面 AI 悬浮球助手。
- **核心理念**：
  - **随问随走**：打开期间支持连续追问；关闭或失焦隐藏后自动清空会话，不维护沉重历史库。
  - **极简界面**：主胶囊输入框仅保留拖动图标、输入区、联网开关、`⋯`（直达设置）与按需出现的发送/停止按钮；参考网站来源默认隐藏，可按需在设置中开启。
  - **零打扰优先**：全屏游戏/视频自动暂停鼠标侧键；双侧键模式下浏览器未选字时自动保留「后退」功能；支持自定义例外软件禁用侧键。
  - **模型中立（BYOK）**：兼容 OpenAI Chat Completions 协议，自动扫描可用模型，API Key 经 Windows DPAPI 本机加密保存。

---

## 2. 四窗口轻量架构

所有前端页面共用同一套 Vite 产物（`index.html`），在 [src/main.tsx](../src/main.tsx) 中通过 `getCurrentWindow().label` 分发路由：

| 窗口 Label | 生命周期 | 尺寸（逻辑 px） | 职责与特性 |
| --- | --- | --- | --- |
| `ball` | 常驻 | `86×86`（内置 `52×52` 圆形球 + 阴影留白） | 透明无边框、置顶、`skipTaskbar`、`focusable: false`；通过 Win32 子类化实现无焦点点击、原生拖拽与文件拖放（`WM_DROPFILES`）。 |
| `popup` | 预热隐藏 | `584×100` 起（随内容动态伸缩至最高 `610`） | 透明无边框磨砂胶囊；按悬浮球位置锚定弹出（悬浮球隐藏时退回光标/屏幕上方）；支持按住拖动、连续追问、`Ctrl+V` 贴图、拖放文本/图片。 |
| `capture` | 按需创建，用完销毁 | 覆盖当前显示器全屏 | 独立线程创建，展示冻结底图并按 `[0..1]` 归一化比例裁剪，避免多屏/高分屏 DPI 偏差。 |
| `settings` | 按需创建，关闭销毁 | 标准设置窗口 | 独立线程创建避免阻塞主消息循环；包含「模型与 API」「快捷键与侧键」「悬浮球与交互」三个分组，开关与快捷键改动即刻自动生效。 |

> **渲染说明**：透明无边框窗口（`ball`、`popup`）在 Windows 11 WebView2 上采用纯透明 DirectComposition 合成 + CSS 磨砂渐变与 3σ 阴影留白，不使用 `SetWindowRgn` 或 `SetWindowCompositionAttribute`，避免圆角裁切异常、黑框或拖动掉帧。

---

## 3. 核心机制实现

### 3.1 悬浮球原生交互（`win_native.rs`）
- 启动时对 `ball` 的 WebView2 子窗口禁用鼠标命中（`EnableWindow(child, false)`），并对顶层 HWND 安装 `ball_wnd_proc` 子类化过程：
  - `WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE`，点击悬浮球不抢占前台焦点，从而不影响对前台窗口划词内容的读取，同时避免 WebView2 冷启动吞掉首次点击。
  - 左键按下移动超过阈值时调用原生 `SetWindowPos` 流畅拖拽，松开后持久化坐标并检测屏幕左右边缘触发贴边半隐藏。
  - 启用 `DragAcceptFiles` 处理 `WM_DROPFILES`，支持直接把图片或文件拖到悬浮球上打开胶囊提问。

### 3.2 鼠标侧键与全局快捷键（`mouse_hook.rs` / `hotkey.rs`）
- 独立线程运行 Win32 低级鼠标钩子 `WH_MOUSE_LL`：
  - 支持 `XButton1/XButton2`（双侧键智能触发）、`XButton2`（仅前侧键）、`XButton1`（仅后侧键）及修饰键组合（如 `Alt+XButton1`）。
  - 被动记录左键拖拽或双击产生的选词手势时间戳（`LAST_SELECT_GESTURE_MS`）。
  - 优先级判定：全屏应用（可开关）与用户自定义的例外进程直接放行；双侧键模式下，当浏览器/资源管理器处于前台且无选词/新鲜截图时，自动放行后侧键（`XButton1`）保留原生「后退」。
  - 在 `WM_XBUTTONDOWN` 阶段标记挂起动作，在 `WM_XBUTTONUP` 阶段派发，确保后续 UIA / 剪贴板操作时鼠标按键已释放。
- 键盘快捷键通过 `tauri-plugin-global-shortcut` 注册，默认：`Alt+Space`（呼出/收起）、`Alt+A`（框选截屏）、`Alt+S`（当前窗口截屏）。

### 3.3 划词读取与新鲜截图感知（`selection.rs` / `clipboard_watch.rs`）
- **划词读取（优先级 1）**：
  1. 优先调用 Windows UI Automation（`IUIAutomationTextPattern`），在焦点元素及最多 3 层祖先节点上无感读取选中文本。
  2. 对纯终端进程（`WindowsTerminal`、`pwsh`、`cmd` 等）及 UIA 识别到的终端控件严禁发送 `Ctrl+C`。
  3. 对集成终端的 IDE（如 VS Code、Cursor、T3 Code、JetBrains），仅在刚检测到明确鼠标选词手势时允许执行分步 `Ctrl+C`（备份并恢复原剪贴板），彻底避免向空闲终端误发 `SIGINT`。
  4. 默认开启 `autoSubmitSelection`：捕获选中文字后按语种/代码特征自动组装 Prompt 并立即请求 AI 解答。
- **剪贴板新鲜截图（优先级 2）**：
  - 独立隐藏消息窗口通过 `AddClipboardFormatListener` 静默监听 `WM_CLIPBOARDUPDATE`，记录新图片进入剪贴板的时间戳。
  - 在新鲜度窗口内（默认 90 秒）呼出胶囊时自动解析 `CF_DIB` 并压缩为 JPEG Data URL 挂载为附件，消费一次后即标记已用，避免反复误带旧图。

### 3.4 AI 对话、模型扫描与免费 Agent 搜索（`llm.rs` / `search.rs`）
- **模型自动发现**：请求 `{baseUrl}/models`（自动回退 `/v1/models` 与 Ollama `/api/tags`），自动修正缺失的 `/v1` 后缀，过滤非对话模型并推断 Vision / Tool Calling 能力。
- **纯净流式输出**：`ThinkFilter` 在流式 SSE 分片边界实时剥离 `<think>...</think>`、`<thought>...</thought>`、`<tool_call>...</tool_call>` 及前导空白，同时忽略 `reasoning_content` / `reasoning` 字段的展示（但在多轮工具调用时完整回传给上游模型）。
- **免费 Agent 联网搜索**：
  - 自动读取 Windows WinINet 系统代理配置。
  - 通过标准 OpenAI Function Calling 注入 `web_search`（Bing HTML + DuckDuckGo HTML 自动兜底）与 `fetch_url`（网页正文提取与截断）。
  - 兼容流式 SSE 与部分中转站的非流式 JSON / `choice.message` 响应；若多轮工具调用后上游未输出有效正文，自动基于已收集的检索摘要触发一次无工具的纯净流式汇总请求，确保必有完整回答。

### 3.5 配置存储与安全（`config.rs`）
- 配置文件路径：`%APPDATA%\BTW-Float\config.json`。
- 保存时对所有非空 `apiKey` 使用 Windows DPAPI（`CryptProtectData` + `CRYPTPROTECT_UI_FORBIDDEN`）加密并以 `dpapi:` 前缀落盘，读取时自动解密。
- 开机自启通过写入/移除 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\BTW-Float` 实现。
