//! WAV track probing, SHA-256 identity, and a streaming feeder thread (bounded memory:
//! ~2 s of audio buffered regardless of song length).

use super::core::Frame;
use crate::ring::{ring, Consumer, Producer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrackInfo {
    pub name: String,
    pub sha256: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub bytes: u64,
}

impl TrackInfo {
    pub fn duration_s(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }
}

pub fn sha256_file(path: &Path) -> Result<(String, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    let d = h.finalize();
    Ok((d.iter().map(|b| format!("{b:02x}")).collect(), total))
}

/// Validate a WAV file and compute its identity. Only PCM int 8–32 bit and float32 WAV.
pub fn probe(path: &Path) -> Result<TrackInfo, String> {
    let r = hound::WavReader::open(path).map_err(|e| format!("not a supported WAV file: {e}"))?;
    let spec = r.spec();
    if spec.channels == 0 || spec.sample_rate < 8000 || spec.sample_rate > 384_000 {
        return Err("unsupported WAV format".into());
    }
    let frames = r.duration() as u64;
    drop(r);
    let (sha256, bytes) = sha256_file(path)?;
    Ok(TrackInfo {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        sha256,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        frames,
        bytes,
    })
}

enum FeedCmd {
    Seek { gen: u32, frame: u64 },
    Idle,
}

/// Owns a decoder thread pushing tagged frames into a bounded ring.
pub struct Feeder {
    tx: Sender<FeedCmd>,
    pub info: TrackInfo,
    pub path: PathBuf,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Feeder {
    /// Returns the feeder plus the consumer side (handed to the render engine) and the
    /// end-of-file generation flag.
    pub fn open(
        path: &Path,
        info: TrackInfo,
    ) -> Result<(Self, Consumer<Frame>, Arc<AtomicU32>), String> {
        let reader = hound::WavReader::open(path).map_err(|e| format!("open WAV: {e}"))?;
        let cap = info.sample_rate as usize * 2;
        let (prod, cons) = ring::<Frame>(cap);
        let eof = Arc::new(AtomicU32::new(u32::MAX));
        let (tx, rx) = channel();
        let eof2 = eof.clone();
        let thread = std::thread::Builder::new()
            .name("duetsync-feeder".into())
            .spawn(move || feeder_thread(reader, prod, rx, eof2))
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                tx,
                info,
                path: path.to_path_buf(),
                thread: Some(thread),
            },
            cons,
            eof,
        ))
    }

    /// Start producing frames from `frame`, tagged with `gen`.
    pub fn seek(&self, gen: u32, frame: u64) {
        let _ = self.tx.send(FeedCmd::Seek { gen, frame });
    }

    pub fn idle(&self) {
        let _ = self.tx.send(FeedCmd::Idle);
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        let (tx, _) = channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn feeder_thread(
    mut reader: hound::WavReader<BufReader<File>>,
    mut prod: Producer<Frame>,
    rx: Receiver<FeedCmd>,
    eof: Arc<AtomicU32>,
) {
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let int_scale = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
    let float = spec.sample_format == hound::SampleFormat::Float;
    let mut gen: Option<u32> = None;
    let mut pending: Vec<Frame> = Vec::with_capacity(2048);
    let mut chunk: Vec<f32> = Vec::with_capacity(2048 * ch);
    loop {
        // Block when idle or when the ring is full; never busy-wait.
        let wait = if gen.is_none() {
            Duration::from_secs(3600)
        } else if !pending.is_empty() {
            Duration::from_millis(15)
        } else {
            Duration::ZERO
        };
        let cmd = if wait.is_zero() {
            rx.try_recv().map_err(|e| match e {
                std::sync::mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                std::sync::mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
            })
        } else {
            rx.recv_timeout(wait)
        };
        match cmd {
            Ok(FeedCmd::Seek { gen: g, frame }) => {
                pending.clear();
                let target = frame.min(reader.duration() as u64) as u32;
                if reader.seek(target).is_err() {
                    eof.store(g, Ordering::Release);
                    gen = None;
                    continue;
                }
                eof.store(u32::MAX, Ordering::Release);
                gen = Some(g);
            }
            Ok(FeedCmd::Idle) => {
                gen = None;
                pending.clear();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some(g) = gen else { continue };
        if pending.is_empty() {
            chunk.clear();
            let need = 2048 * ch;
            if float {
                for s in reader.samples::<f32>().take(need) {
                    chunk.push(s.unwrap_or(0.0));
                }
            } else {
                for s in reader.samples::<i32>().take(need) {
                    chunk.push(s.unwrap_or(0) as f32 * int_scale);
                }
            }
            if chunk.len() < ch {
                eof.store(g, Ordering::Release);
                gen = None;
                continue;
            }
            for f in chunk.chunks_exact(ch) {
                let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
                pending.push(Frame { l, r, gen: g });
            }
        }
        let n = prod.push_slice(&pending);
        pending.drain(..n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, frames: u32, rate: u32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            let v = (i % 1000) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(-v).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn probe_hash_and_stream() {
        let dir = std::env::temp_dir().join(format!("duetsync-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        let b = dir.join("b.wav");
        write_wav(&a, 48_000, 48_000);
        write_wav(&b, 48_001, 48_000);
        let ia = probe(&a).unwrap();
        let ib = probe(&b).unwrap();
        assert_eq!(ia.frames, 48_000);
        assert_ne!(ia.sha256, ib.sha256, "mismatched files must differ");
        assert_eq!(ia.sha256, probe(&a).unwrap().sha256);

        let (feeder, mut cons, eof) = Feeder::open(&a, ia).unwrap();
        feeder.seek(7, 47_000);
        let mut got = Vec::new();
        let t0 = std::time::Instant::now();
        while eof.load(Ordering::Acquire) != 7 || !cons.is_empty() {
            if let Some(f) = cons.pop() {
                got.push(f);
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(t0.elapsed() < Duration::from_secs(5));
        }
        assert_eq!(got.len(), 1000);
        assert!(got.iter().all(|f| f.gen == 7));
        assert!((got[0].l - (47_000 % 1000) as f32 / 32768.0).abs() < 1e-6);
        drop(feeder);
        let _ = std::fs::remove_dir_all(dir);
    }
}
