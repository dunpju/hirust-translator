//! UniFFI 接口：Android (Kotlin) ↔ Rust 引擎
//!
//! 职责划分：
//! - Kotlin：AudioRecord 采集 16k 单声道 f32 → feed_pcm 推流；MediaPlayer 播放 on_mp3 回调的音频
//! - Rust：VAD 断句 → whisper 识别 → 翻译 → Edge-TTS 合成 mp3 → 回调

use crate::config::{Accent, Config};
use crate::model_dl::{self, DlProgress};
use crate::pipeline::{self, AudioSink, EngineEvent, MicHandle, MicSource, NoticeKind};
use anyhow::Result as AResult;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

/// UniFFI 错误类型（Kotlin 映射为异常类；String 不能直接作抛错类型）
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum HirustError {
    #[error("配置错误: {0}")]
    Config(String),
    #[error("引擎错误: {0}")]
    Engine(String),
}

/// Kotlin 侧回调（UniFFI callback interface）
#[uniffi::export(callback_interface)]
pub trait HirustCallback: Send + Sync {
    /// 一句话完成：ts=时间戳 zh=中文原文（直译模式为 None） en=英文译文 engine=所用引擎
    fn on_utterance(&self, ts: String, zh: Option<String>, en: String, engine: String);
    /// 相位变化文案（待机/监听中/正在听你说…/识别中/翻译中/播报中/已暂停）
    fn on_phase(&self, phase: String);
    /// 通知：kind 0=info 1=warn 2=error
    fn on_notice(&self, kind: i32, msg: String);
    /// TTS 音频（mp3 字节），Kotlin 播放完毕后须调用 playback_done()
    fn on_mp3(&self, bytes: Vec<u8>);
    /// 模型下载进度；total=None 表示未知大小
    fn on_model_progress(&self, downloaded: u64, total: Option<u64>);
}

// ── 音频注入实现 ────────────────────────────────────────────

/// 安卓"麦克风源"：Kotlin 线程调用 feed_pcm 推流
#[derive(Clone)]
struct FeedMicSrc(Arc<FeedMicHandle>);

struct FeedMicHandle {
    tx: Mutex<Option<mpsc::Sender<Vec<f32>>>>,
}

impl MicSource for FeedMicSrc {
    fn start(self: Box<Self>, tx: mpsc::Sender<Vec<f32>>) -> AResult<MicHandle> {
        *self.0.tx.lock().unwrap() = Some(tx);
        Ok(MicHandle::new(crate::audio::TARGET_SAMPLE_RATE))
    }
}

/// 安卓音频输出：mp3 回调 Kotlin，阻塞等待 playback_done()（维持串行队列与回声防护语义）
struct CallbackSink {
    cb: Arc<dyn HirustCallback>,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl AudioSink for CallbackSink {
    fn play(&self, mp3: Vec<u8>) -> AResult<()> {
        let (lock, cvar) = &*self.done;
        *lock.lock().unwrap() = false;
        self.cb.on_mp3(mp3);
        let g = lock.lock().unwrap();
        let (guard, timeout) = cvar
            .wait_timeout_while(g, Duration::from_secs(180), |done| !*done)
            .unwrap();
        let _timed_out = timeout.timed_out();
        drop(guard);
        Ok(())
    }
}

// ── 引擎对象 ────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct HirustEngine {
    state: Arc<pipeline::SharedState>,
    feed: Arc<FeedMicHandle>,
    done: Arc<(Mutex<bool>, Condvar)>,
    cb: Arc<dyn HirustCallback>,
    cfg: Mutex<Config>,
    rt_handle: tokio::runtime::Handle,
    /// 持有 tokio Runtime（drop 即后台任务终止）
    keep: Mutex<Option<Box<dyn Send + Sync>>>,
}

#[uniffi::export]
impl HirustEngine {
    /// cfg_toml 传空串则使用默认配置；model_dir 为安卓 App 私有目录绝对路径
    #[uniffi::constructor]
    pub fn new(
        cfg_toml: String,
        model_dir: String,
        callback: Box<dyn HirustCallback>,
    ) -> Result<Arc<Self>, HirustError> {
        let callback: Arc<dyn HirustCallback> = Arc::from(callback);
        let mut cfg = if cfg_toml.trim().is_empty() {
            Config::default()
        } else {
            toml::from_str(&cfg_toml).map_err(|e| HirustError::Config(format!("解析配置失败: {e}")))?
        };
        // 模型固定落在 App 私有目录
        let model_path = Path::new(&model_dir).join(model_dl::model_file_name(&cfg.asr.model));
        cfg.asr.model_path = model_path.to_string_lossy().into_owned();

        let feed = Arc::new(FeedMicHandle { tx: Mutex::new(None) });
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let sink = Arc::new(CallbackSink { cb: callback.clone(), done: done.clone() });

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| HirustError::Engine(format!("创建运行时失败: {e}")))?;
        let rt_handle = rt.handle().clone();
        let _guard = rt.enter();
        let (state, mut rx, _mic_handle) =
            pipeline::start_with(cfg.clone(), Box::new(FeedMicSrc(feed.clone())), sink)
                .map_err(|e| HirustError::Engine(format!("引擎启动失败: {e:#}")))?;

        // 事件泵：引擎事件 → Kotlin 回调
        let cb_events = callback.clone();
        std::thread::Builder::new()
            .name("uniffi-events".into())
            .spawn(move || {
                while let Some(ev) = rx.blocking_recv() {
                    match ev {
                        EngineEvent::Utterance { ts, zh, en, engine } => {
                            cb_events.on_utterance(ts, zh, en, engine.to_string());
                        }
                        EngineEvent::Phase(p) => cb_events.on_phase(p.desc().to_string()),
                        EngineEvent::Notice(k, m) => cb_events.on_notice(
                            match k {
                                NoticeKind::Info => 0,
                                NoticeKind::Warn => 1,
                                NoticeKind::Error => 2,
                            },
                            m,
                        ),
                        EngineEvent::Calibrating => cb_events.on_phase("校准环境噪声…".to_string()),
                        EngineEvent::Calibrated(th) => {
                            cb_events.on_notice(0, format!("噪声校准完成 阈值={th:.4}"));
                        }
                    }
                }
            })
            .map_err(|e| HirustError::Engine(format!("启动事件线程失败: {e}")))?;

        let keep_box: Box<dyn Send + Sync> = Box::new(rt);
        Ok(Arc::new(Self {
            state,
            feed,
            done,
            cb: callback,
            cfg: Mutex::new(cfg),
            rt_handle,
            keep: Mutex::new(Some(keep_box)),
        }))
    }

    /// Kotlin AudioRecord 推流（16kHz 单声道 f32）
    pub fn feed_pcm(&self, samples: Vec<f32>, _sample_rate: u32) {
        if let Some(tx) = self.feed.tx.lock().unwrap().as_ref() {
            let _ = tx.send(samples);
        }
    }

    /// 当前模型是否就绪
    pub fn model_ready(&self) -> bool {
        let cfg = self.cfg.lock().unwrap().clone();
        model_dl::is_model_ok(&cfg.asr.model, &cfg.model_path())
    }

    /// 下载模型（异步，进度走 on_model_progress）
    pub fn download_model(&self) {
        let cfg = self.cfg.lock().unwrap().clone();
        let model = cfg.asr.model.clone();
        let path = cfg.model_path();
        let cb_progress = self.cb.clone();
        let cb_result = self.cb.clone();
        self.rt_handle.spawn(async move {
            let r = model_dl::ensure_model(&model, &path, move |ev: DlProgress| match ev {
                DlProgress::Start { total, .. } => cb_progress.on_model_progress(0, total),
                DlProgress::Bytes(n) => cb_progress.on_model_progress(n, None),
                DlProgress::Done => cb_progress.on_model_progress(1, Some(1)),
            })
            .await;
            if let Err(e) = r {
                cb_result.on_notice(2, format!("模型下载失败: {e:#}"));
            }
        });
    }

    // ── 运行控制 ──
    pub fn set_paused(&self, paused: bool) {
        self.state.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_accent_us(&self, us: bool) {
        self.state.set_accent(if us { Accent::Us } else { Accent::Gb });
    }

    pub fn accent_is_us(&self) -> bool {
        matches!(self.state.accent(), Accent::Us)
    }

    pub fn set_muted(&self, muted: bool) {
        self.state.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_echo_guard(&self, guard: bool) {
        self.state.echo_guard.store(guard, Ordering::Relaxed);
    }

    pub fn phase(&self) -> String {
        self.state.phase().desc().to_string()
    }

    /// 重播/试听
    pub fn speak(&self, text: String) -> bool {
        self.state.speak(text)
    }

    /// Kotlin 播放完 on_mp3 音频后调用（解锁 TTS 队列）
    pub fn playback_done(&self) {
        let (lock, cvar) = &*self.done;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }

    /// 输入电平（RMS，0~1），供 UI 电平条轮询（诊断麦克风→引擎链路）
    pub fn audio_level(&self) -> f32 {
        self.state.level()
    }

    /// VAD 诊断状态（阈值/是否说话中/切段/识别计数），排障用
    pub fn vad_state(&self) -> String {
        let s = &self.state;
        format!(
            "阈值 {:.4} | 说话中 {} | 已切段 {} | 识别 {} | 空段 {}",
            f32::from_bits(s.vad_threshold.load(Ordering::Relaxed)),
            if s.vad_speaking.load(Ordering::Relaxed) { "是" } else { "否" },
            s.segments_cut.load(Ordering::Relaxed),
            s.asr_done.load(Ordering::Relaxed),
            s.asr_empty.load(Ordering::Relaxed),
        )
    }

    /// 灵敏度档位：2=高(0.0003) 1=中(0.0008) 0=低(0.002) -1=自动（校准+自适应噪声底）
    pub fn set_vad_sensitivity(&self, level: i32) {
        let t: f32 = match level {
            2 => 0.0003,
            1 => 0.0008,
            0 => 0.002,
            _ => 0.0, // 自动
        };
        self.state.vad_threshold_override.store(t.to_bits(), Ordering::Relaxed);
    }

    /// 停止引擎（不可再启动；Kotlin 侧应重建 HirustEngine）
    pub fn shutdown(&self) {
        *self.feed.tx.lock().unwrap() = None;
        self.state.shutdown();
        *self.keep.lock().unwrap() = None;
    }
}
