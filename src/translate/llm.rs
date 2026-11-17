//! OpenAI 兼容 Chat API 翻译客户端（DeepSeek / Moonshot / OpenAI 等）

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::config::{Accent, LlmConfig};

pub struct LlmClient {
    http: reqwest::Client,
    pub cfg: LlmConfig,
}

impl LlmClient {
    pub fn new(cfg: &LlmConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_sec))
            .build()?;
        Ok(Self { http, cfg: cfg.clone() })
    }

    /// 中文 → 指定口音英文
    pub async fn translate(&self, accent: Accent, text: &str) -> Result<String> {
        let (acc, examples) = match accent {
            Accent::Us => ("美式", "color/elevator/apartment"),
            Accent::Gb => ("英式", "colour/lift/flat"),
        };
        let system = format!(
            "你是一名专业的中英同声传译员。把用户提供的中文翻译成{acc}英语。要求：\
             1) 只输出英文译文本身，不要解释、不要重复原文；\
             2) 口语自然流畅，符合日常口头表达；\
             3) 使用{acc}英语的拼写与用词习惯（如 {examples}）；\
             4) 人名等专有名词保留原文。"
        );

        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.cfg.api_key.trim())
            .json(&serde_json::json!({
                "model": self.cfg.model,
                "temperature": 0.2,
                "stream": false,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": text }
                ]
            }))
            .send()
            .await
            .with_context(|| format!("请求 LLM 失败: {url}"))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let body: String = resp.text().await.unwrap_or_default().chars().take(300).collect();
            bail!("LLM 返回 {code}: {body}");
        }

        let body: ChatResponse = resp.json().await.context("解析 LLM 响应失败")?;
        body.choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("LLM 返回内容为空"))
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}
