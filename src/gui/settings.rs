//! 设置面板（右侧滑出）：识别 / 翻译 / 语音合成 / 断句 / 界面

use super::{Action, TranslatorApp};
use eframe::egui;

pub fn panel(app: &mut TranslatorApp, root_ui: &mut egui::Ui) {
    egui::Panel::right("settings")
        .resizable(false)
        .default_size(360.0)
        .show(root_ui, |ui| {
            ui.add_space(8.0);
            ui.heading("⚙ 设置");
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // ── 识别 ──
                    egui::CollapsingHeader::new("识别（重启生效）")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("模型:");
                                egui::ComboBox::from_id_salt("model-box")
                                    .selected_text(app.cfg_edit.asr.model.clone())
                                    .show_ui(ui, |ui| {
                                        for m in ["tiny", "base", "small"] {
                                            ui.selectable_value(
                                                &mut app.cfg_edit.asr.model,
                                                m.to_string(),
                                                m,
                                            );
                                        }
                                    });
                            });
                            ui.add_enabled(
                                false,
                                egui::Button::new(format!("当前: {}", app.cfg.asr.model)),
                            );
                            ui.weak(format!(
                                "路径: {}",
                                app.cfg.asr.model_path
                            ));
                        });

                    ui.add_space(6.0);
                    // ── 翻译 ──
                    egui::CollapsingHeader::new("翻译（引擎与 Key 重启生效）")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.radio_value(&mut app.cfg_edit.translate.engine, "llm".into(), "LLM API（推荐，需 Key）");
                            ui.radio_value(&mut app.cfg_edit.translate.engine, "whisper".into(), "Whisper 直译（离线）");
                            ui.add_space(4.0);
                            egui::Grid::new("llm-grid")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("接口地址:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.translate.llm.base_url);
                                    ui.end_row();
                                    ui.label("API Key:");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut app.cfg_edit.translate.llm.api_key)
                                            .password(true)
                                            .hint_text("sk-..."),
                                    );
                                    ui.end_row();
                                    ui.label("模型:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.translate.llm.model);
                                    ui.end_row();
                                });
                        });

                    ui.add_space(6.0);
                    // ── 语音合成 ──
                    egui::CollapsingHeader::new("语音合成（保存后立即生效）")
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Grid::new("tts-grid")
                                .num_columns(2)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("美式音色:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.tts.voice_us);
                                    ui.end_row();
                                    ui.label("英式音色:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.tts.voice_gb);
                                    ui.end_row();
                                    ui.label("语速:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.tts.rate);
                                    ui.end_row();
                                    ui.label("音调:");
                                    ui.text_edit_singleline(&mut app.cfg_edit.tts.pitch);
                                    ui.end_row();
                                });
                            ui.add_space(4.0);
                            let preview_ready = app.state.is_some();
                            if ui
                                .add_enabled(preview_ready, egui::Button::new("🔊 试听（美式音色）"))
                                .on_disabled_hover_text("先开始录音启动引擎")
                                .clicked()
                            {
                                app.action = Some(Action::PreviewVoice {
                                    voice: app.cfg_edit.tts.voice_us.clone(),
                                    rate: app.cfg_edit.tts.rate.clone(),
                                    pitch: app.cfg_edit.tts.pitch.clone(),
                                });
                            }
                            ui.weak("常用: en-US-AriaNeural / en-US-GuyNeural / en-GB-SoniaNeural / en-GB-RyanNeural");
                        });

                    ui.add_space(6.0);
                    // ── 断句 ──
                    egui::CollapsingHeader::new("断句 VAD（重启生效）")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.checkbox(&mut app.cfg_edit.vad.auto_calibrate, "启动时自动校准噪声");
                            ui.horizontal(|ui| {
                                ui.label("静音断句:");
                                ui.add(
                                    egui::Slider::new(&mut app.cfg_edit.vad.silence_ms, 300..=2000)
                                        .suffix(" ms"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("手动阈值:");
                                ui.add(
                                    egui::DragValue::new(&mut app.cfg_edit.vad.energy_threshold)
                                        .speed(0.001)
                                        .range(0.002..=0.1),
                                );
                            });
                        });

                    ui.add_space(6.0);
                    // ── 界面 ──
                    egui::CollapsingHeader::new("界面")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.checkbox(&mut app.cfg_edit.ui.echo_guard, "回声防护（外放模式；戴耳机可关闭）");
                            ui.checkbox(&mut app.cfg_edit.tts.enabled, "自动播报译文");
                        });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("恢复默认").clicked() {
                            app.action = Some(Action::ResetSettings);
                        }
                        if ui
                            .button("💾 保存")
                            .on_hover_text("音色/播报/防护立即生效；引擎与 VAD 重启生效")
                            .clicked()
                        {
                            app.action = Some(Action::SaveSettings);
                        }
                    });
                    ui.add_space(10.0);
                });
        });
}
