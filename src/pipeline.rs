//! 引擎编排（平台无关）：PCM 推流 → VAD 分段 → 识别/翻译 → TTS 合成 → 音频输出
//!
//! 音频输入/输出经 trait 注入：
//! - 桌面（desktop 模块）：cpal 采集 + rodio 播放
//! - 安卓（jni_api 模块）：Kotlin AudioRecord 推流 + mp3 回调 Kotlin 播放
//!
//! 线程模型见 docs/02-architecture.md §3。

use crate::asr::{Asr, AsrMode};
use crate::audio::{resample::Resampler, vad::Vad, TARGET_SAMPLE_RATE};
use crate::config::{Accent, Config};
use crate::translate::{self, llm::LlmClient};
use crate::tts::{EdgeTts, TtsRequest};
use anyhow::Result;
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Listening,
    Capturing,
    Recognizing,
    Translating,
    Speaking,
}

impl Phase {
    pub fn desc(&self) -> &'static str {
        match self {
            Phase::Idle => "待机",
            Phase::Listening => "监听中",
            Phase::Capturing => "正在听你说…",
            Phase::Recognizing => "识别中",
            Phase::Translating => "翻译中",
            Phase::Speaking => "播报中",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Phase(Phase),
    Calibrating,
    Calibrated(f32),
    Utterance {
        ts: String,
        zh: Option<String>,
        en: String,
        #[allow(dead_code)]
        engine: &'static str,
    },
    Notice(NoticeKind, String),
}

pub struct SharedState {
    phase: RwLock<Phase>,
    accent: RwLock<Accent>,
    /// 录音暂停（引擎保持运行，快速恢复）
    pub paused: AtomicBool,
    pub muted: AtomicBool,
    pub echo_guard: AtomicBool,
    pub tts_enabled: AtomicBool,
    pub running: AtomicBool,
    pub tts_playing: AtomicBool,
    /// 输入电平（f32 位模式），供 UI 电平条读取
    pub level: AtomicU32,
    /// VAD 触发阈值（f32 位模式，校准后更新）
    pub vad_threshold: AtomicU32,
    /// 用户强制阈值（f32 位模式；0 = 自动/自适应）
    pub vad_threshold_override: AtomicU32,
    pub vad_speaking: AtomicBool,
    /// 累计切段 / 识别成功 / 识别为空（诊断 UI 用）
    pub segments_cut: AtomicUsize,
    pub asr_done: AtomicUsize,
    pub asr_empty: AtomicUsize,
    voice_us: RwLock<String>,
    voice_gb: RwLock<String>,
    rate: RwLock<String>,
    pitch: RwLock<String>,
    tts_tx: Mutex<Option<UnboundedSender<TtsRequest>>>,
    events: UnboundedSender<EngineEvent>,
}

impl SharedState {
    pub fn phase(&self) -> Phase {
        *self.phase.read().unwrap()
    }

    fn set_phase(&self, p: Phase) {
        *self.phase.write().unwrap() = p;
        let _ = self.events.send(EngineEvent::Phase(p));
    }

    pub fn accent(&self) -> Accent {
        *self.accent.read().unwrap()
    }

    pub fn set_accent(&self, a: Accent) {
        *self.accent.write().unwrap() = a;
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn voice_us(&self) -> String {
        self.voice_us.read().unwrap().clone()
    }

    pub fn voice_gb(&self) -> String {
        self.voice_gb.read().unwrap().clone()
    }

    pub fn rate(&self) -> String {
        self.rate.read().unwrap().clone()
    }

    pub fn pitch(&self) -> String {
        self.pitch.read().unwrap().clone()
    }

    /// 热更新语音参数（设置面板"保存"时调用，立即生效）
    pub fn set_voice_params(&self, us: String, gb: String, rate: String, pitch: String) {
        *self.voice_us.write().unwrap() = us;
        *self.voice_gb.write().unwrap() = gb;
        *self.rate.write().unwrap() = rate;
        *self.pitch.write().unwrap() = pitch;
    }

    /// 用当前口音播报文本（重播/试听）；返回是否成功入队
    pub fn speak(&self, text: String) -> bool {
        let voice = match self.accent() {
            Accent::Us => self.voice_us(),
            Accent::Gb => self.voice_gb(),
        };
        self.speak_with(voice, self.rate(), self.pitch(), text)
    }

    /// 指定音色播报（设置面板"试听"）
    pub fn speak_with(&self, voice: String, rate: String, pitch: String, text: String) -> bool {
        match self.tts_tx.lock().unwrap().as_ref() {
            Some(tx) => tx.send(TtsRequest { text, voice, rate, pitch }).is_ok(),
            None => false,
        }
    }

    fn notice(&self, kind: NoticeKind, msg: impl Into<String>) {
        let _ = self.events.send(EngineEvent::Notice(kind, msg.into()));
    }

    /// 停止全链路（等价于 Engine::stop，供无 Engine 句柄的宿主使用）
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        *self.tts_tx.lock().unwrap() = None;
    }
}

pub struct Engine {
    /// 麦克风保活句柄（cpal Stream 在 Windows 非 Send，故用 Any；仅桌面存在）
    #[cfg(not(target_os = "android"))]
    _mic_keep: Option<Box<dyn Any>>,
    pub state: Arc<SharedState>,
}

impl Engine {
    pub fn state(&self) -> Arc<SharedState> {
        self.state.clone()
    }

    /// 播报文本（重播/试听入口）
    #[allow(dead_code)]
    pub fn speak(&self, text: String) -> bool {
        self.state.speak(text)
    }

    pub fn stop(self) {
        self.state.shutdown();
        // drop(self) 释放保活句柄与各通道，各线程自然退出
    }
}

// ── 平台无关的音频注入接口 ──────────────────────────────────

/// 麦克风源：持续向通道输送原始采样（单声道 f32，采样率自报）
pub trait MicSource: Send + 'static {
    /// 启动采集；返回采样率与保活句柄（drop 即停止）
    fn start(self: Box<Self>, tx: mpsc::Sender<Vec<f32>>) -> Result<MicHandle>;
}

pub struct MicHandle {
    pub sample_rate: u32,
    #[cfg(not(target_os = "android"))]
    pub keep: Option<Box<dyn Any>>,
}

impl MicHandle {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            #[cfg(not(target_os = "android"))]
            keep: None,
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn with_keep(mut self, keep: Box<dyn Any>) -> Self {
        self.keep = Some(keep);
        self
    }
}

/// 音频输出：阻塞播放 mp3 直至结束（由 TTS 任务在阻塞线程调用）
pub trait AudioSink: Send + Sync + 'static {
    fn play(&self, mp3: Vec<u8>) -> Result<()>;
}

/// 启动引擎；必须在 tokio 运行时内调用。
/// 返回共享状态、事件流与麦克风句柄（桌面端需用句柄构造 Engine 保活）。
pub fn start_with(
    cfg: Config,
    mic: Box<dyn MicSource>,
    sink: Arc<dyn AudioSink>,
) -> Result<(Arc<SharedState>, UnboundedReceiver<EngineEvent>, MicHandle)> {
    let (ev_tx, ev_rx) = unbounded_channel();
    let state = Arc::new(SharedState {
        phase: RwLock::new(Phase::Idle),
        accent: RwLock::new(cfg.tts.accent),
        paused: AtomicBool::new(false),
        muted: AtomicBool::new(false),
        echo_guard: AtomicBool::new(cfg.ui.echo_guard),
        tts_enabled: AtomicBool::new(cfg.tts.enabled),
        running: AtomicBool::new(true),
        tts_playing: AtomicBool::new(false),
        level: AtomicU32::new(0),
        vad_threshold: AtomicU32::new(cfg.vad.energy_threshold.to_bits()),
        vad_threshold_override: AtomicU32::new(0),
        vad_speaking: AtomicBool::new(false),
        segments_cut: AtomicUsize::new(0),
        asr_done: AtomicUsize::new(0),
        asr_empty: AtomicUsize::new(0),
        voice_us: RwLock::new(cfg.tts.voice_us.clone()),
        voice_gb: RwLock::new(cfg.tts.voice_gb.clone()),
        rate: RwLock::new(cfg.tts.rate.clone()),
        pitch: RwLock::new(cfg.tts.pitch.clone()),
        tts_tx: Mutex::new(None),
        events: ev_tx,
    });

    // ── 1. 麦克风源 ────────────────────────────────────────────
    let (raw_tx, raw_rx) = mpsc::channel::<Vec<f32>>();
    let mic_handle = mic.start(raw_tx)?;

    // ── 2. VAD 分段线程 ────────────────────────────────────────
    let (seg_tx, seg_rx) = mpsc::channel::<Vec<f32>>();
    {
        let state = state.clone();
        let vad_cfg = cfg.vad.clone();
        let sample_rate = mic_handle.sample_rate;
        std::thread::Builder::new()
            .name("vad".into())
            .spawn(move || {
                let mut resampler = Resampler::new(sample_rate, TARGET_SAMPLE_RATE);
                let mut vad = Vad::new(&vad_cfg);
                if vad_cfg.auto_calibrate {
                    vad.begin_calibration(1000);
                    let _ = state.events.send(EngineEvent::Calibrating);
                }
                let mut was_speaking = false;
                let mut was_paused = false;
                while let Ok(chunk) = raw_rx.recv() {
                    if !state.running.load(Ordering::Relaxed) {
                        break;
                    }
                    // 暂停门：暂停时丢弃输入并回到待机相位
                    let paused = state.paused.load(Ordering::Relaxed);
                    if paused != was_paused {
                        was_paused = paused;
                        match state.phase() {
                            Phase::Listening | Phase::Capturing | Phase::Idle => {
                                state.set_phase(if paused { Phase::Idle } else { Phase::Listening })
                            }
                            _ => {}
                        }
                    }
                    // 回声防护：播报期间丢弃输入
                    let allow = !paused
                        && !(state.echo_guard.load(Ordering::Relaxed)
                            && state.tts_playing.load(Ordering::Relaxed));
                    let mut pcm16k = Vec::new();
                    resampler.process(&chunk, &mut pcm16k);
                    // 用户灵敏度档位覆盖（>0 时强制阈值）
                    let ov = f32::from_bits(state.vad_threshold_override.load(Ordering::Relaxed));
                    if ov > 0.0 {
                        vad.set_threshold(ov);
                    }
                    let mut segments = Vec::new();
                    vad.process(&pcm16k, allow, &mut segments);
                    // 同步当前阈值给诊断 UI
                    state.vad_threshold.store(vad.threshold().to_bits(), Ordering::Relaxed);
                    let lv = if paused { 0.0 } else { vad.level() };
                    state.level.store(lv.to_bits(), Ordering::Relaxed);
                    if let Some(th) = vad.take_calibration_result() {
                        state.vad_threshold.store(th.to_bits(), Ordering::Relaxed);
                        let _ = state.events.send(EngineEvent::Calibrated(th));
                    }
                    let speaking = vad.is_speaking();
                    state.vad_speaking.store(speaking, Ordering::Relaxed);
                    if speaking != was_speaking {
                        was_speaking = speaking;
                        match (state.phase(), speaking) {
                            (Phase::Listening, true) | (Phase::Capturing, true) => {
                                state.set_phase(Phase::Capturing)
                            }
                            (Phase::Capturing, false) => state.set_phase(Phase::Listening),
                            _ => {}
                        }
                    }
                    for seg in segments {
                        state.segments_cut.fetch_add(1, Ordering::Relaxed);
                        let _ = seg_tx.send(seg);
                    }
                }
            })?;
    }

    // ── 3. 识别 + 翻译线程 ────────────────────────────────────
    let (tts_tx, mut tts_rx) = unbounded_channel::<TtsRequest>();
    *state.tts_tx.lock().unwrap() = Some(tts_tx.clone());
    {
        let state = state.clone();
        let asr_cfg = cfg.asr.clone();
        let engine_kind = translate::effective(&cfg.translate);
        let llm = LlmClient::new(&cfg.translate.llm)?;
        let rt = tokio::runtime::Handle::current();

        std::thread::Builder::new()
            .name("asr".into())
            .spawn(move || {
                let model_path = std::path::PathBuf::from(&asr_cfg.model_path);
                let mut asr = match Asr::load(&model_path, Some(asr_cfg.language.clone())) {
                    Ok(a) => a,
                    Err(e) => {
                        state.notice(NoticeKind::Error, format!("识别引擎初始化失败: {e:#}"));
                        return;
                    }
                };
                state.notice(
                    NoticeKind::Info,
                    format!("识别引擎就绪（whisper {}，引擎: {}）", asr_cfg.model, engine_kind.label()),
                );
                state.set_phase(Phase::Listening);

                while let Ok(seg) = seg_rx.recv() {
                    if !state.running.load(Ordering::Relaxed) {
                        break;
                    }
                    let t0 = Instant::now();
                    state.set_phase(Phase::Recognizing);

                    let zh: Option<String>;
                    let en: String;
                    let engine_used: &'static str;

                    match engine_kind {
                        translate::Engine::WhisperDirect => {
                            match asr.run(&seg, AsrMode::TranslateDirect) {
                                Ok(text) => {
                                    zh = None;
                                    en = text;
                                    engine_used = "whisper-direct";
                                }
                                Err(e) => {
                                    state.notice(NoticeKind::Error, format!("识别失败: {e:#}"));
                                    state.set_phase(Phase::Listening);
                                    continue;
                                }
                            }
                        }
                        translate::Engine::Llm => {
                            let text = match asr.run(&seg, AsrMode::Transcribe) {
                                Ok(t) => t,
                                Err(e) => {
                                    state.notice(NoticeKind::Error, format!("识别失败: {e:#}"));
                                    state.set_phase(Phase::Listening);
                                    continue;
                                }
                            };
                            if text.is_empty() {
                                state.set_phase(Phase::Listening);
                                continue;
                            }
                            state.set_phase(Phase::Translating);
                            let accent = state.accent();
                            match rt.block_on(llm.translate(accent, &text)) {
                                Ok(t) => {
                                    zh = Some(text);
                                    en = t;
                                    engine_used = "llm";
                                }
                                Err(e) => {
                                    state.notice(
                                        NoticeKind::Warn,
                                        format!("LLM 翻译失败，本句回退直译: {e:#}"),
                                    );
                                    match asr.run(&seg, AsrMode::TranslateDirect) {
                                        Ok(t) => {
                                            zh = Some(text);
                                            en = t;
                                            engine_used = "whisper-fallback";
                                        }
                                        Err(e2) => {
                                            state.notice(
                                                NoticeKind::Error,
                                                format!("直译亦失败: {e2:#}"),
                                            );
                                            state.set_phase(Phase::Listening);
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if en.trim().is_empty() {
                        state.asr_empty.fetch_add(1, Ordering::Relaxed);
                        state.set_phase(Phase::Listening);
                        continue;
                    }
                    state.asr_done.fetch_add(1, Ordering::Relaxed);

                    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                    let _ = state.events.send(EngineEvent::Utterance {
                        ts,
                        zh,
                        en: en.clone(),
                        engine: engine_used,
                    });
                    state.notice(
                        NoticeKind::Info,
                        format!("本句耗时 {:.2}s（{engine_used}）", t0.elapsed().as_secs_f32()),
                    );

                    let muted = state.muted.load(Ordering::Relaxed);
                    let tts_on = state.tts_enabled.load(Ordering::Relaxed);
                    if muted || !tts_on || en.trim().is_empty() {
                        state.set_phase(Phase::Listening);
                    } else {
                        state.set_phase(Phase::Speaking);
                        let voice = match state.accent() {
                            Accent::Us => state.voice_us(),
                            Accent::Gb => state.voice_gb(),
                        };
                        let _ = tts_tx.send(TtsRequest {
                            text: en,
                            voice,
                            rate: state.rate(),
                            pitch: state.pitch(),
                        });
                    }
                }
            })?;
    }

    // ── 4. TTS 播报任务（串行队列：合成 → 输出） ─────────────
    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(req) = tts_rx.recv().await {
                state.tts_playing.store(true, Ordering::Relaxed);

                match EdgeTts::synthesize(&req).await {
                    Ok(audio) => {
                        let s = sink.clone();
                        let played = tokio::task::spawn_blocking(move || s.play(audio)).await;
                        if let Err(e) = played {
                            state.notice(NoticeKind::Warn, format!("播放失败: {e:#}"));
                        } else if let Err(e) = played.unwrap() {
                            state.notice(NoticeKind::Warn, format!("播放失败: {e:#}"));
                        }
                    }
                    Err(e) => {
                        state.notice(NoticeKind::Warn, format!("语音合成失败: {e:#}"));
                    }
                }

                state.tts_playing.store(false, Ordering::Relaxed);
                if state.phase() == Phase::Speaking {
                    state.set_phase(Phase::Listening);
                }
            }
        });
    }

    Ok((state, ev_rx, mic_handle))
}

// ── 桌面实现：cpal 采集 + rodio 播放 ────────────────────────

#[cfg(all(feature = "desktop", not(target_os = "android")))]
pub mod desktop {
    use super::{AudioSink, Engine, MicHandle, MicSource, UnboundedReceiver};
    use crate::audio::capture;
    use crate::config::Config;
    use crate::tts::player::Player;
    use anyhow::{anyhow, Result};
    use std::sync::mpsc;
    use std::sync::Arc;

    pub struct CpalMic {
        device: Option<String>,
    }

    impl CpalMic {
        pub fn new(device: Option<String>) -> Self {
            Self { device }
        }
    }

    impl MicSource for CpalMic {
        fn start(self: Box<Self>, tx: mpsc::Sender<Vec<f32>>) -> Result<MicHandle> {
            let cap = capture::start(self.device.as_deref(), tx)?;
            Ok(MicHandle::new(cap.sample_rate).with_keep(Box::new(cap.stream)))
        }
    }

    pub struct RodioSink {
        player: Player,
    }

    impl RodioSink {
        pub fn new() -> Result<Self> {
            Ok(Self { player: Player::spawn()? })
        }
    }

    impl AudioSink for RodioSink {
        fn play(&self, mp3: Vec<u8>) -> Result<()> {
            self.player
                .play(mp3)
                .recv()
                .map_err(|_| anyhow!("播放失败"))
        }
    }

    /// 桌面便捷启动（保持原 pipeline::start 行为）
    pub fn start(cfg: Config) -> Result<(Engine, UnboundedReceiver<super::EngineEvent>)> {
        let sink = Arc::new(RodioSink::new()?);
        let mic = CpalMic::new(if cfg.audio.input_device.is_empty() {
            None
        } else {
            Some(cfg.audio.input_device.clone())
        });
        let (state, rx, mic_handle) = super::start_with(cfg, Box::new(mic), sink)?;
        Ok((
            Engine {
                _mic_keep: mic_handle.keep,
                state,
            },
            rx,
        ))
    }
}
