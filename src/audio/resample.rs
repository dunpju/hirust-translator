//! 线性插值重采样（默认 48k/44.1k → 16k），跨块保持相位连续

pub struct Resampler {
    /// 每产出一个输出样本需要消耗的输入样本数
    step: f64,
    carry: Vec<f32>,
    /// 在 carry 中的浮点读位置（输入样本单位）
    pos: f64,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            step: input_rate as f64 / output_rate as f64,
            carry: Vec::with_capacity(8192),
            pos: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.carry.extend_from_slice(input);
        let len = self.carry.len() as f64;
        while self.pos + 1.0 < len {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let s = self.carry[i] * (1.0 - frac) + self.carry[i + 1] * frac;
            out.push(s);
            self.pos += self.step;
        }
        let consumed = self.pos.floor() as usize;
        if consumed > 0 {
            self.carry.drain(0..consumed);
            self.pos -= consumed as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_48k_to_16k_length() {
        let mut r = Resampler::new(48_000, 16_000);
        // 1 秒 48k 正弦，分块送入
        let mut total = 0usize;
        let chunk: Vec<f32> = (0..4800)
            .map(|i| ((i as f32) * 0.01).sin())
            .collect();
        for _ in 0..10 {
            let mut out = Vec::new();
            r.process(&chunk, &mut out);
            total += out.len();
        }
        // 应输出约 16000 个样本（±1 块的尾差）
        assert!((15_900..=16_100).contains(&total), "total = {total}");
    }

    #[test]
    fn passthrough_same_rate() {
        let mut r = Resampler::new(16_000, 16_000);
        let mut out = Vec::new();
        r.process(&[0.5, 0.5, 0.5], &mut out);
        assert_eq!(out.len(), 2); // 最后一个样本等待下一个块
        r.process(&[0.5], &mut out);
        assert_eq!(out.len(), 3);
    }
}
