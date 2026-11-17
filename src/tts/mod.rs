//! 语音合成：Edge-TTS 神经音色（en-US / en-GB），产出 mp3 字节
//! 播放（rodio）仅桌面；安卓端 mp3 回调 Kotlin 播放

#[cfg(not(target_os = "android"))]
pub mod player;

use anyhow::{anyhow, Context, Result};
use msedge_tts::tts::{client::tokio_runtime::connect_async, SpeechConfig};
use msedge_tts::voice::tokio_runtime::get_voices_list_async;
use msedge_tts::voice::Voice;

/// 一条播报请求
#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    /// 音色短名，如 en-US-AriaNeural / en-GB-SoniaNeural
    pub voice: String,
    /// 如 "+0%"、"+10%"
    pub rate: String,
    /// 如 "+0Hz"、"+2Hz"
    pub pitch: String,
}

/// 从 "+10%" / "-5Hz" 之类字符串提取带符号整数
pub fn parse_signed_int(s: &str) -> i32 {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
        .collect();
    t.parse().unwrap_or(0)
}

static VOICES: tokio::sync::OnceCell<Result<Vec<Voice>, anyhow::Error>> =
    tokio::sync::OnceCell::const_new();

async fn voices() -> Result<&'static [Voice]> {
    let res = VOICES
        .get_or_init(|| async { get_voices_list_async().await.map_err(anyhow::Error::from) })
        .await;
    res.as_ref()
        .map(|v| v.as_slice())
        .map_err(|e| anyhow!("获取音色列表失败: {e}"))
}

/// Edge-TTS 合成（每次独立连接，自带重连语义；音色列表全局缓存）
pub struct EdgeTts;

impl EdgeTts {
    pub async fn synthesize(req: &TtsRequest) -> Result<Vec<u8>> {
        let all = voices().await?;
        let voice = all
            .iter()
            .find(|v| v.short_name.as_deref() == Some(req.voice.as_str()))
            .with_context(|| format!("找不到音色 {}", req.voice))?;

        let mut cfg = SpeechConfig::from(voice);
        // SSML voice 节点使用短名（From<&Voice> 默认取长名，实测短名才是服务端期望值）
        cfg.voice_name = req.voice.clone();
        cfg.rate = parse_signed_int(&req.rate);
        cfg.pitch = parse_signed_int(&req.pitch);

        let mut client = connect_async().await.map_err(|e| anyhow!("连接 Edge-TTS 失败: {e}"))?;
        let audio = client
            .synthesize(&req.text, &cfg)
            .await
            .map_err(|e| anyhow!("语音合成失败: {e}"))?;
        Ok(audio.audio_bytes)
    }
}
