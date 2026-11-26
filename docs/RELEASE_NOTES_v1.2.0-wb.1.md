## BTW-Float v1.2.0-wb.1 (WorkBuddy 内置版)

面向 Windows 11 的轻量 `/btw` 风格桌面 AI 悬浮球助手 — **`feat/workbuddy2api-core` 独立并行发行版**。

> 本版本作为 **Pre-release** 独立发布，不会影响 `main` 主线（纯 BYOK 版）与 Scoop 正式版。

### 下载

| 文件 | 说明 |
| --- | --- |
| `BTW-Float_1.2.0-wb.1_x64-setup.exe` | Windows 64 位安装包（NSIS），双击安装，自动创建开始菜单与桌面快捷方式，支持卸载 |

### 本次更新（v1.2.0-wb.1）

- **内置纯 Rust 极简 WorkBuddy 内核（零配置开箱即用）**
  - 内嵌单账号 WorkBuddy 协议转换核心，仅绑定本机回环地址 `127.0.0.1:18080` 提供 OpenAI 兼容接口（`/v1/models`、`/v1/chat/completions`），无需额外运行代理服务。
  - 在设置页 **WorkBuddy 内核** 中支持国内版（`workbuddy.cn`）与国际版（`codebuddy.ai` / `workbuddy.ai`）官方 OAuth 登录，自动同步可用模型（默认优先选用 `DeepSeek-V4.1-Flash`）。
  - 单账号卡片直接展示当前状态（`正常可用` / `积分不足` / `限流中` / `需重登`）、剩余积分与令牌有效期，采用按需查询、无后台轮询。
  - 国内版个人号内置**拟人每日自动签到**（默认开启，每天在 `08:15–10:45` CST 随机时段自然触发并支持晚开机补签）。
  - 所有 OAuth 令牌与可选网关密钥均经 Windows DPAPI 本机加密存储，不向前端暴露明文。
- **统一模型切换体验**
  - 在 **设置 → 模型与 API** 顶部新增「当前对话使用的模型」选择器，支持在内置 WorkBuddy 模型与自定义 BYOK 接口之间一键切换，同时保持悬浮胶囊输入框极简干净。

### 系统要求

- Windows 11（x64）
- WebView2 Runtime（Windows 11 默认自带，通常无需单独安装）
- 可直接登录 WorkBuddy 账号使用，也可继续配置任意自定义 OpenAI 兼容 API Key（BYOK）

### 安装提示

安装包未做代码签名，Windows SmartScreen 可能提示「未知发布者」。点击 **更多信息 → 仍要运行** 即可正常安装。

### 开源协议与致谢

- [MIT](https://github.com/Bugsoon/BTW-Float/blob/main/LICENSE) © Bugsoon
- 内置 WorkBuddy 协议转换与站点适配参考了 [misakano7545/workbuddy2api-panel](https://github.com/misakano7545/workbuddy2api-panel)（MIT License）
