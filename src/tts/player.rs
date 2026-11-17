//! 播放：rodio 解码 mp3 输出到默认设备
//!
//! rodio 的 OutputStream 非 Send（内含平台原始指针），
//! 因此全部播放操作收敛在专用线程内，外部仅持有通道句柄。

use anyhow::{Context, Result};

pub struct Player {
    tx: std::sync::mpsc::Sender<(Vec<u8>, tokio::sync::oneshot::Sender<()>)>,
}

impl Player {
    /// 启动专用播放线程（串行播放，保证译文顺序）
    pub fn spawn() -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<(Vec<u8>, tokio::sync::oneshot::Sender<()>)>();
        std::thread::Builder::new()
            .name("audio-player".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
                    eprintln!("[player] 音频输出设备不可用");
                    return;
                };
                while let Ok((mp3, done)) = rx.recv() {
                    let played = (|| -> Result<()> {
                        let src = rodio::Decoder::new(std::io::Cursor::new(mp3))
                            .context("解码 mp3 失败")?;
                        let sink = rodio::Sink::try_new(&handle).context("创建播放器失败")?;
                        sink.append(src);
                        sink.sleep_until_end();
                        Ok(())
                    })();
                    if let Err(e) = played {
                        eprintln!("[player] {e:#}");
                    }
                    let _ = done.send(());
                }
            })?;
        Ok(Self { tx })
    }

    /// 入队播放，返回播放完成信号
    pub fn play(&self, mp3: Vec<u8>) -> tokio::sync::oneshot::Receiver<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        // 播放线程已退出（无输出设备）时 send 失败，done_rx 立即就绪
        let _ = self.tx.send((mp3, done_tx));
        done_rx
    }
}
