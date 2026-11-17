//! GUI 小部件：状态胶囊、电平表、通用配色

use crate::pipeline::Phase;
use eframe::egui::{self, Color32, Sense};

pub const COL_BG: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x24);
pub const COL_CARD: Color32 = Color32::from_rgb(0x26, 0x27, 0x2e);
pub const COL_LINE: Color32 = Color32::from_rgb(0x33, 0x35, 0x3d);
pub const COL_DIM: Color32 = Color32::from_rgb(0x8a, 0x8f, 0x98);
pub const COL_ZH: Color32 = Color32::from_rgb(0xe5, 0xe7, 0xeb);
pub const COL_EN: Color32 = Color32::from_rgb(0x67, 0xe8, 0xf9);
pub const COL_IDLE: Color32 = Color32::from_rgb(0x8a, 0x8f, 0x98);
pub const COL_LISTEN: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const COL_RECOG: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
pub const COL_TRANS: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
pub const COL_SPEAK: Color32 = Color32::from_rgb(0xa8, 0x55, 0xf7);
pub const COL_WARN: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
pub const COL_ERR: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);

pub fn phase_color(p: Phase) -> Color32 {
    match p {
        Phase::Idle => COL_IDLE,
        Phase::Listening | Phase::Capturing => COL_LISTEN,
        Phase::Recognizing => COL_RECOG,
        Phase::Translating => COL_TRANS,
        Phase::Speaking => COL_SPEAK,
    }
}

/// 顶部状态胶囊：● + 文案（颜色随相位）
pub fn status_pill(ui: &mut egui::Ui, phase: Option<Phase>, paused: bool) {
    egui::Frame::default()
        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 0))
        .show(ui, |ui| match phase {
            Some(p) => {
                let (color, text) = if paused {
                    (COL_IDLE, "已暂停")
                } else {
                    (phase_color(p), p.desc())
                };
                dot(ui, color);
                ui.colored_label(color, text);
            }
            None => {
                dot(ui, COL_DIM);
                ui.colored_label(COL_DIM, "未启动");
            }
        });
}

pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 4.5, color);
}

/// 水平分段电平条（dB 刻度 -60..0，含峰值缓释）
pub fn level_meter(ui: &mut egui::Ui, disp: &mut f32, level: f32, active: bool) {
    let db = 20.0 * level.max(1e-8).log10();
    let target = if active { ((db + 60.0) / 60.0).clamp(0.0, 1.0) } else { 0.0 };
    *disp = target.max(*disp * 0.88);

    let height = 16.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let segs = 48usize;
    let gap = 2.0;
    let w = ((rect.width() - gap * (segs as f32 - 1.0)) / segs as f32).max(1.0);
    let lit = (*disp * segs as f32).round() as usize;
    let painter = ui.painter_at(rect);
    for i in 0..segs {
        let frac = i as f32 / segs as f32;
        let x = rect.left() + i as f32 * (w + gap);
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, height));
        let color = if i < lit {
            if frac < 0.65 {
                COL_LISTEN
            } else if frac < 0.85 {
                COL_TRANS
            } else {
                COL_ERR
            }
        } else {
            COL_LINE
        };
        painter.rect_filled(r, 1.0, color);
    }
}
