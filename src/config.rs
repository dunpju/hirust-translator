//! 配置：加载 / 生成默认模板 config.toml

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// 口音：美式（默认）/ 英式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Us,
    Gb,
}

impl Default for Accent {
    fn default() -> Self {
        Accent::Us // 默认美式
    }
}

impl Accent {
    pub fn label(&self) -> &'static str {
        match self {
            Accent::Us => "美式",
            Accent::Gb => "英式",
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Accent::Us => "us",
            Accent::Gb => "gb",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// 留空 = 系统默认输入设备
    pub input_device: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { input_device: String::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub auto_calibrate: bool,
    pub energy_threshold: f32,
    pub silence_ms: u64,
    pub min_speech_ms: u64,
    pub max_segment_ms: u64,
    pub pre_roll_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            auto_calibrate: true,
            energy_threshold: 0.010,
            silence_ms: 750,
            min_speech_ms: 300,
            max_segment_ms: 12000,
            pre_roll_ms: 150,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// tiny | base | small
    pub model: String,
    /// 相对运行目录，或绝对路径
    pub model_path: String,
    /// zh | auto
    pub language: String,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model: "base".into(),
            model_path: "models/ggml-base.bin".into(),
            language: "zh".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_sec: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com/v1".into(),
            api_key: String::new(),
            model: "deepseek-chat".into(),
            timeout_sec: 15,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TranslateConfig {
    /// llm | whisper
    pub engine: String,
    pub llm: LlmConfig,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self { engine: "llm".into(), llm: LlmConfig::default() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub accent: Accent,
    pub voice_us: String,
    pub voice_gb: String,
    pub rate: String,
    pub pitch: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            accent: Accent::Us,
            voice_us: "en-US-AriaNeural".into(),
            voice_gb: "en-GB-SoniaNeural".into(),
            rate: "+0%".into(),
            pitch: "+0Hz".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// 回声防护：播报时暂停拾音（外放场景）
    pub echo_guard: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { echo_guard: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub vad: VadConfig,
    pub asr: AsrConfig,
    pub translate: TranslateConfig,
    pub tts: TtsConfig,
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            vad: VadConfig::default(),
            asr: AsrConfig::default(),
            translate: TranslateConfig::default(),
            tts: TtsConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl Config {
    /// 读取配置；文件不存在则生成默认模板后返回默认值
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            fs::write(path, DEFAULT_TOML)
                .with_context(|| format!("写入默认配置失败: {}", path.display()))?;
            eprintln!("已生成默认配置: {}", path.display());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("解析配置失败: {}（可删除后重新生成）", path.display()))?;
        Ok(cfg)
    }

    /// 模型路径（相对路径基于当前工作目录）
    pub fn model_path(&self) -> PathBuf {
        PathBuf::from(&self.asr.model_path)
    }

    /// 当前口音对应的音色
    pub fn voice_for(&self, accent: Accent) -> &str {
        match accent {
            Accent::Us => &self.tts.voice_us,
            Accent::Gb => &self.tts.voice_gb,
        }
    }
}

pub const DEFAULT_TOML: &str = r#"# Hirust 中英同声传译 配置文件
# 口音说明：accent = "us"（美式，默认） | "gb"（英式）

[audio]
# 输入设备名，留空 = 系统默认
input_device = ""

[vad]
# 启动时自动测量环境噪声并设定阈值
auto_calibrate = true
# 手动阈值（auto_calibrate = false 时生效）
energy_threshold = 0.010
# 静音多久判定一句话结束（毫秒）
silence_ms = 750
# 最短语音时长（毫秒），滤除短噪声
min_speech_ms = 300
# 单句最长（毫秒），超时强制断句
max_segment_ms = 12000
# 触发前回溯保留（毫秒），避免吃掉句首
pre_roll_ms = 150

[asr]
# 模型：tiny | base | small（首次自动下载）
model = "base"
model_path = "models/ggml-base.bin"
# 识别语言：zh | auto
language = "zh"

[translate]
# 引擎：llm（推荐，需 API Key） | whisper（内置直译，离线可用）
engine = "llm"
[translate.llm]
# 任意 OpenAI 兼容接口（DeepSeek / Moonshot / OpenAI 等）
base_url = "https://api.deepseek.com/v1"
api_key = ""
model = "deepseek-chat"
timeout_sec = 15

[tts]
# 是否自动播报译文
enabled = true
# 默认口音：us 美式 | gb 英式
accent = "us"
voice_us = "en-US-AriaNeural"
voice_gb = "en-GB-SoniaNeural"
rate = "+0%"
pitch = "+0Hz"

[ui]
# 回声防护：外放时播报期间暂停拾音；戴耳机可改为 false
echo_guard = true
"#;
