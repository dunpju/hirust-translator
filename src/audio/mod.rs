//! 音频层：设备采集、48k→16k 重采样、VAD 断句

pub mod capture;
pub mod resample;
pub mod vad;

/// whisper 要求的目标采样率
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
