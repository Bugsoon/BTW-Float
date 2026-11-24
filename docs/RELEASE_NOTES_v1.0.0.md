## BTW-Float v1.0.0

面向 Windows 11 的轻量 `/btw` 风格桌面 AI 悬浮球助手。随手呼出、问完即走、关闭自动清空。

### 下载

| 文件 | 说明 |
| --- | --- |
| `BTW-Float_1.0.0_x64-setup.exe` | Windows 64 位安装包（NSIS），双击安装，自动创建开始菜单与桌面快捷方式，支持卸载 |

### 系统要求

- Windows 11（x64）
- WebView2 Runtime（Windows 11 默认自带，通常无需单独安装）
- 需要自备大模型 API Key（BYOK，本项目不含任何内置额度）

### 主要功能

- **悬浮球 + 磨砂胶囊问答**：常驻桌面透明悬浮球，可拖拽、记忆位置、贴边半隐藏，支持拖放文字或图片投喂
- **鼠标侧键智能触发**：划选文字按侧键直接翻译/解释（UIA 读取，含终端与 IDE 防误触）；刚截图后按侧键自动附带截图提问；平时按侧键呼出输入框
- **截屏提问**：`Alt + A` 全屏冻结框选，`Alt + S` 截取当前窗口（高分屏 DPI 精确裁剪）
- **BYOK 多模型**：兼容所有 OpenAI Chat Completions 协议接口（DeepSeek、OpenRouter、硅基流动、本地 Ollama 等），填 Base URL 与 Key 后自动扫描可用模型并识别识图能力
- **免费联网搜索**：内置 Bing + DuckDuckGo 兜底检索与正文提取，无需第三方搜索 API Key，自动跟随 Windows 系统代理
- **纯净输出**：实时剥离 `reasoning_content`、`<think>` 等思考过程，只展示最终答案

### 默认快捷键

| 操作 | 默认按键 |
| --- | --- |
| 呼出 / 划词即问 / 带图提问 | 鼠标前 / 后侧键 |
| 打开 / 收起输入框 | `Alt + Space` |
| 框选屏幕区域提问 | `Alt + A` |
| 截取当前窗口提问 | `Alt + S` |
| 收起并清空当前会话 | `Esc` |

全部快捷键与鼠标侧键均可在设置页自定义。

### 隐私与安全

- **API Key 本地加密**：配置仅保存在本机 `%APPDATA%\BTW-Float\config.json`，所有 Key 通过 Windows DPAPI（`CryptProtectData`）按当前 Windows 用户加密存储
- **直连接口**：所有请求直接发往你配置的模型接口，无中转、无遥测
- **对话不留存**：内容仅保留在内存中，关闭胶囊即清空

### 安装提示

安装包未做代码签名，Windows SmartScreen 可能提示「未知发布者」。点击 **更多信息 → 仍要运行** 即可正常安装。

### 已知限制

- 仅支持 Windows 11（x64），暂不支持 macOS / Linux
- 鼠标侧键与全局快捷键依赖 Win32 低级钩子，部分带反作弊的全屏游戏内可能不生效（已内置自动暂停）
- 需自行准备可用的模型接口与 API Key

### 开源协议

[MIT](https://github.com/Bugsoon/BTW-Float/blob/main/LICENSE) © Bugsoon
