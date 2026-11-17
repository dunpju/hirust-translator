//! 能量 VAD + 自动断句
//!
//! 帧长 30ms（480 样本 @16k）。RMS 超过阈值视为语音帧：
//! - 待机中连续语音 >= min_speech 触发"说话中"（含 pre_roll 回溯）
//! - 说话中连续静音 >= silence_ms 判定一句话结束，吐出完整语音段
//! - 单段超过 max_segment_ms 强制切分
//! - allow_input=false（回声防护）时丢弃一切输入

use crate::config::VadConfig;

const FRAME_LEN: usize = 480; // 30ms @ 16kHz
const SAMPLES_PER_MS: usize = 16; // 16000 / 1000
const TAIL_PAD_MS: usize = 200; // 断句后尾部保留

pub struct Vad {
    threshold: f32,
    silence_frames_needed: usize,
    min_speech_frames: usize,
    max_segment_samples: usize,
    pre_roll_samples: usize,
    tail_pad_samples: usize,

    /// 输入组帧缓冲（可能有半帧残留）
    buf: Vec<f32>,

    speaking: bool,
    /// 待机期的累积（pre_roll + 疑似语音）
    pending: Vec<f32>,
    /// 疑似语音起点（pending 中的样本下标）
    tentative: Option<usize>,
    over_frames: usize,

    segment: Vec<f32>,
    silence_run: usize,
    last_voice_end: usize,

    /// 校准剩余帧数（0 = 非校准中）
    cal_frames_left: usize,
    cal_acc: f64,
    cal_n: u64,
    cal_result: Option<f32>,

    level: f32,
}

impl Vad {
    pub fn new(cfg: &VadConfig) -> Self {
        Self {
            threshold: cfg.energy_threshold,
            silence_frames_needed: (cfg.silence_ms as usize / 30).max(1),
            min_speech_frames: (cfg.min_speech_ms as usize / 30).max(1),
            max_segment_samples: cfg.max_segment_ms as usize * SAMPLES_PER_MS,
            pre_roll_samples: cfg.pre_roll_ms as usize * SAMPLES_PER_MS,
            tail_pad_samples: TAIL_PAD_MS * SAMPLES_PER_MS,

            buf: Vec::new(),
            speaking: false,
            pending: Vec::new(),
            tentative: None,
            over_frames: 0,
            segment: Vec::new(),
            silence_run: 0,
            last_voice_end: 0,
            cal_frames_left: 0,
            cal_acc: 0.0,
            cal_n: 0,
            cal_result: None,
            level: 0.0,
        }
    }

    /// 启动环境噪声校准（静默 duration_ms 后阈值 = max(噪声×3, 0.004)）
    pub fn begin_calibration(&mut self, duration_ms: u64) {
        self.cal_frames_left = (duration_ms as usize / 30).max(1);
        self.cal_acc = 0.0;
        self.cal_n = 0;
    }

    /// 取出校准结果（一次性）
    pub fn take_calibration_result(&mut self) -> Option<f32> {
        self.cal_result.take()
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// 送入 16k 单声道 PCM；完整语音段追加到 out
    pub fn process(&mut self, pcm: &[f32], allow_input: bool, out: &mut Vec<Vec<f32>>) {
        self.buf.extend_from_slice(pcm);
        let mut off = 0usize;
        let mut frame = [0f32; FRAME_LEN];
        while self.buf.len() - off >= FRAME_LEN {
            frame.copy_from_slice(&self.buf[off..off + FRAME_LEN]);
            off += FRAME_LEN;
            let rms = Self::rms(&frame);
            self.level = rms;
            self.handle_frame(&frame, rms, allow_input, out);
        }
        if off > 0 {
            self.buf.drain(..off);
        }
    }

    fn rms(frame: &[f32]) -> f32 {
        let sum: f32 = frame.iter().map(|s| s * s).sum();
        (sum / frame.len() as f32).sqrt()
    }

    fn handle_frame(&mut self, frame: &[f32], rms: f32, allow: bool, out: &mut Vec<Vec<f32>>) {
        // 校准期：只累计噪声，不做检测
        if self.cal_frames_left > 0 {
            self.cal_frames_left -= 1;
            if allow {
                self.cal_acc += rms as f64;
                self.cal_n += 1;
            }
            if self.cal_frames_left == 0 {
                let noise = (self.cal_acc / self.cal_n.max(1) as f64) as f32;
                self.threshold = (noise * 3.0).clamp(0.004, 0.05);
                self.cal_result = Some(self.threshold);
            }
            return;
        }

        if !allow {
            // 回声防护：丢弃一切（包括半截语音，避免拼接垃圾）
            if self.speaking {
                self.segment.clear();
                self.speaking = false;
                self.silence_run = 0;
            }
            self.tentative = None;
            self.over_frames = 0;
            self.pending.clear();
            return;
        }

        let voice = rms >= self.threshold;

        if !self.speaking {
            self.pending.extend_from_slice(frame);
            if voice {
                let start = self.pending.len() - frame.len();
                *self.tentative.get_or_insert(start) = self.tentative.unwrap_or(start);
                self.over_frames += 1;
                if self.over_frames >= self.min_speech_frames {
                    let seg_start = self.tentative.unwrap().saturating_sub(self.pre_roll_samples);
                    self.segment = self.pending.split_off(seg_start);
                    self.pending.clear();
                    self.speaking = true;
                    self.silence_run = 0;
                    self.last_voice_end = self.segment.len();
                    self.over_frames = 0;
                    self.tentative = None;
                }
            } else {
                self.over_frames = 0;
                if let Some(t) = self.tentative {
                    // 疑似语音未达最短时长，静音足够久后放弃
                    let since = self.pending.len() - t;
                    let cancel_after = self.silence_frames_needed * FRAME_LEN + self.pre_roll_samples;
                    if since > cancel_after {
                        self.tentative = None;
                    }
                }
                if self.tentative.is_none() && self.pending.len() > self.pre_roll_samples {
                    let cut = self.pending.len() - self.pre_roll_samples;
                    self.pending.drain(..cut);
                }
            }
        } else {
            self.segment.extend_from_slice(frame);
            if voice {
                self.silence_run = 0;
                self.last_voice_end = self.segment.len();
            } else {
                self.silence_run += 1;
                if self.silence_run >= self.silence_frames_needed {
                    self.finalize(out);
                    return;
                }
            }
            if self.segment.len() >= self.max_segment_samples {
                self.finalize(out);
            }
        }
    }

    fn finalize(&mut self, out: &mut Vec<Vec<f32>>) {
        let end = (self.last_voice_end + self.tail_pad_samples).min(self.segment.len());
        let head: Vec<f32> = self.segment.drain(..end).collect();
        let tail = std::mem::take(&mut self.segment);

        // 断句残留尾部作为下一句的 pre-roll 起点
        self.pending.clear();
        if tail.len() > self.pre_roll_samples {
            self.pending.extend_from_slice(&tail[tail.len() - self.pre_roll_samples..]);
        } else {
            self.pending.extend_from_slice(&tail);
        }

        let long_enough = head.len() >= self.min_speech_frames * FRAME_LEN;
        if long_enough {
            out.push(head);
        }
        self.speaking = false;
        self.silence_run = 0;
        self.tentative = None;
        self.over_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VadConfig {
        VadConfig::default()
    }

    #[test]
    fn splits_on_silence() {
        let mut vad = Vad::new(&cfg());
        // 1 秒"语音"（正弦幅值0.3）+ 1 秒静音
        let speech: Vec<f32> = (0..16_000).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        let silence = vec![0.0f32; 16_000];
        let mut out = Vec::new();
        vad.process(&speech, true, &mut out);
        assert!(out.is_empty());
        vad.process(&silence, true, &mut out);
        assert_eq!(out.len(), 1);
        let seg = &out[0];
        // 语音 1s：33 个纯语音帧 + 1 个混合帧（34×480=16320）+ 尾垫 200ms（3200）= 19520
        assert!((19_000..=19_600).contains(&seg.len()), "seg len = {}", seg.len());
    }

    #[test]
    fn ignores_pure_noise() {
        let mut vad = Vad::new(&cfg());
        let noise = vec![0.001f32; 32_000]; // 远低于阈值
        let mut out = Vec::new();
        vad.process(&noise, true, &mut out);
        assert!(out.is_empty());
    }
}
