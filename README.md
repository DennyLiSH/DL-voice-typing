# DL-voice-typing (语文兔)

[![Build Windows x64](https://github.com/DennyLiSH/DL-voice-typing/actions/workflows/build.yml/badge.svg)](https://github.com/DennyLiSH/DL-voice-typing/actions/workflows/build.yml)

- Windows 本地语音输入工具。
- 按住热键说话，松开即自动转写并粘贴到光标位置。
- 基于 Whisper.cpp 本地识别，无需联网，隐私安全。

## 🖼️ 截图

<!-- TODO: 添加应用截图 -->

## ✨ 功能特性

- **即说即输** — 按住热键录音，松开自动转写并粘贴到光标所在应用
- **完全本地** — Whisper.cpp 本地语音识别，无需联网，数据不出本机
- **GPU 加速** — Vulkan GPU 加速，无兼容显卡时自动降级 CPU 模式
- **多语言** — 中文、英文、日文、韩文
- **可选 LLM 后处理** — 接入大语言模型自动修正同音字和错别字
- **粘贴前确认** — 可选弹出编辑窗口，确认后再粘贴
- **多模型可选** — tiny (75MB)、base (142MB)、small (466MB)、medium (1.5GB)
- **双镜像下载** — HF-Mirror (国内加速) / HuggingFace (国际)
- **系统托盘** — 后台常驻，支持开机自启

## 💾 下载安装

前往 [GitHub Releases](https://github.com/DennyLiSH/DL-voice-typing/releases) 下载最新安装包。

**系统要求：** Windows 10/11 x64，无额外依赖。

> GPU 加速为可选项：有 Vulkan 兼容显卡的用户自动启用，否则使用 CPU 模式。

首次启动后在设置中下载 Whisper 模型即可使用。

## 🔨 从源码构建

### 前置条件

- [Rust](https://www.rust-lang.org/tools/install) 1.85+ (stable-x86_64-pc-windows-msvc)
- [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) 1.4.341.1+
- Visual Studio Build Tools 2022+ (C++ 工作负载)

### 构建步骤

```powershell
# 设置环境变量（MSVC 长路径限制的规避方案）
$env:VULKAN_SDK = "C:\VulkanSDK\1.4.341.1"
$env:PATH = "$env:VULKAN_SDK\Bin;" + $env:PATH
$env:CARGO_TARGET_DIR = "C:\tmp"

# 构建
cd src-tauri
cargo build

# 或使用构建脚本（自动配置环境）
.\_Project\build_vulkan.ps1          # Debug
.\_Project\build_vulkan.ps1 -Release # Release (生成 NSIS 安装包)
.\_Project\build_vulkan.ps1 -Dev     # 开发模式
```

## 🏗️ 技术栈

| 技术 | 用途 |
|------|------|
| Rust | 后端 |
| Tauri 2 | 应用框架 |
| Whisper.cpp (whisper-rs) | 本地语音识别 |
| cpal | 音频采集 |
| Vulkan | GPU 加速 |
| reqwest | LLM API 调用 |

## 📁 项目结构

```
src-tauri/
├── src/
│   ├── main.rs            # 入口
│   ├── lib.rs             # 应用初始化
│   ├── state.rs           # 状态机 (Idle → Recording → Transcribing → Injecting)
│   ├── config.rs          # 配置读写
│   ├── audio/             # 音频采集 + RMS 波形
│   ├── speech/            # SpeechEngine trait + Whisper 实现
│   ├── clipboard/         # 剪贴板保存/恢复 + Ctrl+V 模拟
│   ├── hotkey/            # 全局键盘钩子 (SetWindowsHookEx)
│   ├── llm/               # LLM 客户端 + 提示词
│   ├── tray.rs            # 系统托盘
│   ├── data_saving.rs     # 训练数据收集
│   └── error.rs           # 统一错误类型
└── Cargo.toml

ui/                        # 前端 (HTML/CSS/JS, 无框架)
├── floating.html/js       # 浮动波形窗口
├── settings.html/js       # 设置界面
└── review.html/js         # 粘贴前确认窗口
```

## 🔧 配置

配置文件位于 `%APPDATA%\dl-voice-typing\config.json`，也可在设置界面中修改。

| 配置项 | 默认值 | 说明 |
|-------|--------|------|
| `hotkey` | `RightCtrl` | 录音热键 |
| `language` | `zh` | 识别语言 (zh/en/ja/ko) |
| `whisper_model` | `base` | 模型大小 (tiny/base/small/medium) |
| `llm_enabled` | `false` | 是否启用 LLM 后处理 |
| `review_before_paste` | `false` | 粘贴前弹出确认窗口 |
| `download_mirror` | `hf-mirror` | 模型下载镜像 (hf-mirror/huggingface) |

## 🛠️ 开发

```bash
cargo fmt                  # 格式化
cargo clippy -- -D warnings # Lint 检查
cargo test                 # 运行测试
cargo nextest run          # 运行测试 (更快)
```

## 📜 License

TBD
