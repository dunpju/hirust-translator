//! Hirust 便携核心库
//!
//! 同一份引擎同时服务两个宿主：
//! - 桌面版（本仓库 bin：CLI + egui GUI），cpal 采集 / rodio 播放
//! - 安卓版（cdylib `libhirust_translator.so` + UniFFI），Kotlin 采音推流 / 回调 mp3 播放

pub mod asr;
pub mod audio;
pub mod config;
pub mod jni_api;
pub mod model_dl;
pub mod pipeline;
pub mod translate;
pub mod tts;

uniffi::setup_scaffolding!();
