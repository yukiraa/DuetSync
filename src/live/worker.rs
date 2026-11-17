//! Non-real-time capture-to-envelope worker.

use super::EnvelopeBuilder;
use crate::audio::{wasapi::CaptureHandle, Anchor, CaptureSource};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug)]
pub struct LiveBins {
    pub start_ms: i64,
    pub values: Vec<f32>,
}

pub struct LiveCapture {
    rx: Receiver<LiveBins>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    pub dropped: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl LiveCapture {
    pub fn start(source: CaptureSource) -> Result<Self, String> {
        let (tx, rx) = sync_channel(64);
        let (ready_tx, ready_rx) = sync_channel(1);
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_error = error.clone();
        let worker_stop = stop.clone();
        let worker_dropped = dropped.clone();
        let thread = std::thread::Builder::new()
            .name("duetsync-live-capture".into())
            .spawn(move || {
                let mut capture = match CaptureHandle::start(source) {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.clone()));
                        if let Ok(mut slot) = worker_error.lock() {
                            *slot = Some(e);
                        }
                        return;
                    }
                };
                capture.set_streaming(true);
                let mut builder = EnvelopeBuilder::new();
                let mut samples = Vec::with_capacity(16_384);
                let mut anchors: Vec<Anchor> = Vec::with_capacity(256);
                while !worker_stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(100));
                    if let Err(e) = capture.drain(&mut samples, &mut anchors) {
                        if let Ok(mut slot) = worker_error.lock() {
                            *slot = Some(e);
                        }
                        break;
                    }
                    if let Some((start_ms, values)) = builder.push(&samples, &anchors) {
                        match tx.try_send(LiveBins { start_ms, values }) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                worker_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(TrySendError::Disconnected(_)) => break,
                        }
                    }
                }
                capture.set_streaming(false);
            })
            .map_err(|e| format!("spawn live capture: {e}"))?;
        ready_rx
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| "live capture start timed out".to_string())??;
        Ok(Self {
            rx,
            error,
            stop,
            dropped,
            thread: Some(thread),
        })
    }

    pub fn try_recv(&self) -> Option<LiveBins> {
        self.rx.try_recv().ok()
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|slot| slot.clone())
    }
}

impl Drop for LiveCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
