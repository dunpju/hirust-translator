//! Hirust 中英同声传译
//!
//! 用法：
//!   hirust-translator              启动图形界面（默认）
//!   hirust-translator gui          同上
//!   hirust-translator cli run      启动实时翻译（headless）
//!   hirust-translator cli devices  列出音频输入设备
//!   hirust-translator cli download [tiny|base|small]   下载/更新识别模型
//!   hirust-translator cli say <文本> [--gb]            TTS 试音（默认美式）
//!   hirust-translator cli config   查看配置
//!
//! CLI 运行时命令：q 退出 · us 美式 · gb 英式 · m 静音 · e 回声防护 · ? 帮助

mod gui;

use anyhow::{Context, Result};
use colored::Colorize;
use hirust_translator::config;
use hirust_translator::model_dl::{self, ensure_model, DlProgress};
use hirust_translator::pipeline::{self, EngineEvent, NoticeKind, Phase};
use hirust_translator::tts::{self, player::Player, EdgeTts, TtsRequest};
use hirust_translator::{audio, translate};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use config::{Accent, Config};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 配置文件位置：优先 exe 同目录，否则当前目录（并在缺失时生成默认模板）
fn config_file() -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        let p = dir.join("config.toml");
        if p.exists() {
            return p;
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.toml")
}

fn load_config() -> Result<Config> {
    let path = config_file();
    let mut cfg = Config::load_or_create(&path)?;
    // 相对模型路径基于配置文件所在目录解析
    let mp = PathBuf::from(&cfg.asr.model_path);
    if mp.is_relative() {
        if let Some(base) = path.parent() {
            cfg.asr.model_path = base.join(mp).to_string_lossy().into_owned();
        }
    }
    Ok(cfg)
}

fn setup_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
    let _ = colored::control::set_virtual_terminal(true);
}

fn print_usage() {
    println!(
        "{} v{VERSION}

{}",
        "Hirust 中英同声传译".bold(),
        "用法:
  hirust-translator                启动图形界面（默认）
  hirust-translator cli [run]      启动实时翻译（headless）
  hirust-translator cli devices    列出音频输入设备
  hirust-translator cli download [tiny|base|small]  下载/更新识别模型
  hirust-translator cli say <文本> [--gb]           TTS 试音
  hirust-translator cli config     查看配置

CLI 运行时命令:
  q 退出 · us 美式 · gb 英式 · m 静音播报 · e 回声防护 · ? 帮助

GUI 快捷键:
  Space 开始/暂停 · 1/2 美式/英式 · M 静音 · E 回声防护 · R 重播 · Ctrl+, 设置"
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("gui") => gui::run(load_config()?, config_file()),
        Some("cli") => cli_main(&args[1..]).await,
        Some("help") | Some("-h") | Some("--help") => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            anyhow::bail!("未知命令: {other}（CLI 子命令请加 cli 前缀）")
        }
    }
}

async fn cli_main(rest: &[String]) -> Result<()> {
    match rest.first().map(String::as_str) {
        None | Some("run") => cmd_run().await,
        Some("devices") => cmd_devices(),
        Some("download") => cmd_download(rest.get(1).map(String::as_str)).await,
        Some("say") => cmd_say(&rest[1..]).await,
        Some("config") => cmd_config(),
        Some("help") | Some("-h") | Some("--help") => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            print_usage();
            anyhow::bail!("未知命令: {other}")
        }
    }
}

// ── 子命令 ──────────────────────────────────────────────────

fn cmd_devices() -> Result<()> {
    let devices = audio::capture::list_devices().context("枚举音频输入设备失败")?;
    if devices.is_empty() {
        println!("{}", "未发现任何输入设备".yellow());
        return Ok(());
    }
    println!("{}（config.toml → audio.input_device 填写名称）", "输入设备:".bold());
    for (i, d) in devices.iter().enumerate() {
        println!("  {}. {d}", i + 1);
    }
    Ok(())
}

async fn cmd_download(model_arg: Option<&str>) -> Result<()> {
    let cfg = load_config()?;
    let model = model_arg.unwrap_or(cfg.asr.model.as_str()).to_string();
    // 指定了与配置不同的模型时，放到同一目录下的 ggml-<model>.bin
    let path = if model == cfg.asr.model {
        cfg.model_path()
    } else {
        cfg.model_path()
            .parent()
            .map(|p| p.join(model_dl::model_file_name(&model)))
            .unwrap_or_else(|| cfg.model_path())
    };
    download_with_progress(&model, &path).await?;
    if model_arg.is_some() && model != cfg.asr.model {
        println!(
            "{}",
            format!("提示：在 config.toml 中设置 asr.model = \"{model}\" 与对应 model_path 后启用该模型").yellow()
        );
    }
    Ok(())
}

async fn download_with_progress(model: &str, path: &std::path::Path) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};

    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} {msg}\n{bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
            .progress_chars("##-"),
    );
    let m = model.to_string();
    ensure_model(model, path, move |ev| match ev {
        DlProgress::Start { total, url } => {
            bar.set_length(total.unwrap_or(0));
            bar.set_message(format!("下载 {m} ← {url}"));
        }
        DlProgress::Bytes(n) => bar.set_position(n),
        DlProgress::Done => bar.finish_with_message("模型已就绪".to_string()),
    })
    .await?;
    if path.exists() {
        println!("{} {}", "✓ 模型就绪:".green(), path.display());
    }
    Ok(())
}

async fn cmd_say(args: &[String]) -> Result<()> {
    let cfg = load_config()?;
    let gb = args.iter().any(|a| a == "--gb" || a == "-gb" || a == "--british");
    let text = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        anyhow::bail!("用法: hirust-translator say <文本> [--gb]");
    }
    let accent = if gb { Accent::Gb } else { cfg.tts.accent };
    let req = TtsRequest {
        text,
        voice: cfg.voice_for(accent).to_string(),
        rate: cfg.tts.rate.clone(),
        pitch: cfg.tts.pitch.clone(),
    };
    println!("{} {}（{}）", "合成中:".dimmed(), req.text, accent.label());
    let t0 = std::time::Instant::now();
    let bytes = EdgeTts::synthesize(&req).await?;
    println!("{} {:.0}ms，{} 字节", "合成完成:".dimmed(), t0.elapsed().as_millis(), bytes.len());
    let player = Player::spawn()?;
    player
        .play(bytes)
        .recv()
        .map_err(|_| anyhow::anyhow!("播放失败"))?;
    Ok(())
}

fn cmd_config() -> Result<()> {
    let cfg = load_config()?;
    let path = config_file();
    let engine = translate::effective(&cfg.translate);
    println!("{}", "Hirust 配置概览".bold());
    println!("  配置文件 : {}", path.display());
    println!("  识别模型 : {} → {}", cfg.asr.model, cfg.asr.model_path);
    println!("  识别语言 : {}", cfg.asr.language);
    println!(
        "  翻译引擎 : {}",
        match engine {
            translate::Engine::Llm => {
                format!("LLM（{} @ {}）", cfg.translate.llm.model, cfg.translate.llm.base_url)
            }
            translate::Engine::WhisperDirect => {
                "Whisper 直译（离线；配置 api_key 后自动启用 LLM）".to_string()
            }
        }
    );
    println!(
        "  API Key  : {}",
        if cfg.translate.llm.api_key.is_empty() { "未配置".to_string().dimmed().to_string() } else { "已配置".green().to_string() }
    );
    println!("  默认口音 : {}（{}）", cfg.tts.accent.label(), cfg.voice_for(cfg.tts.accent));
    println!("  美式音色 : {}", cfg.tts.voice_us);
    println!("  英式音色 : {}", cfg.tts.voice_gb);
    println!(
        "  回声防护 : {}",
        if cfg.ui.echo_guard { "开（外放模式；戴耳机可在 config.toml 关闭）" } else { "关（耳机模式）" }
    );
    Ok(())
}

async fn cmd_run() -> Result<()> {
    let cfg = load_config()?;
    println!("{}", format!("Hirust 中英同声传译 v{VERSION}").bold());
    println!("{}", "─".repeat(56).dimmed());

    // 确保模型就绪（首次自动下载）
    if !model_dl::is_model_ok(&cfg.asr.model, &cfg.model_path()) {
        println!("{}", "首次运行：准备识别模型…".yellow());
        let (m, p) = (cfg.asr.model.clone(), cfg.model_path());
        download_with_progress(&m, &p).await?;
    }

    let engine = translate::effective(&cfg.translate);
    println!("  识别模型 : {}", model_dl::model_file_name(&cfg.asr.model));
    println!(
        "  翻译引擎 : {}",
        match engine {
            translate::Engine::Llm => format!("LLM（{}）", cfg.translate.llm.model),
            translate::Engine::WhisperDirect => "Whisper 直译（离线）".to_string(),
        }
    );
    println!("  口音     : {} → {}", cfg.tts.accent.label(), cfg.voice_for(cfg.tts.accent));
    println!(
        "  回声防护 : {}",
        if cfg.ui.echo_guard { "开" } else { "关" }
    );
    println!("{}", "─".repeat(56).dimmed());
    println!("{}", "对着麦克风说中文即可。命令: q 退出 · us 美式 · gb 英式 · m 静音 · e 回声防护 · ? 帮助".dimmed());

    let (engine, mut rx) = match pipeline::desktop::start(cfg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", format!("启动失败: {e:#}").red());
            eprintln!("{}", "可用 `hirust-translator devices` 查看输入设备".yellow());
            return Err(e);
        }
    };
    let state = engine.state();

    // stdin 命令线程
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if cmd_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut quit = false;
    let mut cmd_alive = true; // stdin 关闭后禁用该分支，避免 select 空转
    while !quit {
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(e) => print_event(&e),
                None => break,
            },
            cmd = async {
                if cmd_alive {
                    cmd_rx.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                match cmd {
                    Some(c) => match c.trim() {
                        "q" | "quit" | "exit" => quit = true,
                        "us" => {
                            state.set_accent(Accent::Us);
                            println!("{}", "口音 → 美式".cyan());
                        }
                        "gb" => {
                            state.set_accent(Accent::Gb);
                            println!("{}", "口音 → 英式".cyan());
                        }
                        "m" => {
                            let m = !state.muted.load(Ordering::Relaxed);
                            state.muted.store(m, Ordering::Relaxed);
                            println!(
                                "{}",
                                if m { "自动播报已静音（m 恢复）".yellow() } else { "自动播报已恢复".cyan() }
                            );
                        }
                        "e" => {
                            let g = !state.echo_guard.load(Ordering::Relaxed);
                            state.echo_guard.store(g, Ordering::Relaxed);
                            println!(
                                "{}",
                                if g { "回声防护：开（外放模式）".cyan() } else { "回声防护：关（耳机模式）".cyan() }
                            );
                        }
                        "" => {}
                        "?" | "help" => println!(
                            "{}",
                            "命令: q 退出 · us 美式 · gb 英式 · m 静音 · e 回声防护".dimmed()
                        ),
                        other => println!("{}", format!("未知命令: {other}（? 查看帮助）").yellow()),
                    },
                    None => cmd_alive = false, // stdin 已关闭
                }
            }
            _ = tokio::signal::ctrl_c() => quit = true,
        }
    }
    println!("{}", "正在停止…".dimmed());
    engine.stop();
    Ok(())
}

fn print_event(e: &EngineEvent) {
    match e {
        EngineEvent::Phase(p) => match p {
            Phase::Recognizing => println!("{}", "… 识别中".dimmed()),
            Phase::Translating => println!("{}", "… 翻译中".dimmed()),
            _ => {}
        },
        EngineEvent::Calibrating => {
            println!("{}", "环境噪声校准中，请保持安静 1 秒…".dimmed())
        }
        EngineEvent::Calibrated(th) => {
            println!("{}", format!("校准完成，触发阈值 = {th:.4}").dimmed())
        }
        EngineEvent::Utterance { ts, zh, en, .. } => {
            if let Some(zh) = zh {
                println!("{} {}", format!("[{ts}]").dimmed(), format!("zh {zh}").yellow());
            }
            println!("{} {}", format!("[{ts}]").dimmed(), format!("en {en}").cyan());
        }
        EngineEvent::Notice(kind, msg) => {
            let line = match kind {
                NoticeKind::Info => msg.dimmed(),
                NoticeKind::Warn => format!("⚠ {msg}").yellow(),
                NoticeKind::Error => format!("✗ {msg}").red(),
            };
            println!("{line}");
        }
    }
}
