//! 翻译层：引擎选择（LLM / Whisper 直译）

pub mod llm;

use crate::config::TranslateConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// 先 whisper 转写中文，再交 LLM 翻译（质量高，需 API Key）
    Llm,
    /// whisper 内置 translate 任务直接输出英文（离线、零配置）
    WhisperDirect,
}

/// 实际生效的引擎：engine=llm 且配置了 api_key 才走 LLM
pub fn effective(cfg: &TranslateConfig) -> Engine {
    if cfg.engine.eq_ignore_ascii_case("llm") && !cfg.llm.api_key.trim().is_empty() {
        Engine::Llm
    } else {
        Engine::WhisperDirect
    }
}

impl Engine {
    pub fn label(&self) -> &'static str {
        match self {
            Engine::Llm => "LLM",
            Engine::WhisperDirect => "Whisper 直译",
        }
    }
}
