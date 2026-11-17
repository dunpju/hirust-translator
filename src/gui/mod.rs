//! eframe GUI 主界面（三区布局，见 docs/03-ui-design.md）

pub mod settings;
pub mod widgets;

use hirust_translator::config::{Accent, Config};
use hirust_translator::model_dl::{self, DlProgress};
use hirust_translator::pipeline::{self, EngineEvent, NoticeKind, Phase, SharedState};
use anyhow::{anyhow, Result};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

use widgets::*;

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub ts: String,
    pub zh: Option<String>,
    pub en: String,
}

struct Toast {
    deadline: f64,
    kind: NoticeKind,
    msg: String,
}

/// 面板交互统一延迟到面板闭包外执行（避免借用冲突）
pub enum Action {
    StartEngine,
    TogglePause,
    ClearHistory,
    ToggleMuted,
    ToggleEcho,
    SetAccent(Accent),
    Speak(String),
    PreviewVoice { voice: String, rate: String, pitch: String },
    BeginDownload,
    SaveSettings,
    ResetSettings,
}

struct DlUi {
    total: Option<u64>,
    got: u64,
    finished: bool,
    ok: bool,
    err: Option<String>,
}

pub struct TranslatorApp {
    rt: tokio::runtime::Runtime,
    cfg: Config,
    cfg_path: PathBuf,
    pub cfg_edit: Config,
    engine: Option<pipeline::Engine>,
    pub state: Option<Arc<SharedState>>,
    events: Option<UnboundedReceiver<EngineEvent>>,
    history: Vec<HistoryItem>,
    last_en: Option<String>,
    notices: Vec<Toast>,
    meter_disp: f32,
    pub show_settings: bool,
    model_ready: bool,
    dl: Option<Arc<Mutex<DlUi>>>,
    pub action: Option<Action>,
}

impl TranslatorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config, cfg_path: PathBuf) -> Self {
        install_fonts(&cc.egui_ctx);
        let mut vis = egui::Visuals::dark();
        vis.panel_fill = COL_BG;
        cc.egui_ctx.set_visuals(vis);

        let model_ready = model_dl::is_model_ok(&cfg.asr.model, &cfg.model_path());
        let cfg_edit = cfg.clone();
        Self {
            rt: tokio::runtime::Runtime::new().expect("创建异步运行时失败"),
            cfg,
            cfg_path,
            cfg_edit,
            engine: None,
            state: None,
            events: None,
            history: Vec::new(),
            last_en: None,
            notices: Vec::new(),
            meter_disp: 0.0,
            show_settings: false,
            model_ready,
            dl: None,
            action: None,
        }
    }

    // ── 事件与通知 ────────────────────────────────────────────

    fn drain_events(&mut self, ctx: &egui::Context) {
        let now = ctx.time();
        self.notices.retain(|t| t.deadline > now);
        let Some(rx) = self.events.as_mut() else { return };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                EngineEvent::Utterance { ts, zh, en, .. } => {
                    self.last_en = Some(en.clone());
                    self.history.push(HistoryItem { ts, zh, en });
                    if self.history.len() > 300 {
                        self.history.remove(0);
                    }
                }
                EngineEvent::Notice(kind, msg) => {
                    if kind != NoticeKind::Info {
                        // Info 类碎消息不弹条（如每句耗时）
                        self.notices.push(Toast { deadline: now + 6.0, kind, msg });
                        if self.notices.len() > 5 {
                            self.notices.remove(0);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ── 面板 ──────────────────────────────────────────────────

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top-bar").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                dot(ui, COL_LISTEN);
                ui.heading("Hirust 中英同声传译");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let accent = self.state.as_ref().map(|s| s.accent()).unwrap_or(self.cfg.tts.accent);
                    let badge = match accent {
                        Accent::Us => "🇺🇸 美式",
                        Accent::Gb => "🇬🇧 英式",
                    };
                    if ui.button(badge).clicked() {
                        let next = match accent {
                            Accent::Us => Accent::Gb,
                            Accent::Gb => Accent::Us,
                        };
                        self.action = Some(Action::SetAccent(next));
                    }
                    let phase = self.state.as_ref().map(|s| s.phase());
                    let paused = self.state.as_ref().map(|s| s.paused.load(Ordering::Relaxed)).unwrap_or(false);
                    status_pill(ui, phase, paused);
                });
            });
            ui.add_space(6.0);
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("bottom-bar").show(ui, |ui| {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                let engine_on = self.engine.is_some();
                let paused = self.state.as_ref().map(|s| s.paused.load(Ordering::Relaxed)).unwrap_or(false);
                let (label, fill) = if !engine_on {
                    ("●  开始录音", COL_LISTEN)
                } else if paused {
                    ("▶  继续录音", COL_LISTEN)
                } else {
                    ("■  暂停录音", COL_ERR)
                };
                let btn = egui::Button::new(egui::RichText::new(label).size(18.0).color(egui::Color32::WHITE))
                    .fill(fill)
                    .min_size(egui::vec2(200.0, 46.0));
                let enabled = self.model_ready;
                let mut resp = ui.add_enabled(enabled, btn);
                if !self.model_ready {
                    resp = resp.on_disabled_hover_text("识别模型未就绪，请先完成首次下载");
                }
                if resp.clicked() {
                    self.action = Some(if !engine_on { Action::StartEngine } else { Action::TogglePause });
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                let accent = self.state.as_ref().map(|s| s.accent()).unwrap_or(self.cfg.tts.accent);
                ui.label("口音:");
                let mut sel = accent;
                ui.selectable_value(&mut sel, Accent::Us, "美式");
                ui.selectable_value(&mut sel, Accent::Gb, "英式");
                if sel != accent {
                    self.action = Some(Action::SetAccent(sel));
                }
                ui.separator();
                let muted = self.state.as_ref().map(|s| s.muted.load(Ordering::Relaxed)).unwrap_or(false);
                if ui
                    .selectable_label(muted, if muted { "🔇 已静音" } else { "🔊 自动播报" })
                    .clicked()
                {
                    self.action = Some(Action::ToggleMuted);
                }
                let echo = self.state.as_ref().map(|s| s.echo_guard.load(Ordering::Relaxed)).unwrap_or(self.cfg.ui.echo_guard);
                if ui
                    .selectable_label(echo, if echo { "🛡 回声防护:开" } else { "🎧 耳机模式" })
                    .clicked()
                {
                    self.action = Some(Action::ToggleEcho);
                }
                ui.separator();
                if ui.button("🗑 清空记录").clicked() {
                    self.action = Some(Action::ClearHistory);
                }
                if ui.selectable_label(self.show_settings, "⚙ 设置").clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
            ui.add_space(4.0);
        });
    }

    fn central(&mut self, ctx: &egui::Context, root_ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(root_ui, |ui| {
            // 通知条
            let now = ctx.time();
            let mut expired = false;
            for t in &self.notices {
                if t.deadline <= now {
                    expired = true;
                    continue;
                }
                let (col, prefix) = match t.kind {
                    NoticeKind::Warn => (COL_WARN, "⚠"),
                    NoticeKind::Error => (COL_ERR, "✗"),
                    NoticeKind::Info => (COL_DIM, "ℹ"),
                };
                egui::Frame::default()
                    .fill(COL_CARD)
                    .stroke(egui::Stroke::new(1.0, col))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.colored_label(col, format!("{prefix} {}", t.msg));
                    });
            }
            if expired {
                self.notices.retain(|t| t.deadline > now);
            }

            // 电平条
            let level = self.state.as_ref().map(|s| s.level()).unwrap_or(0.0);
            let active = self.engine.is_some()
                && !self.state.as_ref().map(|s| s.paused.load(Ordering::Relaxed)).unwrap_or(false);
            level_meter(ui, &mut self.meter_disp, level, active);

            ui.add_space(4.0);

            // 记录区
            let mut replay: Option<String> = None;
            let mut copy: Option<String> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.history.is_empty() {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.set_width(ui.available_width());
                            ui.add_space(30.0);
                            ui.colored_label(
                                COL_DIM,
                                if self.engine.is_some() {
                                    "对着麦克风说中文，停顿约 0.8 秒即出译文"
                                } else if self.model_ready {
                                    "点击「开始录音」开始同传"
                                } else {
                                    "首次运行需下载识别模型（见弹窗）"
                                },
                            );
                        });
                    }
                    for item in &self.history {
                        egui::Frame::default()
                            .fill(COL_CARD)
                            .stroke(egui::Stroke::new(1.0, COL_LINE))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.colored_label(COL_DIM, &item.ts);
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("🔊 重播").clicked() {
                                            replay = Some(item.en.clone());
                                        }
                                        if ui.small_button("📋 复制").clicked() {
                                            copy = Some(item.en.clone());
                                        }
                                    });
                                });
                                if let Some(zh) = &item.zh {
                                    ui.colored_label(COL_ZH, format!("zh  {zh}"));
                                }
                                ui.colored_label(COL_EN, format!("en  {}", item.en));
                            });
                        ui.add_space(6.0);
                    }
                    // 进行中指示
                    if let Some(st) = &self.state {
                        let phase = st.phase();
                        let dots = ".".repeat(((now * 2.0) as usize % 3) + 1);
                        match phase {
                            Phase::Capturing => {
                                ui.colored_label(COL_LISTEN, "正在听你说…");
                            }
                            Phase::Recognizing => {
                                ui.colored_label(COL_RECOG, format!("识别中{dots}"));
                            }
                            Phase::Translating => {
                                ui.colored_label(COL_TRANS, format!("翻译中{dots}"));
                            }
                            Phase::Speaking => {
                                ui.colored_label(COL_SPEAK, "🔊 播报中…");
                            }
                            _ => {}
                        }
                    }
                });
            if let Some(text) = replay {
                self.action = Some(Action::Speak(text));
            }
            if let Some(text) = copy {
                ui.ctx()
                    .output_mut(|o| o.commands.push(egui::OutputCommand::CopyText(text)));
            }
        });
    }

    fn first_run_modal(&mut self, ctx: &egui::Context) {
        if self.model_ready {
            return;
        }
        let mut open = true;
        egui::Window::new("首次运行 · 需要下载识别模型")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .min_width(400.0)
            .show(ctx, |ui| {
                let size = model_dl::expected_size(&self.cfg.asr.model);
                ui.label(format!(
                    "语音识别模型尚未就绪\n{} · 约 {} · 仅下载一次",
                    model_dl::model_file_name(&self.cfg.asr.model),
                    size.map(|s| format!("{:.0} MB", s as f64 / 1e6)).unwrap_or("?".into()),
                ));
                ui.add_space(8.0);
                match self.dl.as_ref() {
                    None => {
                        ui.weak("下载源：hf-mirror.com（国内镜像优先，HuggingFace 兜底）");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("开始下载").clicked() {
                                self.action = Some(Action::BeginDownload);
                            }
                            if ui.button("退出").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    }
                    Some(st) => {
                        let g = st.lock().unwrap();
                        if g.finished {
                            if g.ok {
                                ui.colored_label(COL_LISTEN, "✓ 模型已就绪");
                            } else {
                                ui.colored_label(COL_ERR, format!("下载失败: {}", g.err.clone().unwrap_or_default()));
                                ui.weak(format!("手动下载: {}", model_dl::download_urls(&self.cfg.asr.model).join("  或  ")));
                                if ui.button("重试").clicked() {
                                    self.action = Some(Action::BeginDownload);
                                }
                            }
                        } else {
                            let frac = g.total.map(|t| g.got as f32 / t as f32).unwrap_or(0.0);
                            let pb = egui::ProgressBar::new(frac).text(format!(
                                "{:.1} / {} MB",
                                g.got as f64 / 1e6,
                                g.total
                                    .map(|t| format!("{:.0}", t as f64 / 1e6))
                                    .unwrap_or("?".into()),
                            ));
                            ui.add(pb);
                            ui.colored_label(COL_DIM, "正在下载…");
                        }
                    }
                }
            });
        // 下载成功后关闭弹窗
        if let Some(st) = &self.dl {
            let ok = { st.lock().unwrap().finished && st.lock().unwrap().ok };
            if ok {
                self.model_ready = true;
                self.dl = None;
            }
        }
        let _ = open;
    }

    // ── 动作执行 ──────────────────────────────────────────────

    fn start_engine(&mut self) {
        let cfg = self.cfg.clone();
        let _guard = self.rt.enter();
        match pipeline::desktop::start(cfg) {
            Ok((engine, rx)) => {
                self.state = Some(engine.state());
                self.engine = Some(engine);
                self.events = Some(rx);
            }
            Err(e) => {
                let msg = format!("引擎启动失败: {e:#}（可用 cli devices 查看输入设备）");
                self.push_err_local(msg);
            }
        }
    }

    fn push_err_local(&mut self, msg: String) {
        self.notices.push(Toast { deadline: f64::MAX, kind: NoticeKind::Error, msg });
    }

    fn apply_action(&mut self, ctx: &egui::Context) {
        // 立即错误通知需要 ctx.time；deadline 用 now+8s
        let now = ctx.time();
        self.notices.iter_mut().for_each(|t| {
            if t.deadline == f64::MAX {
                t.deadline = now + 8.0;
            }
        });
        match self.action.take() {
            Some(Action::StartEngine) => {
                if self.engine.is_none() && self.model_ready {
                    self.start_engine();
                }
            }
            Some(Action::TogglePause) => {
                if let Some(st) = &self.state {
                    let p = !st.paused.load(Ordering::Relaxed);
                    st.paused.store(p, Ordering::Relaxed);
                }
            }
            Some(Action::ClearHistory) => {
                self.history.clear();
                self.last_en = None;
            }
            Some(Action::ToggleMuted) => {
                if let Some(st) = &self.state {
                    let m = !st.muted.load(Ordering::Relaxed);
                    st.muted.store(m, Ordering::Relaxed);
                }
            }
            Some(Action::ToggleEcho) => {
                if let Some(st) = &self.state {
                    let g = !st.echo_guard.load(Ordering::Relaxed);
                    st.echo_guard.store(g, Ordering::Relaxed);
                    self.cfg_edit.ui.echo_guard = g;
                }
            }
            Some(Action::SetAccent(a)) => {
                if let Some(st) = &self.state {
                    st.set_accent(a);
                }
                self.cfg_edit.tts.accent = a;
                self.cfg.tts.accent = a;
            }
            Some(Action::Speak(text)) => {
                if let Some(st) = &self.state {
                    if !st.speak(text) {
                        self.push_err_local("播报入队失败（引擎未就绪）".into());
                    }
                }
            }
            Some(Action::PreviewVoice { voice, rate, pitch }) => {
                if let Some(st) = &self.state {
                    let _ = st.speak_with(voice, rate, pitch, "This is a preview of the selected voice.".into());
                }
            }
            Some(Action::BeginDownload) => {
                let st = Arc::new(Mutex::new(DlUi { total: None, got: 0, finished: false, ok: false, err: None }));
                self.dl = Some(st.clone());
                let model = self.cfg.asr.model.clone();
                let path = self.cfg.model_path();
                let st_cb = st.clone();
                self.rt.spawn(async move {
                    let r = model_dl::ensure_model(&model, &path, move |ev: DlProgress| {
                        let mut g = st_cb.lock().unwrap();
                        match ev {
                            DlProgress::Start { total, .. } => g.total = total,
                            DlProgress::Bytes(n) => g.got = n,
                            DlProgress::Done => {
                                g.finished = true;
                                g.ok = true;
                            }
                        }
                    })
                    .await;
                    if let Err(e) = r {
                        let mut g = st.lock().unwrap();
                        g.finished = true;
                        g.ok = false;
                        g.err = Some(format!("{e:#}"));
                    }
                });
            }
            Some(Action::SaveSettings) => {
                match toml::to_string_pretty(&self.cfg_edit) {
                    Ok(text) => {
                        if let Err(e) = std::fs::write(&self.cfg_path, text) {
                            self.push_err_local(format!("保存配置失败: {e}"));
                        } else {
                            if let Some(st) = &self.state {
                                st.set_voice_params(
                                    self.cfg_edit.tts.voice_us.clone(),
                                    self.cfg_edit.tts.voice_gb.clone(),
                                    self.cfg_edit.tts.rate.clone(),
                                    self.cfg_edit.tts.pitch.clone(),
                                );
                                st.echo_guard.store(self.cfg_edit.ui.echo_guard, Ordering::Relaxed);
                                st.tts_enabled.store(self.cfg_edit.tts.enabled, Ordering::Relaxed);
                            }
                            self.cfg = self.cfg_edit.clone();
                        }
                    }
                    Err(e) => self.push_err_local(format!("序列化配置失败: {e}")),
                }
            }
            Some(Action::ResetSettings) => {
                self.cfg_edit = Config::default();
            }
            None => {}
        }
    }

    fn apply_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Comma)) {
            self.show_settings = !self.show_settings;
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return; // 正在编辑文本框
        }
        let (space, one, two, m, e, r) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Num1),
                i.key_pressed(egui::Key::Num2),
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::E),
                i.key_pressed(egui::Key::R),
            )
        });
        let engine_on = self.engine.is_some();
        if space && self.model_ready {
            self.action = Some(if engine_on { Action::TogglePause } else { Action::StartEngine });
        }
        if one {
            self.action = Some(Action::SetAccent(Accent::Us));
        }
        if two {
            self.action = Some(Action::SetAccent(Accent::Gb));
        }
        if m {
            self.action = Some(Action::ToggleMuted);
        }
        if e {
            self.action = Some(Action::ToggleEcho);
        }
        if r {
            if let Some(t) = self.last_en.clone() {
                self.action = Some(Action::Speak(t));
            }
        }
    }
}

impl eframe::App for TranslatorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.apply_shortcuts(&ctx);
        self.top_bar(ui);
        if self.show_settings {
            settings::panel(self, ui);
        }
        self.bottom_bar(ui);
        self.central(&ctx, ui);
        self.first_run_modal(&ctx);
        self.apply_action(&ctx);
        // 持续重绘（电平表/进行中动画）
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

impl Drop for TranslatorApp {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.stop();
        }
    }
}

/// 加载中文字体（egui 默认不含 CJK 字形）
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("cjk".into());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// 启动 GUI
pub fn run(cfg: Config, cfg_path: PathBuf) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hirust 中英同声传译")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "hirust-translator",
        options,
        Box::new(move |cc| Ok(Box::new(TranslatorApp::new(cc, cfg, cfg_path)))),
    )
    .map_err(|e| anyhow!("GUI 运行失败: {e}"))
}
