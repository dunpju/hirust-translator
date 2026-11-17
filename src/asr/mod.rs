//! whisper.cpp 本地语音识别（whisper-rs）

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrMode {
    /// 转写为源语言文本（中文）
    Transcribe,
    /// whisper 内置直译：直接输出英文译文
    TranslateDirect,
}

pub struct Asr {
    ctx: WhisperContext,
    threads: usize,
    language: Option<String>,
}

impl Asr {
    pub fn load(model_path: &Path, language: Option<String>) -> Result<Self> {
        let bytes = std::fs::read(model_path)
            .with_context(|| format!("读取模型失败: {}", model_path.display()))?;
        let ctx = WhisperContext::new_from_buffer_with_params(
            &bytes,
            WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow!("加载 whisper 模型失败: {e}"))?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        Ok(Self { ctx, threads, language })
    }

    /// 对一段 16kHz 单声道 f32 语音执行推理，返回文本
    pub fn run(&mut self, samples: &[f32], mode: AsrMode) -> Result<String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow!("创建 whisper state 失败: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads as i32);
        match self.language.as_deref() {
            Some("auto") | None => {}
            Some(lang) => params.set_language(Some(lang)),
        }
        if matches!(mode, AsrMode::TranslateDirect) {
            params.set_translate(true);
        }
        params.set_suppress_nst(true);
        // 关闭 whisper.cpp 的 stderr 输出
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, samples)
            .map_err(|e| anyhow!("whisper 推理失败: {e}"))?;

        let n = state.full_n_segments().map_err(|e| anyhow!("{e}"))?;
        let mut text = String::new();
        for i in 0..n {
            let seg = state.full_get_segment_text(i).map_err(|e| anyhow!("{e}"))?;
            text.push_str(&seg);
        }
        Ok(text.trim().to_string())
    }
}
