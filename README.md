# DL-voice-typing (语文兔)

[![Build Windows x64](https://github.com/DennyLiSH/DL-voice-typing/actions/workflows/build.yml/badge.svg)](https://github.com/DennyLiSH/DL-voice-typing/actions/workflows/build.yml)

Windows 本地语音输入工具。按住热键说话，松开即自动转写并粘贴到光标位置。基于 Whisper.cpp 本地识别，无需联网，数据不出本机。

## 🖼️ 截图 / Demo

<!-- TODO: 补充以下截图 -->
<!-- - 浮动波形指示器（录音状态） -->
<!-- - 设置界面（6 页侧栏导航） -->
<!-- - 粘贴前确认编辑窗口 -->
<!-- - 系统托盘右键菜单 -->
<!-- - 完整流程 GIF：按住热键 → 说话 → 松开 → 文字粘贴 -->

## ✨ 功能特性

**语音输入**
- 按住热键录音，松开自动转写并粘贴到光标所在应用
- 完全本地 Whisper.cpp 语音识别，无需联网，隐私安全

**性能与模型**
- Vulkan GPU 加速，无兼容显卡时自动降级 CPU 模式
- 8 种内置模型 + 自定义模型（tiny ~40MB → medium 1.5GB，含 Q8 量化版）
- HF-Mirror（国内加速）/ HuggingFace（国际）双镜像下载

**后处理**
- 可选 LLM 后处理：接入大语言模型自动修正同音字和错别字
- 中文标点自动规范化（半角 → 全角）

**交互体验**
- 实时转写模式：录音时滑动窗口边说边出字
- 粘贴前确认：可选弹出编辑窗口，修改后再粘贴
- 系统托盘常驻，支持开机自启

**多语言** — 中文、英文、日文、韩文

## 💾 下载安装

前往 [GitHub Releases](https://github.com/DennyLiSH/DL-voice-typing/releases) 下载最新安装包。

**系统要求：** Windows 10/11 x64，无额外依赖。

> GPU 加速为可选项：有 Vulkan 兼容显卡自动启用，否则使用 CPU 模式。

**模型选择建议：**

| 模型 | 大小 | 适用场景 |
|------|------|---------|
| tiny-q8_0 | ~40MB | 速度优先，简单听写 |
| **base** | **142MB** | **推荐首选，速度与准确率均衡** |
| small / small-q8_0 | 466MB / ~250MB | 更高准确率，转录稍慢 |
| medium / medium-q8_0 | 1.5GB / ~800MB | 最高准确率，需要较强硬件 |

Q8 量化版体积约为原版一半，准确率损失很小，推荐优先选择。

## 🚀 快速开始

1. 从 [GitHub Releases](https://github.com/DennyLiSH/DL-voice-typing/releases) 下载并安装
2. 启动应用 — 自动最小化到系统托盘
3. 右键托盘图标 → **设置**
4. 在「模型」页下载 Whisper 模型（推荐 base，国内用户选 HF-Mirror）
5. 按住 **Right Ctrl**，对麦克风说话
6. 松开热键 — 文字自动粘贴到当前光标位置

## 📖 使用说明

### 热键工作流

按住热键 → 屏幕中央出现浮动波形指示器（随音量跳动）→ 松开热键 → 语音转写为文字并自动粘贴到光标位置。

默认热键为 **Right Ctrl**，可在设置中切换为 Left Ctrl / Alt / Shift / F1-F12 等。切换热键需重启应用生效。

### 系统托盘

| 菜单项 | 功能 |
|-------|------|
| 重置状态 | 强制重置状态机（应用卡住时使用） |
| 设置... | 打开设置窗口 |
| 退出 | 关闭应用 |

### 设置项

设置窗口包含 6 个页面：

- **通用** — 识别语言、开机自启、实时转写开关
- **快捷键** — 语音输入热键选择
- **模型** — Whisper 模型下载/切换、下载源、GPU/CPU 运行模式
- **LLM 纠错** — 启用 LLM 后处理，配置 API 地址、密钥、模型名称
- **数据** — 保存训练数据、粘贴前确认开关
- **帮助** — 快速入门、常见问题、已知限制

### 文件位置

| 文件 | 路径 |
|------|------|
| 配置文件 | `%APPDATA%\dl-voice-typing\config.json` |
| Whisper 模型 | `%APPDATA%\dl-voice-typing\models\` |
| 日志文件 | `%APPDATA%\dl-voice-typing\logs\` |

## ❓ 常见问题

**GPU 模式未生效？**
需要支持 Vulkan 的显卡（NVIDIA / AMD / Intel 近年型号均可）。应用会自动检测并回退到 CPU 模式。GPU 首次加载需编译 shader 缓存，约 10-30 秒。

**文字没有粘贴到目标应用？**
确保目标应用处于焦点状态。以**管理员权限**运行的应用可能无法接收来自普通权限进程的粘贴（Windows 安全限制）。

**首次转写很慢？**
首次使用需加载 Whisper 模型到内存（2-5 秒），后续转写会更快。

**LLM 纠错连接失败？**
检查 API 地址格式是否正确（需包含完整路径如 `/v1/chat/completions`），确认密钥有效，检查网络连通性。

**应用无响应？**
右键托盘图标 → 「重置状态」。状态机看门狗也会在 30 秒后自动恢复。

---

<details>
<summary><strong>🛠️ 开发者指南</strong></summary>

## 🏗️ 技术架构

| 技术 | 用途 |
|------|------|
| Rust (edition 2024, MSRV 1.85) | 后端逻辑，状态机驱动整个管道 |
| Tauri 2 | 桌面框架，系统托盘，多窗口管理 |
| whisper-rs (whisper.cpp) | 本地语音识别，Vulkan GPU / CPU fallback |
| cpal | 音频采集，48kHz → 16kHz 重采样 |
| tokio | 异步运行时，阻塞工作通过 spawn_blocking 卸载 |
| reqwest | HTTP 客户端，LLM API 调用（OpenAI 兼容） |
| windows-rs | Win32 API（剪贴板、热键钩子、光标定位、DPAPI） |
| 纯 HTML/CSS/JS | 前端（无框架），三个 WebView 窗口 |

架构：Rust 后端驱动完整管道（音频采集 → Whisper 转写 → LLM 纠错 → 剪贴板注入），前端仅负责可视化（浮动波形、设置界面、编辑确认）。

## 📐 状态机与流水线

所有行为由 Rust 枚举状态机驱动（`state.rs`）：

```
Idle → Recording → Transcribing → [LLMRefining →] Injecting → Idle
                        ↘ Reviewing ↗
任何状态 → Idle（错误/取消/看门狗重置）
```

**PipelineMode** 由 `realtime_transcription` + `review_before_paste` 两个配置派生：

| 模式 | 实时转写 | 粘贴前确认 | 行为 |
|------|---------|-----------|------|
| ClassicDirect | off | off | 完整 Whisper 转写，直接粘贴 |
| ClassicReview | off | on | 完整 Whisper 转写，编辑确认后粘贴 |
| RealtimeDirect | on | off | 滑动窗口实时出字，直接粘贴（跳过 Whisper） |
| RealtimeReview | on | on | 滑动窗口实时出字，编辑确认后粘贴 |

热键回调流程：`Pressed` → 启动音频采集 + 可选实时转写线程 → `Released` → 停止采集 → 按 PipelineMode 分发 → 异步管道处理。

## 🧠 关键设计决策

1. **枚举派发替代 dyn trait** — `AnyEngine`/`AnyClipboard`/`AnyCorrector` 枚举实现对应 trait，避免 `dyn` + `Box<dyn Future>` 的性能开销。每个枚举都有 `Mock` 变体用于测试。

2. **剪贴板 + Ctrl+V 文本注入** — 写入剪贴板 → 模拟 Ctrl+V → 恢复原剪贴板内容。未使用 SendInput 直接文本输入，因 Windows IME 兼容性不可靠。剪贴板操作有 2 秒超时，防止其他进程持有剪贴板时死锁。

3. **面向测试的 trait 抽象** — `AudioCaptureProvider`、`EventEmitter`、`WindowController`、`ClipboardProvider`、`TextCorrector`、`ReviewProvider` 等 trait 解耦核心逻辑与平台依赖。`PipelineState` 通过构造函数注入所有依赖。

4. **阻塞工作卸载** — Whisper 推理和 LLM HTTP 调用使用 `spawn_blocking` 避免阻塞 tokio 运行时。LLM 同步包装器使用 `block_in_place`。

5. **GPU→CPU 模型加载 fallback** — `WhisperEngine::load_model()` 先尝试 GPU 参数，失败后自动以 `use_gpu: false` 重试。用户无需手动配置。

6. **状态机看门狗** — 后台线程每 10 秒检查状态机（`try_lock` 非阻塞），非 Idle 状态超过 30 秒则强制重置、隐藏窗口、更新托盘提示。配合托盘「重置状态」提供自动+手动两种恢复路径。

7. **静音/幻觉防护** — 音频预处理跳过 RMS < 0.01 的静音段。Whisper 输出中 `no_speech_probability > 0.6` 的段落被丢弃。

8. **中文标点规范化** — 后处理将 CJK 字符之间的 ASCII 逗号/句号替换为全角，同时保留数字间的小数点（如 "3.5"）。

9. **DPAPI API 密钥加密** — LLM API 密钥写入磁盘前使用 Windows DPAPI 加密，以 `DPAPI:` 前缀 + base64 编码存储。旧版明文密钥在下次保存时自动迁移。

10. **实时转写滑动窗口** — 5 秒窗口、500ms 步长。重叠检测通过忽略标点符号比较内容字符实现去重，仅追加新内容。能量检测 VAD（100ms 帧长）过滤静音和噪声后才送入 Whisper。

## 🔨 从源码构建

### 前置条件

- Rust 1.85+ (stable-x86_64-pc-windows-msvc)
- Vulkan SDK 1.4.341.1+
- Visual Studio Build Tools 2022+（C++ 工作负载）
- Node.js（前端测试）

### 构建

```powershell
# 推荐方式：使用构建脚本（自动配置环境变量）
.\scripts\build_vulkan.ps1          # Debug 构建
.\scripts\build_vulkan.ps1 -Release # Release 安装包
.\scripts\build_vulkan.ps1 -Dev     # 开发模式 (cargo tauri dev)
.\scripts\build_vulkan.ps1 -Check   # 仅 cargo check
```

**CARGO_TARGET_DIR 必须指向短路径**（如 `D:\t`）。MSVC 在依赖嵌套路径超过 ~260 字符时报 C1083 错误，whisper-rs-sys 的构建产物很容易超出限制。

**Feature flags：** `default = ["whisper", "devtools"]`。Release 构建通过 `--no-default-features --features whisper` 剥离 DevTools。

## 📁 项目结构

```
src-tauri/src/
├── main.rs                    # 入口
├── lib.rs                     # 应用初始化与组装
├── state.rs                   # 状态机 (Idle/Recording/Transcribing/LLMRefining/Reviewing/Injecting)
├── config.rs                  # 配置读写、PipelineMode、模型枚举、DPAPI 集成
├── error.rs                   # 统一错误类型 (AppError + CommandError)
├── crypto.rs                  # DPAPI 加密/解密
├── tray.rs                    # 系统托盘菜单
├── watchdog.rs                # 状态机看门狗 (10s check, 30s force reset)
├── perf.rs                    # 性能指标采集
├── win32.rs                   # Win32 API (光标定位、显示器工作区、前台窗口)
├── platform.rs                # 平台相关工具
├── util.rs                    # 通用工具 (锁 poisoning 日志)
├── data_saving.rs             # 训练数据保存 (WAV + JSON)
├── realtime.rs                # 实时转写 (滑动窗口、VAD、累积去重)
├── audio/
│   ├── mod.rs                 # AudioCaptureProvider trait, cpal 采集, 重采样
│   └── rms.rs                 # RMS 音量计算
├── speech/
│   ├── mod.rs                 # SpeechEngine trait + AnyEngine 枚举派发
│   ├── whisper.rs             # Whisper.cpp 引擎 (GPU fallback, no_speech filter)
│   └── mock.rs                # 测试用 MockEngine
├── clipboard/
│   └── mod.rs                 # ClipboardProvider trait + AnyClipboard, Win32 剪贴板 + Ctrl+V, 2s timeout
├── hotkey/
│   ├── mod.rs                 # HotkeyManager trait, HotkeyEvent
│   └── windows.rs             # Windows 全局热键 (SetWindowsHookEx)
├── llm/
│   ├── mod.rs                 # TextCorrector trait + AnyCorrector 枚举派发
│   └── prompt.rs              # LLM 提示词模板
└── commands/
    ├── mod.rs                 # EventEmitter trait, 命令注册
    ├── config_cmd.rs          # get_config / save_settings
    ├── download.rs            # 模型下载/删除/取消
    ├── hotkey_pipeline.rs     # 热键回调 → PipelineMode 分发
    ├── pipeline_state.rs      # PipelineState 共享状态聚合
    ├── text_injector.rs       # 文本注入逻辑 (clipboard → paste → restore)
    ├── review.rs              # 粘贴前确认窗口
    ├── review_provider.rs     # ReviewProvider trait
    ├── window_controller.rs   # WindowController trait
    └── misc_cmd.rs            # test_llm / perf_history / compute_mode

ui/                            # 前端 (纯 HTML/CSS/JS, 无框架)
├── floating.html/js/css       # 浮动波形指示器
├── settings.html/js/css       # 设置界面 (6 页侧栏导航)
├── review.html/js/css         # 粘贴前确认编辑窗口
├── common.css                 # 共享样式
└── index.html                 # 入口页面

__tests__/                     # 前端测试 (vitest)
├── floating.test.js           # 波形动画、RMS 映射、事件处理
└── settings.test.js           # 设置读写、脏检查、验证逻辑
```

## 🧪 开发与测试

```bash
# Rust 代码检查
cargo fmt --check                        # 格式检查
cargo clippy --all-targets -- -D warnings # Lint（须与 CI 一致）

# Rust 测试
cargo nextest run                        # 推荐测试运行器
cargo test                               # 标准测试

# 前端测试
npm test                                 # vitest run
npm run test:watch                       # vitest watch

# 开发
.\scripts\build_vulkan.ps1 -Dev          # 开发模式（含 DevTools）
```

测试覆盖：状态机转换、watchdog 行为、实时转写累积重叠逻辑、剪贴板超时、DPAPI 加解密 roundtrip、配置序列化、中文标点规范化、PipelineMode 派发、E2E 管道（MockEngine 覆盖所有状态分支）。

</details>

## 📜 License

TBD
