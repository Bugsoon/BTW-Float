# BTW-Float

![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)
![Platform: Windows 11](https://img.shields.io/badge/Platform-Windows%2011-0078D6.svg)
![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB.svg)

> 面向 Windows 11 的轻量 `/btw` 风格桌面 AI 悬浮球助手。随手呼出、问完即走、关闭自动清空。

基于 **Tauri v2 (Rust) + React 19 + TypeScript + Tailwind CSS v4** 构建，原生接入 Windows 11 Win32 API（低级鼠标钩子、UI Automation 划词读取、剪贴板监听、高分屏截屏与 DPAPI 本地加密）。

---

## 核心特性

- **极简悬浮球 & 磨砂胶囊问答**
  - 常驻桌面透明圆形悬浮球，支持自由拖拽并记忆位置、贴边自动半隐藏、直接拖放文字或图片投喂。
  - 点击悬浮球、按鼠标侧键或快捷键时，胶囊输入框在悬浮球旁秒开；按住输入框左侧图标或空白处可自由拖动；支持连续追问，关闭或失焦后自动清空本次会话。
- **鼠标侧键智能触发（零打扰）**
  - **划选文字 + 按侧键**：优先通过 Windows UIA 读取选中内容（带终端/IDE 防误触保护），直接自动发给 AI 翻译或解释（可在设置中切换为「先填入输入框等你提问」）。
  - **刚截完图 + 按侧键**：自动附带最近 90 秒内剪贴板的新鲜截图（单次消费不重复挂旧图），弹出输入框等你提问。
  - **平时直接按侧键**：在悬浮球旁随时打开 / 收起输入框；默认双侧键模式下逛网页不划词时自动保留浏览器「后退」功能，全屏游戏/视频自动暂停侧键，支持自定义例外软件。
- **看屏幕截屏提问**
  - `Alt + A`：全屏冻结框选任意区域提问（高分屏 DPI 精确裁剪）。
  - `Alt + S`：一键截取当前活动窗口提问。
  - 截屏前自动隐藏自身窗口，避免把悬浮球拍进截图。
- **BYOK 多模型 & 自动扫描**
  - 兼容所有 OpenAI Chat Completions 协议接口（DeepSeek、OpenRouter、硅基流动、各类中转站、本地 Ollama 等）。
  - 填写 Base URL 与 API Key 后自动扫描 `/models`（兼容 `/v1/models` 与 Ollama `/api/tags`），支持搜索勾选模型并自动识别识图能力。
  - API Key 使用 **Windows DPAPI** 在本机加密存储（`%APPDATA%\BTW-Float\config.json`），不上传任何云端服务。
- **免费 Agent 联网搜索 & 纯净输出**
  - 无需第三方搜索 API Key，内置 Bing + DuckDuckGo HTML 自动兜底检索与网页正文读取，自动识别 Windows 系统代理。
  - 实时剥离模型思考过程（`reasoning_content`、`<think>`、`<thought>`），只展示最终答案；参考网站来源默认隐藏，可在设置中自由开关。

---

## 默认快捷键

所有快捷键与鼠标侧键均可在设置页自定义，点按即刻自动生效：

| 操作 | 默认按键 | 说明 |
| --- | --- | --- |
| 鼠标侧键呼出 / 划词即问 / 带图提问 | 前/后侧键智能双键 | 可切换为仅前侧键、仅后侧键、组合侧键或关闭 |
| 打开 / 收起 AI 输入框 | `Alt + Space` | 随时在悬浮球旁呼出或收起 |
| 框选屏幕区域提问 | `Alt + A` | 拖动框选屏幕任意区域发给 AI |
| 截取当前窗口提问 | `Alt + S` | 截取当前活动软件窗口发给 AI |
| 收起并清空当前会话 | `Esc` | 未关闭「点击外部自动收起」时，点窗口外同样会收起并清空 |

---

## 快速开始

### 直接安装（普通用户）

前往 **[Releases](https://github.com/Bugsoon/BTW-Float/releases/latest)** 下载 `BTW-Float_1.0.0_x64-setup.exe`，双击安装即可，无需安装 Node.js 或 Rust。

> 安装包未做代码签名，Windows SmartScreen 可能提示「未知发布者」。点击 **更多信息 → 仍要运行** 即可。

### 环境要求

- **操作系统**：Windows 11（需已安装 WebView2 Runtime，Win11 默认自带）
- **Node.js**：18+ 与 `pnpm`
- **Rust**：最新稳定版（含 `cargo` 与 MSVC 构建工具链）

### 安装与开发运行

```powershell
# 1. 安装前端依赖
pnpm install

# 2. 启动开发模式（Vite + Tauri 热重载）
pnpm tauri dev
```

### 打包发布（NSIS 安装包）

```powershell
pnpm tauri build
```

构建完成后，安装包位于 `src-tauri/target/release/bundle/nsis/`。

### 运行校验与单元测试

```powershell
# 前端 TypeScript 类型检查 + 生产构建
pnpm build

# Rust 后端单元测试
cd src-tauri
cargo test
```

---

## 目录结构

```text
BTW-Float/
├── README.md                  # 项目说明与使用指南
├── docs/
│   └── SPEC.md                # 技术架构与功能规格文档
├── package.json               # 前端依赖与脚本
├── src/                       # React + TypeScript 前端
│   ├── main.tsx               # 多窗口路由入口（ball / popup / capture / settings）
│   ├── index.css              # Tailwind v4 与磨砂胶囊视觉样式
│   ├── windows/
│   │   ├── BallWindow.tsx     # 桌面悬浮球窗口
│   │   ├── PopupWindow.tsx    # 极简胶囊问答窗口
│   │   ├── CaptureWindow.tsx  # 区域框选截屏窗口
│   │   └── SettingsWindow.tsx # 设置窗口（模型 / 快捷键与侧键 / 悬浮球与交互）
│   ├── components/
│   │   ├── MarkdownView.tsx   # Markdown 渲染与代码块一键复制
│   │   └── SourceList.tsx     # 可选联网参考来源展示
│   ├── lib/
│   │   ├── commands.ts        # Tauri IPC 命令封装
│   │   ├── suggestions.ts     # 0ms 本地联想词、划词 Prompt 与图片压缩
│   │   └── theme.ts           # 跟随系统浅色/深色主题
│   └── types/app.ts           # 前后端共享类型定义
└── src-tauri/                 # Rust + Win32 原生后端
    ├── Cargo.toml             # Rust 依赖与 Release 体积优化配置
    ├── tauri.conf.json        # Tauri v2 窗口与打包配置
    └── src/
        ├── lib.rs             # 应用入口与 Tauri Commands 注册
        ├── config.rs          # 配置读写与 Windows DPAPI 加密
        ├── window_mgr.rs      # 四窗口生命周期、定位与动态缩放
        ├── win_native.rs      # Win32 子类化、无焦点悬浮球、系统代理、开机自启
        ├── mouse_hook.rs      # WH_MOUSE_LL 低级鼠标钩子与智能防冲突
        ├── selection.rs       # UIA 划词读取 + 终端安全 Ctrl+C 回退
        ├── clipboard_watch.rs # AddClipboardFormatListener 新鲜截图监听
        ├── capture.rs         # 高分屏 GDI 截屏、DPI 归一化裁剪与压缩
        ├── llm.rs             # OpenAI 流式 SSE、思考过滤、Agent 工具循环与模型扫描
        ├── search.rs          # 零 Key 免费联网搜索（Bing + DuckDuckGo）与正文提取
        ├── hotkey.rs          # 全局快捷键注册与分发
        └── tray.rs            # 系统托盘与悬浮球右键菜单
```

---

## 隐私与安全

- **本地配置与加密**：配置文件仅保存在本机 `%APPDATA%\BTW-Float\config.json`，其中所有 API Key 均通过 Windows DPAPI（`CryptProtectData`）按当前 Windows 用户加密存储。
- **直连接口**：所有问答请求均直接发往你配置的模型接口，无任何中转遥测或数据收集；对话内容仅保留在内存中，关闭胶囊即清空。

---

## 更多文档

- 架构设计与技术细节请参阅 [docs/SPEC.md](./docs/SPEC.md)。

---

## 开源协议

本项目采用 [MIT License](./LICENSE) 开源发布。

```
MIT License

Copyright (c) 2026 Bugsoon
```

你可以自由地使用、复制、修改、合并、发布、再授权和/或销售本软件的副本，包括用于商业目的，唯一要求是保留上述版权声明和本许可声明。本软件按「原样」提供，不提供任何形式的担保。
