//! cpal 麦克风采集：回调内混单声道（原始采样率），经无界通道送出

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::Sender;

pub struct Capture {
    /// 必须持有 Stream 才能持续采集
    pub stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: usize,
    pub device_name: String,
}

/// 列出所有输入设备名
pub fn list_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let names: Vec<String> = host
        .input_devices()?
        .map(|d| d.name().unwrap_or_else(|_| "<未知>".into()))
        .collect();
    Ok(names)
}

/// 打开输入设备开始采集；每块数据混为单声道 f32（设备原始采样率）发送到 tx
pub fn start(device_name: Option<&str>, tx: Sender<Vec<f32>>) -> Result<Capture> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) if !name.is_empty() => host
            .input_devices()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .with_context(|| format!("找不到输入设备: {name}"))?,
        _ => host
            .default_input_device()
            .context("未找到默认输入设备（请检查麦克风连接/权限）")?,
    };
    let device_name = device.name().unwrap_or_default();

    let supported = device
        .default_input_config()
        .context("获取输入音频配置失败")?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.clone().into();

    let err_fn = |err| eprintln!("[audio] 音频流错误: {err}");

    macro_rules! mono_push {
        ($data:expr, |$s:ident| $conv:expr) => {{
            let mut mono = Vec::with_capacity($data.len() / channels.max(1));
            for frame in $data.chunks(channels) {
                let mut acc = 0.0f32;
                let mut n = 0usize;
                for $s in frame {
                    acc += $conv;
                    n += 1;
                }
                mono.push(acc / n.max(1) as f32);
            }
            // 无界通道，绝不阻塞音频回调；对端关闭则丢弃
            let _ = tx.send(mono);
        }};
    }

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &_| mono_push!(data, |s| *s as f32),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &_| mono_push!(data, |s| (*s as f32) / 32768.0),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &_| mono_push!(data, |s| ((*s as f32) - 32768.0) / 32768.0),
            err_fn,
            None,
        ),
        other => anyhow::bail!("不支持的采样格式: {other:?}"),
    }
    .context("打开音频输入流失败")?;

    stream.play().context("启动音频流失败")?;

    Ok(Capture { stream, sample_rate, channels, device_name })
}
