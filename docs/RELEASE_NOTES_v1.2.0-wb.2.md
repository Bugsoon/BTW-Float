## BTW-Float v1.2.0-wb.2 (WorkBuddy 内置版)

面向 Windows 11 的轻量 `/btw` 风格桌面 AI 悬浮球助手 — **`feat/workbuddy2api-core` 独立并行发行版**。

> 本版本作为 **Pre-release** 独立发布，不会影响 `main` 主线（纯 BYOK 版）与 Scoop 正式版。

### 下载

| 文件 | 说明 |
| --- | --- |
| `BTW-Float_1.2.0-wb.2_x64-setup.exe` | Windows 64 位安装包（NSIS），双击安装，自动创建开始菜单与桌面快捷方式，支持卸载 |

### 本次更新（v1.2.0-wb.2）

- **同步 `main` v1.1.2 后台常驻与单实例体验**
  - 窗口锁定 Win32 `WS_EX_TOOLWINDOW` 样式归入后台进程（隐藏任务栏与 Alt+Tab 残留）、拦截意外关闭防止误退、重复启动自动唤醒已有实例；桌面悬浮球默认显示，并支持在系统托盘与设置中自由开关（托盘同步提供悬浮球与开机自启勾选状态）。
- **系统托盘交互与浅色悬浮球图标**
  - 精简托盘悬停提示为 `BTW-Float`，左键/右键均直接打开菜单；新增 `64×64` 浅色天青磨砂球图标（`tray-light.png`），兼容第三方玻璃任务栏，避免被系统误判为单色图标而变黑。
- **全端统一浅色视觉（含 WorkBuddy 内核面板适配）**
  - 悬浮球、磨砂胶囊、设置窗口、顶部模型选择器及内置 **WorkBuddy 内核** 面板全面统一为浅色视觉，移除深浅色动态切换逻辑。
- **WorkBuddy 密钥安全加固**
  - 统一收敛 `redact_workbuddy_secrets` 脱敏逻辑至托盘快捷切换与所有 `config:updated` 事件广播，确保 OAuth 令牌与本地网关密钥不向前端泄露。
- **内置 WorkBuddy 核心能力（继承自 v1.2.0-wb.1）**
  - 纯 Rust 内嵌单账号网关，强制仅监听本机回环地址 `127.0.0.1:18080`；支持国内/国际版官方 OAuth 登录、按需状态与剩余积分查询、国内版个人号拟人每日自动签到（`08:15–10:45` CST 随机择时 + 晚开机补签），以及设置页顶部一键切换模型。

### 系统要求

- Windows 11（x64）
- WebView2 Runtime（Windows 11 默认自带，通常无需单独安装）
- 可直接登录 WorkBuddy 账号使用，也可继续配置任意自定义 OpenAI 兼容 API Key（BYOK）

### 安装提示

安装包未做代码签名，Windows SmartScreen 可能提示「未知发布者」。点击 **更多信息 → 仍要运行** 即可正常安装。

### 开源协议与致谢

- [MIT](https://github.com/Bugsoon/BTW-Float/blob/main/LICENSE) © Bugsoon
- 内置 WorkBuddy 协议转换与站点适配参考了 [misakano7545/workbuddy2api-panel](https://github.com/misakano7545/workbuddy2api-panel)（MIT License）
