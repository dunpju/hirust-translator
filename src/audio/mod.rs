//! 音频层：采集（桌面）、重采样、VAD 断句
//!
//! 安卓端采集在 Kotlin（AudioRecord），通过 jni_api::feed_pcm 推流进来，
//! 因此 capture 模块仅桌面编译。

pub mod resample;
pub mod vad;

#[cfg(not(target_os = "android"))]
pub mod capture;

/// whisper 要求的目标采样率
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
