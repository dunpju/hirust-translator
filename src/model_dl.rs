//! whisper ggml 模型下载（hf-mirror 镜像优先，HuggingFace 兜底）

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 模型公称大小（字节，近似值，用于完整性下限校验）
pub const MODELS: &[(&str, u64)] = &[
    ("tiny", 78_643_200),   // ~75 MiB
    ("base", 148_897_792),  // ~142 MiB
    ("small", 488_628_736), // ~466 MiB
];

pub fn expected_size(model: &str) -> Option<u64> {
    MODELS.iter().find(|(n, _)| *n == model).map(|(_, s)| *s)
}

pub fn model_file_name(model: &str) -> String {
    format!("ggml-{model}.bin")
}

pub fn download_urls(model: &str) -> Vec<String> {
    let file = model_file_name(model);
    vec![
        format!("https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/{file}"),
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}"),
    ]
}

pub enum DlProgress {
    Start { total: Option<u64>, url: String },
    Bytes(u64),
    Done,
}

/// 确保模型就绪：存在且大小达标则直接返回，否则下载
pub async fn ensure_model(
    model: &str,
    path: &Path,
    mut on: impl FnMut(DlProgress) + Send,
) -> Result<()> {
    if is_model_ok(model, path) {
        on(DlProgress::Done);
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建模型目录失败: {}", parent.display()))?;
        }
    }
    let part: PathBuf = path.with_extension("part");
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(1800))
        .build()?;

    let mut last_err: Option<anyhow::Error> = None;
    for url in download_urls(model) {
        match download_one(&http, &url, &part, &mut on).await {
            Ok(()) => {
                fs::rename(&part, path)
                    .with_context(|| format!("保存模型失败: {}", path.display()))?;
                on(DlProgress::Done);
                return Ok(());
            }
            Err(e) => {
                eprintln!("[下载] 源失败 {url}: {e:#}");
                let _ = fs::remove_file(&part);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("无可用下载源")))
}

pub fn is_model_ok(model: &str, path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(md) => match expected_size(model) {
            // 大小达到公称值 95% 视为完整
            Some(exp) => md.len() >= exp * 95 / 100,
            None => md.len() > 1_000_000,
        },
        Err(_) => false,
    }
}

async fn download_one(
    http: &reqwest::Client,
    url: &str,
    part: &Path,
    on: &mut (impl FnMut(DlProgress) + Send),
) -> Result<()> {
    let resp = http.get(url).send().await.context("请求失败")?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let total = resp.content_length();
    on(DlProgress::Start { total, url: url.to_string() });

    let mut file = tokio::fs::File::create(part)
        .await
        .with_context(|| format!("创建临时文件失败: {}", part.display()))?;
    let mut stream = resp.bytes_stream();
    let mut got: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("下载中断")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        got += chunk.len() as u64;
        on(DlProgress::Bytes(got));
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);

    if let Some(t) = total {
        if got < t * 95 / 100 {
            bail!("下载不完整: {got}/{t} 字节");
        }
    }
    Ok(())
}
