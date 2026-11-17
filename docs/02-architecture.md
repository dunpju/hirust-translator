# 架构设计 · Hirust 中英实时同传翻译机

| 项 | 值 |
|---|---|
| 版本 | v1.0 |
| 日期 | 2026-11-17 |
| 状态 | 定稿（M1） |
| 关联文档 | [需求文档](01-requirements.md) · [UI 设计](03-ui-design.md) |

## 1. 总体架构（数据流水线）

```
 ┌────────┐  f32/48k  ┌──────────┐  f32/16k  ┌───────────┐  语音段  ┌─────────────┐
 │ 麦克风  │──cpal采集──▶│ 重采样器 │──────────▶│ VAD 分段器 │─────────▶│ Whisper ASR │
 └────────┘  单声道     │ 48k→16k  │           │ (断句)     │          │  (本地推理)  │
                        └──────────┘           └───────────┘          └──────┬──────┘
                                                                      中文文本│
                     ┌────────────────────────────────────────────────────────▼───────┐
                     │                     翻译层（双引擎，可回退）                     │
                     │  ① LLM API（OpenAI 兼容，DeepSeek 等）  ── 失败/无Key ──▶ 回退  │
                     │  ② Whisper translate 直译（离线）                              │
                     └───────────────────────────────┬───────────────────────────────┘
                                                     英文文本│
 ┌────────┐   mp3    ┌──────────────┐   合成文本   ┌─────────▼──┐
 │ 扬声器  │◀──rodio──│  TTS 播放队列 │◀──串行队列──│  界面/记录  │
 └────────┘   播放    │ (Edge-TTS)   │             └────────────┘
                       └──────┬───────┘
                              │ tts_playing 标志
                              ▼
                    回声防护：播报期间 VAD 丢弃输入
```

## 2. 技术选型

| 层 | 选型 | 版本基线 | 理由 / 备注 |
|---|---|---|---|
| GUI | `eframe` + `egui` | 0.3x | 纯 Rust、单二进制、即时模式天然适合 60fps 状态刷新（电平表） |
| 音频采集 | `cpal` | 0.15 | Rust 标准跨平台音频库，Windows 走 WASAPI |
| 重采样 | 自研线性插值 | — | 48k→16k 只需下采样，线性插值精度足够 whisper，零额外依赖 |
| 语音识别 | `whisper-rs` | 0.14+ | whisper.cpp 绑定；构建需 CMake ✅ + MSVC（本机 CMake 3.28.1 已核查） |
| 翻译(LLM) | `reqwest` | 0.12 | OpenAI 兼容 /chat/completions；rustls-tls |
| 语音合成 | `msedge-tts` | 0.2x | Edge 神经音色：en-US-Aria/Guy/Jenny、en-GB-Sonia/Ryan；已处理微软鉴权令牌 |
| 播放 | `rodio` | 0.20 | 解码 mp3 + 输出设备管理 |
| 异步运行时 | `tokio` | 1.x | TTS/LLM/下载等网络任务；音频与 whisper 走专用线程 + spawn_blocking |
| 序列化 | `serde` + `toml` | — | config.toml 配置持久化 |
| 错误处理 | `anyhow` | — | 应用层统一错误 |
| 下载进度 | `indicatif` | — | 模型下载进度条（headless 用）；GUI 内自绘进度 |

## 3. 线程与数据流模型

```
[cpal 音频回调线程]                     (实时, 不可阻塞)
   │ std::sync::mpsc::Sender<Vec<f32>> (try_send, 满则丢弃)
   ▼
[分段线程]  spawn_blocking 循环         (重采样 + VAD + 断句, 独占)
   │ tokio::sync::mpsc::UnboundedSender<Vec<f32>>   // 完整语音段 16k f32
   ▼
[识别任务]  spawn_blocking              (whisper 推理, Mutex<WhisperContext>)
   │ SegmentResult { zh_text 或 en_text }
   ▼
[翻译任务]  async                       (LLM API 或直译回退; accent 注入提示词)
   │ Utterance { ts, zh, en, state }
   ├──▶ [GUI 共享状态]  Arc<AppState> 原子/RwLock, UI 帧读取渲染
   └──▶ [TTS 队列任务] async 串行
          │ 合成 mp3 bytes
          ▼
        [播放]  rodio Sink, 设置 tts_playing 标志直至播完
                    │
                    └──▶ 回声防护: 分段线程读标志, true 时清空缓冲不再拾音

[UI 线程]   eframe 主循环: 读 AppState 渲染; 用户操作写命令通道/原子量
```

**通道一览**

| 通道 | 类型 | 生产者 → 消费者 | 载荷 |
|---|---|---|---|
| raw_tx | std mpsc (try_send) | cpal 回调 → 分段线程 | 48k f32 块（约 10 ms） |
| seg_tx | tokio unbounded | 分段线程 → 识别任务 | 16k f32 完整语音段 |
| tts_tx | tokio unbounded | 翻译任务 → TTS 任务 | (文本, 音色, 语速, 音调) |
| cmd | 原子量/RwLock | UI → 各模块 | 开始/停止、口音、静音、防护开关 |

## 4. 关键设计决策

### 4.1 断句（VAD）策略
- 帧级 RMS 能量检测（30 ms 窗）：能量 > 阈值判为语音帧；
- 触发条件：连续语音 ≥ `min_speech_ms`(300ms) → 进入"说话中"；
- 切句条件：静音 ≥ `silence_ms`(750ms) 或说话时长 ≥ `max_segment_ms`(12s) 强制切；
- 触发前回溯保留 `pre_roll_ms`(150ms)，避免吃掉句首；
- **自动校准**：启动后静默 1 s 测噪声底，阈值 = max(噪声×3, 0.004)；可在设置中改手动。

### 4.2 翻译双引擎与回退
```
translate = "llm" 且 api_key 非空:
    POST {base_url}/chat/completions
    system: "你是专业中英同传。将中文译成{美式|英式}英语，口语自然，
             使用对应口音的拼写与用词（如 colour/lift vs color/elevator），
             只输出译文。"  temperature=0.2  timeout=15s
    失败(无Key/超时/HTTP错误) → 自动回退 ②，状态栏提示
translate = "whisper":
    ② whisper task=translate，识别即直译（离线、零配置、质量中等）
```
注：直译模式下只有英文结果，界面原文行显示"—"；LLM 模式下原文/译文双行显示。

### 4.3 TTS 队列与回声防护
- TTS 串行队列保证多句播报不重叠、顺序正确；
- `tts_playing: AtomicBool` 在播放期间置位：
  - 扬声器模式（默认）：分段线程丢弃输入并清空半截语音，杜绝"自翻自"；
  - 耳机模式（设置可开）：不丢弃，可边播边说。

### 4.4 状态机（UI 同步定义，详见 UI 文档 §5）
```
S0待机 →(开始)→ S1监听 →(VAD触发)→ S2采集中 →(静音断句)→ S3识别 → S4翻译 → S5播报 →(播完)→ S1监听
   ▲                └────────────── 任意状态点停止 ──────────────┘
```

## 5. 配置设计（config.toml）

首启生成默认值；改动即时生效（口音/静音/断句），模型与引擎重启生效。

```toml
[audio]
input_device = ""          # 留空 = 系统默认输入

[vad]
auto_calibrate = true      # 启动时自动测环境噪声
energy_threshold = 0.010   # 手动阈值（auto_calibrate=false 时生效）
silence_ms = 750           # 静音多久判定一句话结束
min_speech_ms = 300        # 最短语音时长（滤除噪声）
max_segment_ms = 12000     # 最长单句，超时强制切分
pre_roll_ms = 150          # 触发前回溯保留

[asr]
model = "base"             # tiny | base | small
model_path = "models/ggml-base.bin"
language = "zh"            # zh | auto

[translate]
engine = "llm"             # llm | whisper
[translate.llm]
base_url = "https://api.deepseek.com/v1"
api_key = ""               # 为空自动回退 whisper 直译
model = "deepseek-chat"
timeout_sec = 15

[tts]
enabled = true             # 自动播报
accent = "us"              # us | gb   ← 默认美式
voice_us = "en-US-AriaNeural"
voice_gb = "en-GB-SoniaNeural"
rate = "+0%"
pitch = "+0Hz"

[ui]
echo_guard = true          # 回声防护（外放模式）
theme = "dark"
```

## 6. 模型与资源下载

| 模型 | 大小 | 质量/速度 | 用途建议 |
|---|---|---|---|
| ggml-tiny.bin | ~75 MB | 快、中文一般 | 低配机器 |
| ggml-base.bin | ~142 MB | 均衡（**默认**） | 日常 |
| ggml-small.bin | ~466 MB | 明显更好、更慢 | 追求质量 |

下载源（按序尝试）：
1. `https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/<model>`（国内镜像）
2. `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<model>`

校验：文件大小 ≥ 公称值 95% 才视为完整，否则删除重下。

## 7. 工程结构

```
hirust-translator/
├── Cargo.toml
├── config.toml               # 运行时生成/读取
├── models/                   # ggml 模型目录（不入库）
├── docs/                     # 设计文档
└── src/
    ├── main.rs               # eframe 入口、首次运行检查
    ├── app.rs                # App 状态 + 状态机 + egui 顶层布局
    ├── ui/                   # 界面组件
    │   ├── mod.rs
    │   ├── top_bar.rs        # 状态栏（品牌/状态胶囊/口音徽章）
    │   ├── transcript.rs     # 记录区（会话卡片、进行中指示）
    │   ├── controls.rs       # 控制栏（主按钮/口音分段/开关组）
    │   ├── level_meter.rs    # 电平条
    │   └── settings.rs       # 设置面板
    ├── audio/
    │   ├── mod.rs
    │   ├── capture.rs        # cpal 采集 + 单声道混合
    │   ├── resample.rs       # 48k→16k 线性插值（跨块保持相位）
    │   └── vad.rs            # 能量 VAD + 断句 + 自动校准 + 回声门
    ├── asr/
    │   └── mod.rs            # whisper-rs 封装（transcribe / translate 两用）
    ├── translate/
    │   ├── mod.rs            # 引擎选择 + 回退逻辑
    │   └── llm.rs            # OpenAI 兼容 chat 客户端
    ├── tts/
    │   ├── mod.rs            # 队列编排、tts_playing 门
    │   ├── edge.rs           # msedge-tts 合成
    │   └── player.rs         # rodio 播放
    ├── pipeline.rs           # 线程/任务编排、通道定义
    ├── model_dl.rs           # 模型下载（镜像回退、进度、校验）
    └── config.rs             # 配置结构 + 默认值 + 读写
```

## 8. 错误处理与降级

| 故障 | 表现 | 策略 |
|---|---|---|
| 无输入设备 | cpal 枚举失败 | 启动即提示并列设备名，禁用开始按钮 |
| 识别返回空/纯噪声 | whisper 输出空 | 丢弃该段，不生成卡片，静默继续 |
| LLM 失败/超时 | HTTP 错误 | 该句回退直译模式重跑一次（直译模式下直接报错提示） |
| TTS 网络失败 | 合成超时 | 译文照常上屏，状态栏黄条提示"播报失败"，继续监听 |
| 模型缺失/损坏 | 大小校验不过 | 触发首次运行下载流程 |
| 播放设备缺失 | rodio 初始化失败 | 禁用播报，仅文字模式 |

## 9. 构建与运行环境

| 项 | 要求 | 本机核查（2026-11-17） |
|---|---|---|
| Rust | ≥ 1.85 | ✅ rustc 1.97.1 |
| CMake | ≥ 3.20（whisper.cpp） | ✅ 3.28.1 |
| libclang（可选） | 仅在运行 bindgen 生成绑定时需要；**实际构建设置 `WHISPER_DONT_GENERATE_BINDINGS=1` 使用 crate 自带绑定，可完全绕过**（新版 clang 23 会产生 `whisper_full_params` 布局断言失败） | ✅ 已装 LLVM 23，但走自带绑定方案 |
| MSVC Build Tools | C++ 编译器（cmake 编译 whisper.cpp 用） | ✅ 已随工具链就绪 |
| cargo 镜像 | 本机 `CARGO_HOME=D:\rust\cargo`，已配置 rsproxy.cn 镜像 | ✅ 直连 crates.io 会卡索引 |
| 网络 | 首次需下载模型；LLM/TTS 需外网 | 镜像已内置 |

## 10. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| whisper-rs 构建链问题 | 编译失败 | 已装 CMake；备选：切 `openmp`/`simd` 特性调优；极端情况降级 tiny 模型不解决编译——保留 candle 纯 Rust 备选方案（成本：更换 asr 模块） |
| Edge-TTS 服务端令牌策略变化 | 播报失效 | 跟随 msedge-tts 版本升级；降级为 Windows 本地 SAPI 音色（`tts` crate） |
| base 模型中文识别质量一般 | 译文不准 | 设置里一键升级 small；LLM 提示词容错；文档明示各模型定位 |
| 外放回声 | 自翻自 | 回声防护默认开启（FR-11） |
| HF 下载慢/失败 | 首启卡住 | hf-mirror 优先 + 可切换镜像 + 失败给出手动下载 URL |
