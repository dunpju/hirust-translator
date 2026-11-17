# Hirust Translator · 中英实时同传翻译机

> 说中文 → 实时识别 → 翻译成英文 → 自动语音播报。美式/英式口音一键切换，默认美式。纯 Rust 实现，语音识别全程本地运行。

## 功能特性

- 🎙 **实时录音**：自动断句（VAD），说完停顿即出译文
- 🗣 **本地识别**：whisper.cpp 中文语音识别，无需上传音频，隐私安全
- 🔁 **双翻译引擎**：LLM API 高质量翻译（可配 DeepSeek 等 OpenAI 兼容接口）；无 API Key 时自动使用 Whisper 内置直译，零配置可用
- 🔊 **自动播报**：译文自动朗读（Edge-TTS 音色），支持静音开关
- 🇺🇸🇬🇧 **口音切换**：美式（默认）/ 英式，同时影响 TTS 音色与译文用词拼写
- 🛡 **回声防护**：外放时播报期间自动暂停拾音，避免"自翻自"
- 📜 **历史记录**：重播 / 复制 / 导出

## 文档索引

| 文档 | 内容 |
|---|---|
| [docs/01-requirements.md](docs/01-requirements.md) | 需求文档：场景、功能清单（P0/P1/P2）、非功能需求、验收标准 |
| [docs/02-architecture.md](docs/02-architecture.md) | 架构设计：技术选型、数据流水线、线程模型、配置设计、风险对策 |
| [docs/03-ui-design.md](docs/03-ui-design.md) | UI 设计：布局线框图、组件层级树、状态流转线性图、视觉规范 |

## 技术栈

| 层 | 选型 |
|---|---|
| GUI | `eframe/egui`（纯 Rust、单二进制、适合实时刷新） |
| 音频采集 | `cpal`（Windows WASAPI） |
| 语音识别 | `whisper-rs`（whisper.cpp 绑定，本地 ggml 模型） |
| 翻译 | `reqwest` + OpenAI 兼容 Chat API / Whisper 直译 |
| 语音合成 | `msedge-tts`（en-US / en-GB 神经音色） |
| 播放 | `rodio` |
| 运行时 | `tokio`（网络与任务编排） |

## 环境要求（Windows 优先）

- Rust ≥ 1.85（本机已核查：rustc 1.97.1 ✅）
- CMake ≥ 3.20（本机已核查：3.28.1 ✅，whisper.cpp 构建需要）
- MSVC Build Tools（C++ 编译器，whisper.cpp 构建需要）
- 首次运行自动下载 whisper 模型（base 约 142 MB，走 hf-mirror 加速）

## 快速开始

```bash
cargo build --release
./target/release/hirust-translator.exe                      # 图形界面（默认，首次自动下载模型）
./target/release/hirust-translator.exe cli say "Hello"      # TTS 试音（headless）
./target/release/hirust-translator.exe cli devices          # 查看输入设备
```

**GUI 快捷键**：Space 开始/暂停 · 1/2 切美式/英式 · M 静音 · E 回声防护 · R 重播上句 · Ctrl+, 设置

## CI / 构建 / 发布（GitHub Actions）

| 工作流 | 触发 | 内容 |
|---|---|---|
| [ci.yml](.github/workflows/ci.yml) | push 到 master / PR / 手动 | Windows x64 构建 + 单元测试 + 冒烟测试，产物上传 artifact |
| [release.yml](.github/workflows/release.yml) | 推送 `v*` 标签 | Release 构建 + 打包（exe + 默认配置 + 文档）→ 自动创建 GitHub Release |

发布新版本：

```bash
git tag v0.1.0
git push origin v0.1.0     # 触发发布流水线
```

## 路线图

- [x] M1 文档定稿（需求 / 架构 / UI）
- [x] M2 核心引擎打通：录音 → VAD → 识别 → 翻译 → TTS（headless CLI）
- [x] M2.5 GitHub Actions CI + 自动发布
- [ ] M3 GUI 集成（egui 界面 + 状态机）
- [ ] M4 v0.1 发布：设置持久化、模型管理、历史导出

## License

MIT
