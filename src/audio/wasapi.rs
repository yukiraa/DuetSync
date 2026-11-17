//! WASAPI shared-mode capture (device, output loopback, process loopback) and render.
//! All streams are event driven, float32 stereo at the requested rate with OS conversion
//! (`AUTOCONVERTPCM`). Audio threads never allocate or take blocking locks.

use super::{fit_anchors, regrid, Anchor, CaptureSource, DeviceInfo};
use crate::calibration::Recording;
use crate::clock::{now_us, qpc100ns_to_us};
use crate::playback::core::{Command, Frame, FrameSource, PlayerCore, State};
use crate::ring::{ring, Consumer, Producer};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{implement, Interface, HSTRING};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};

type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(ctx: &str) -> impl FnOnce(E) -> String + '_ {
    move |err| format!("{ctx}: {err}")
}

struct Com(bool);
impl Com {
    fn init() -> Self {
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

struct Mmcss(HANDLE);
impl Mmcss {
    fn pro_audio() -> Option<Self> {
        let mut idx = 0u32;
        unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut idx) }
            .ok()
            .map(Mmcss)
    }
}
impl Drop for Mmcss {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

fn float_format(rate: u32, ch: u16) -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: ch,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * ch as u32 * 4,
        nBlockAlign: ch * 4,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

fn enumerator() -> Res<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(e("device enumerator"))
}

unsafe fn device_info(d: &IMMDevice) -> Res<DeviceInfo> {
    let idp = d.GetId().map_err(e("GetId"))?;
    let id = idp.to_string().unwrap_or_default();
    CoTaskMemFree(Some(idp.0 as *const _));
    let name = d
        .OpenPropertyStore(STGM_READ)
        .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
        .map(|v| v.to_string())
        .unwrap_or_else(|_| id.clone());
    Ok(DeviceInfo { id, name })
}

/// List active endpoints. Runs on a helper thread so COM apartment state of the caller
/// (the UI thread is STA under winit) is irrelevant.
pub fn list_devices(capture: bool) -> Res<Vec<DeviceInfo>> {
    std::thread::spawn(move || -> Res<Vec<DeviceInfo>> {
        let _com = Com::init();
        unsafe {
            let en = enumerator()?;
            let flow = if capture { eCapture } else { eRender };
            let col = en
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
                .map_err(e("EnumAudioEndpoints"))?;
            let n = col.GetCount().map_err(e("GetCount"))?;
            let mut v = Vec::new();
            for i in 0..n {
                if let Ok(d) = col.Item(i) {
                    v.push(device_info(&d)?);
                }
            }
            Ok(v)
        }
    })
    .join()
    .map_err(|_| "device enumeration thread panicked".to_string())?
}

unsafe fn open_device(id: &Option<String>, capture: bool) -> Res<IMMDevice> {
    let en = enumerator()?;
    match id {
        Some(id) => en
            .GetDevice(&HSTRING::from(id.as_str()))
            .map_err(e("device not found (unplugged?)")),
        None => en
            .GetDefaultAudioEndpoint(if capture { eCapture } else { eRender }, eConsole)
            .map_err(e("no default audio device")),
    }
}

// ---------- process loopback activation ----------

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivateHandler(Arc<(Mutex<bool>, Condvar)>);

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateHandler_Impl {
    fn ActivateCompleted(
        &self,
        _op: windows_core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows_core::Result<()> {
        let (m, c) = &*self.0;
        *m.lock().unwrap() = true;
        c.notify_all();
        Ok(())
    }
}

/// PROPVARIANT holding a VT_BLOB (layout-compatible with the Win32 struct on x86/x64).
#[repr(C)]
struct BlobPropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    cb: u32,
    ptr: *const u8,
}

unsafe fn activate_process_loopback(pid: u32) -> Res<IAudioClient> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let pv = BlobPropVariant {
        vt: 65, // VT_BLOB
        r1: 0,
        r2: 0,
        r3: 0,
        cb: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
        ptr: &params as *const _ as *const u8,
    };
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let handler: IActivateAudioInterfaceCompletionHandler = ActivateHandler(done.clone()).into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&pv as *const _ as *const PROPVARIANT),
        &handler,
    )
    .map_err(e("process loopback activation (needs Windows 10 2004+)"))?;
    {
        let (m, c) = &*done;
        let g = m.lock().unwrap();
        let (g, to) = c
            .wait_timeout_while(g, Duration::from_secs(5), |d| !*d)
            .unwrap();
        if to.timed_out() && !*g {
            return Err("process loopback activation timed out".into());
        }
    }
    let mut hr = windows_core::HRESULT(0);
    let mut unk: Option<windows_core::IUnknown> = None;
    op.GetActivateResult(&mut hr, &mut unk)
        .map_err(e("GetActivateResult"))?;
    hr.ok().map_err(e("process loopback activation failed"))?;
    unk.ok_or("no interface returned")?
        .cast::<IAudioClient>()
        .map_err(e("IAudioClient cast"))
}

// ---------- capture ----------

pub struct CaptureShared {
    peak_bits: AtomicU32,
    stop: AtomicBool,
    recording: AtomicBool,
    pub alive: AtomicBool,
    pub frames: AtomicU64,
    pub error: Mutex<Option<String>>,
}

pub struct CaptureHandle {
    pub shared: Arc<CaptureShared>,
    pub source: CaptureSource,
    pub description: String,
    samples: Consumer<f32>,
    anchors: Consumer<Anchor>,
    thread: Option<JoinHandle<()>>,
}

const CAPTURE_RING_SECONDS: usize = 4;

impl CaptureHandle {
    pub fn start(source: CaptureSource) -> Res<Self> {
        let rate = crate::SAMPLE_RATE;
        let (sp, sc) = ring::<f32>(rate as usize * CAPTURE_RING_SECONDS);
        let (ap, ac) = ring::<Anchor>(4096);
        let shared = Arc::new(CaptureShared {
            peak_bits: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            error: Mutex::new(None),
        });
        let (tx, rx) = sync_channel::<Res<String>>(1);
        let sh = shared.clone();
        let src = source.clone();
        let thread = std::thread::Builder::new()
            .name("duetsync-capture".into())
            .spawn(move || capture_thread(src, sh, sp, ap, tx))
            .map_err(e("spawn"))?;
        let description = rx
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| "capture start timed out".to_string())??;
        Ok(Self {
            shared,
            source,
            description,
            samples: sc,
            anchors: ac,
            thread: Some(thread),
        })
    }

    /// Peak level since the last call (linear 0..1+).
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.shared.peak_bits.swap(0, Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|g| g.clone())
    }

    /// Record from now until local time `end_us`. Blocking; call from a worker thread.
    pub fn record_until(&mut self, end_us: i64) -> Res<Recording> {
        self.samples.clear();
        self.anchors.clear();
        let dropped0 = self.samples.dropped() + self.anchors.dropped();
        self.shared.recording.store(true, Ordering::Release);
        let mut samples: Vec<f32> = Vec::with_capacity(
            ((end_us - now_us()).max(0) as usize / 1000 + 500) * crate::SAMPLE_RATE as usize / 1000,
        );
        let mut anchors: Vec<Anchor> = Vec::with_capacity(4096);
        let mut buf = vec![0f32; 8192];
        let mut abuf = [Anchor::default(); 64];
        let deadline = end_us + 3_000_000;
        let result = loop {
            std::thread::sleep(Duration::from_millis(20));
            loop {
                let n = self.samples.pop_slice(&mut buf);
                samples.extend_from_slice(&buf[..n]);
                if n < buf.len() {
                    break;
                }
            }
            loop {
                let n = self.anchors.pop_slice(&mut abuf);
                anchors.extend_from_slice(&abuf[..n]);
                if n < abuf.len() {
                    break;
                }
            }
            if let Some(err) = self.error() {
                break Err(err);
            }
            if anchors.last().is_some_and(|a| a.t_us >= end_us) {
                break Ok(());
            }
            if now_us() > deadline {
                break Err("capture produced no data (device idle or silent stream)".into());
            }
        };
        self.shared.recording.store(false, Ordering::Release);
        result?;
        if self.samples.dropped() + self.anchors.dropped() != dropped0 {
            return Err("capture overflow — recording discarded".into());
        }
        let rate = crate::SAMPLE_RATE as f64;
        let (samples, start_us) = regrid(&samples, &anchors, rate).ok_or("no audio received")?;
        // Residual clock-rate estimate over the continuous stretches (diagnostic only).
        let rate_hz = fit_anchors(&anchors, rate)
            .map(|(_, b)| 1e6 / b)
            .unwrap_or(rate);
        Ok(Recording {
            samples,
            start_us,
            sample_rate: crate::SAMPLE_RATE,
            rate_hz: if (rate_hz / rate - 1.0).abs() < 1e-3 {
                rate_hz
            } else {
                rate
            },
        })
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn capture_thread(
    src: CaptureSource,
    sh: Arc<CaptureShared>,
    mut sp: Producer<f32>,
    mut ap: Producer<Anchor>,
    ready: SyncSender<Res<String>>,
) {
    let _com = Com::init();
    let _mm = Mmcss::pro_audio();
    let setup = unsafe { setup_capture(&src) };
    let (client, cap, event, desc) = match setup {
        Ok(v) => v,
        Err(err) => {
            sh.alive.store(false, Ordering::Release);
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(desc));
    let rate = crate::SAMPLE_RATE as f64;
    let mut scratch: Vec<f32> = vec![0.0; crate::SAMPLE_RATE as usize];
    let mut counter: u64 = 0;
    let fail = |msg: String| {
        if let Ok(mut g) = sh.error.lock() {
            *g = Some(msg);
        }
    };
    'outer: while !sh.stop.load(Ordering::Acquire) {
        let w = unsafe { WaitForSingleObject(event, 200) };
        let _ = w == WAIT_OBJECT_0;
        loop {
            let n = match unsafe { cap.GetNextPacketSize() } {
                Ok(n) => n,
                Err(err) => {
                    fail(disconnect_msg(&err));
                    break 'outer;
                }
            };
            if n == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let (mut frames, mut flags, mut devpos, mut qpc) = (0u32, 0u32, 0u64, 0u64);
            if let Err(err) = unsafe {
                cap.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut devpos),
                    Some(&mut qpc),
                )
            } {
                fail(disconnect_msg(&err));
                break 'outer;
            }
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            let ts_err = flags & (AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32) != 0;
            let f = (frames as usize).min(scratch.len());
            let mut peak = 0f32;
            if silent || data.is_null() {
                scratch[..f].fill(0.0);
            } else {
                let s = unsafe { std::slice::from_raw_parts(data as *const f32, f * 2) };
                for i in 0..f {
                    let v = 0.5 * (s[2 * i] + s[2 * i + 1]);
                    peak = peak.max(v.abs());
                    scratch[i] = v;
                }
            }
            let t_us = if qpc != 0 && !ts_err {
                qpc100ns_to_us(qpc)
            } else {
                // Fallback: packet's first frame was captured at least `frames` ago.
                now_us() - (frames as f64 * 1e6 / rate) as i64
            };
            if sh.recording.load(Ordering::Acquire) {
                ap.push(Anchor {
                    frame: counter,
                    t_us,
                    frames,
                });
                sp.push_slice(&scratch[..f]);
            }
            counter += frames as u64;
            sh.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
            if unsafe { cap.ReleaseBuffer(frames) }.is_err() {
                fail("capture ReleaseBuffer failed".into());
                break 'outer;
            }
        }
        sh.frames.store(counter, Ordering::Relaxed);
    }
    unsafe {
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    sh.alive.store(false, Ordering::Release);
}

fn disconnect_msg(err: &windows_core::Error) -> String {
    if err.code() == AUDCLNT_E_DEVICE_INVALIDATED {
        "audio device disconnected".into()
    } else {
        format!("capture error: {err}")
    }
}

unsafe fn setup_capture(
    src: &CaptureSource,
) -> Res<(IAudioClient, IAudioCaptureClient, HANDLE, String)> {
    let base = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    let (client, flags, desc) = match src {
        CaptureSource::Input(id) => {
            let d = open_device(id, true)?;
            let name = device_info(&d).map(|i| i.name).unwrap_or_default();
            let c: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(e("Activate"))?;
            (c, base, format!("input: {name}"))
        }
        CaptureSource::OutputLoopback(id) => {
            let d = open_device(id, false)?;
            let name = device_info(&d).map(|i| i.name).unwrap_or_default();
            let c: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(e("Activate"))?;
            (
                c,
                base | AUDCLNT_STREAMFLAGS_LOOPBACK,
                format!("loopback: {name}"),
            )
        }
        CaptureSource::Process { pid, name } => (
            activate_process_loopback(*pid)?,
            base | AUDCLNT_STREAMFLAGS_LOOPBACK,
            format!("process loopback: {name} ({pid})"),
        ),
    };
    let fmt = float_format(crate::SAMPLE_RATE, 2);
    client
        .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 200_000, 0, &fmt, None)
        .map_err(e("capture Initialize"))?;
    let event = CreateEventW(None, false, false, None).map_err(e("CreateEvent"))?;
    client.SetEventHandle(event).map_err(e("SetEventHandle"))?;
    let cap: IAudioCaptureClient = client.GetService().map_err(e("GetService"))?;
    client.Start().map_err(e("capture Start"))?;
    Ok((client, cap, event, desc))
}

// ---------- render ----------

/// Track frames produced by a feeder thread (see `playback::track`).
pub struct RingSource {
    pub cons: Consumer<Frame>,
    pub eof_gen: Arc<AtomicU32>,
}

impl FrameSource for RingSource {
    fn pop(&mut self) -> Option<Frame> {
        self.cons.pop()
    }
    fn ended(&self, gen: u32) -> bool {
        self.eof_gen.load(Ordering::Acquire) == gen
    }
    fn peek_gen(&self) -> Option<u32> {
        self.cons.peek().map(|f| f.gen)
    }
}

struct NoSource;
impl FrameSource for NoSource {
    fn pop(&mut self) -> Option<Frame> {
        None
    }
    fn peek_gen(&self) -> Option<u32> {
        None
    }
    fn ended(&self, _: u32) -> bool {
        true
    }
}

pub enum RenderMsg {
    Core(Command),
    Source(Option<RingSource>),
}

pub enum Trash {
    Clip(Arc<[f32]>),
    Source(RingSource),
}

#[derive(Default)]
pub struct RenderShared {
    pub state: AtomicU8,
    pos_bits: AtomicU64,
    pub pos_at_us: AtomicI64,
    pub drift_us: AtomicI64,
    pub underruns: AtomicU64,
    pub resyncs: AtomicU64,
    peak_bits: AtomicU32,
    pub stream_latency_us: AtomicI64,
    /// Track frame the player needs re-seeking to (u64::MAX = none).
    pub reseek: AtomicU64,
    stop: AtomicBool,
    pub alive: AtomicBool,
    pub error: Mutex<Option<String>>,
}

pub fn state_from_u8(v: u8) -> State {
    match v {
        1 => State::Scheduled,
        2 => State::Playing,
        3 => State::Paused,
        4 => State::Ended,
        _ => State::Stopped,
    }
}

fn state_to_u8(s: State) -> u8 {
    match s {
        State::Stopped => 0,
        State::Scheduled => 1,
        State::Playing => 2,
        State::Paused => 3,
        State::Ended => 4,
    }
}

impl RenderShared {
    pub fn state(&self) -> State {
        state_from_u8(self.state.load(Ordering::Relaxed))
    }
    /// Track position (frames) at local time `t_us` (extrapolated while playing).
    pub fn position_at(&self, t_us: i64, rate: f64) -> f64 {
        let p = f64::from_bits(self.pos_bits.load(Ordering::Relaxed));
        if self.state() == State::Playing {
            let at = self.pos_at_us.load(Ordering::Relaxed);
            (p + (t_us - at) as f64 * rate / 1e6).max(0.0)
        } else {
            p
        }
    }
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.swap(0, Ordering::Relaxed))
    }
}

pub struct RenderHandle {
    pub shared: Arc<RenderShared>,
    pub rate: u32,
    pub description: String,
    tx: SyncSender<RenderMsg>,
    trash: Receiver<Trash>,
    thread: Option<JoinHandle<()>>,
}

impl RenderHandle {
    pub fn start(device: Option<String>, rate: u32) -> Res<Self> {
        let shared = Arc::new(RenderShared::default());
        shared.alive.store(true, Ordering::Release);
        shared.reseek.store(u64::MAX, Ordering::Release);
        let (tx, rx) = sync_channel::<RenderMsg>(64);
        let (ttx, trx) = sync_channel::<Trash>(64);
        let (rtx, rrx) = sync_channel::<Res<String>>(1);
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("duetsync-render".into())
            .spawn(move || render_thread(device, rate, sh, rx, ttx, rtx))
            .map_err(e("spawn"))?;
        let description = rrx
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| "render start timed out".to_string())??;
        Ok(Self {
            shared,
            rate,
            description,
            tx,
            trash: trx,
            thread: Some(thread),
        })
    }

    pub fn send(&self, m: RenderMsg) -> Res<()> {
        // Drop items the audio thread handed back, outside the audio thread.
        while self.trash.try_recv().is_ok() {}
        match self.tx.try_send(m) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("render command queue full".into()),
            Err(TrySendError::Disconnected(_)) => Err(self
                .error()
                .unwrap_or_else(|| "render stream stopped".into())),
        }
    }

    pub fn cmd(&self, c: Command) -> Res<()> {
        self.send(RenderMsg::Core(c))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for RenderHandle {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn render_thread(
    device: Option<String>,
    rate: u32,
    sh: Arc<RenderShared>,
    rx: Receiver<RenderMsg>,
    trash: SyncSender<Trash>,
    ready: SyncSender<Res<String>>,
) {
    let _com = Com::init();
    let _mm = Mmcss::pro_audio();
    let setup = unsafe { setup_render(&device, rate) };
    let (client, render, clock, event, bufsize, desc) = match setup {
        Ok(v) => v,
        Err(err) => {
            sh.alive.store(false, Ordering::Release);
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(desc));
    let freq = unsafe { clock.GetFrequency() }.unwrap_or(1).max(1) as f64;
    let r = rate as f64;
    let mut core = PlayerCore::new(rate);
    let mut source: Option<RingSource> = None;
    let mut scratch = vec![0f32; bufsize as usize * 2];
    let mut written: u64 = bufsize as u64; // prefilled silence
    let fail = |msg: String| {
        if let Ok(mut g) = sh.error.lock() {
            *g = Some(msg);
        }
    };
    if let Ok(lat) = unsafe { client.GetStreamLatency() } {
        sh.stream_latency_us.store(lat / 10, Ordering::Relaxed);
    }
    while !sh.stop.load(Ordering::Acquire) {
        unsafe { WaitForSingleObject(event, 200) };
        while let Ok(m) = rx.try_recv() {
            match m {
                RenderMsg::Core(c) => {
                    if let Some(clip) = core.command(c) {
                        let _ = trash.try_send(Trash::Clip(clip));
                    }
                }
                RenderMsg::Source(s) => {
                    if let Some(old) = std::mem::replace(&mut source, s) {
                        let _ = trash.try_send(Trash::Source(old));
                    }
                }
            }
        }
        let padding = match unsafe { client.GetCurrentPadding() } {
            Ok(p) => p,
            Err(err) => {
                fail(if err.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                    "output device disconnected".into()
                } else {
                    format!("render error: {err}")
                });
                break;
            }
        };
        let avail = bufsize.saturating_sub(padding);
        if avail == 0 {
            continue;
        }
        let (mut pos, mut qpc) = (0u64, 0u64);
        let t0 = if unsafe { clock.GetPosition(&mut pos, Some(&mut qpc)) }.is_ok() && qpc != 0 {
            let presented = pos as f64 / freq * r;
            qpc100ns_to_us(qpc) as f64 + (written as f64 - presented) * 1e6 / r
        } else {
            now_us() as f64 + padding as f64 * 1e6 / r
        };
        let out = &mut scratch[..avail as usize * 2];
        let mut fin = |c: Arc<[f32]>| {
            let _ = trash.try_send(Trash::Clip(c));
        };
        match source.as_mut() {
            Some(s) => core.fill(out, written, t0, s, &mut fin),
            None => core.fill(out, written, t0, &mut NoSource, &mut fin),
        }
        match unsafe { render.GetBuffer(avail) } {
            Ok(p) => unsafe {
                std::ptr::copy_nonoverlapping(out.as_ptr(), p as *mut f32, out.len());
                let _ = render.ReleaseBuffer(avail, 0);
            },
            Err(err) => {
                fail(format!("render GetBuffer: {err}"));
                break;
            }
        }
        written += avail as u64;
        sh.state.store(state_to_u8(core.state), Ordering::Relaxed);
        sh.pos_bits
            .store(core.position_frames().to_bits(), Ordering::Relaxed);
        // Position is now at the END of the written block: timestamp that frame.
        sh.pos_at_us.store(
            (core.audible_now(written - avail as u64, t0) + avail as f64 * 1e6 / r) as i64,
            Ordering::Relaxed,
        );
        sh.drift_us
            .store((core.drift_ms() * 1000.0) as i64, Ordering::Relaxed);
        sh.underruns.store(core.underruns, Ordering::Relaxed);
        sh.resyncs.store(core.hard_resyncs, Ordering::Relaxed);
        if let Some(f) = core.needs_reseek {
            sh.reseek.store(f, Ordering::Release);
        }
        sh.peak_bits
            .fetch_max(core.peak.to_bits(), Ordering::Relaxed);
    }
    unsafe {
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    sh.alive.store(false, Ordering::Release);
}

type RenderSetup = (
    IAudioClient,
    IAudioRenderClient,
    IAudioClock,
    HANDLE,
    u32,
    String,
);

unsafe fn setup_render(device: &Option<String>, rate: u32) -> Res<RenderSetup> {
    let d = open_device(device, false)?;
    let name = device_info(&d).map(|i| i.name).unwrap_or_default();
    let client: IAudioClient = d.Activate(CLSCTX_ALL, None).map_err(e("Activate"))?;
    let fmt = float_format(rate, 2);
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    client
        .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &fmt, None)
        .map_err(e("render Initialize"))?;
    let event = CreateEventW(None, false, false, None).map_err(e("CreateEvent"))?;
    client.SetEventHandle(event).map_err(e("SetEventHandle"))?;
    let bufsize = client.GetBufferSize().map_err(e("GetBufferSize"))?;
    let render: IAudioRenderClient = client.GetService().map_err(e("GetService"))?;
    let clock: IAudioClock = client.GetService().map_err(e("GetService clock"))?;
    let p = render.GetBuffer(bufsize).map_err(e("prefill"))?;
    std::ptr::write_bytes(p, 0, bufsize as usize * 8);
    render
        .ReleaseBuffer(bufsize, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)
        .map_err(e("prefill release"))?;
    client.Start().map_err(e("render Start"))?;
    Ok((
        client,
        render,
        clock,
        event,
        bufsize,
        format!("{name} @ {rate} Hz"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_devices_fail_cleanly() {
        let e = CaptureHandle::start(CaptureSource::Input(Some("{bogus-device-id}".into())));
        assert!(e.is_err());
        let r = RenderHandle::start(Some("{bogus-device-id}".into()), 48_000);
        assert!(r.err().unwrap().contains("not found"));
        let p = CaptureHandle::start(CaptureSource::Process {
            pid: u32::MAX - 7,
            name: "nonexistent".into(),
        });
        // Process loopback of a missing PID either fails or yields a silent stream;
        // it must never panic.
        drop(p);
    }
}
