//! OBS integration: WebSocket client, compensation planning, background worker.

pub mod client;
pub mod compensation;

use client::ObsClient;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};

#[derive(Clone, Debug, PartialEq)]
pub struct InputState {
    pub name: String,
    pub kind: String,
    pub offset_ms: Option<i64>,
    pub monitor: String,
    pub muted: bool,
}

impl InputState {
    pub fn is_audio(&self) -> bool {
        self.offset_ms.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct ApplyResult {
    pub input: String,
    pub requested: i64,
    pub readback: Result<i64, String>,
}

impl ApplyResult {
    pub fn verified(&self) -> bool {
        self.readback.as_ref().is_ok_and(|v| *v == self.requested)
    }
}

pub enum ObsCmd {
    Connect {
        host: String,
        port: u16,
        password: Option<String>,
    },
    Disconnect,
    Refresh,
    /// Write offsets then read them back.
    SetOffsets {
        restore: bool,
        targets: Vec<(String, i64)>,
    },
    SetVideoDelay {
        source: String,
        ms: i64,
    },
    RemoveVideoDelay {
        source: String,
    },
}

#[derive(Debug)]
pub enum ObsEvent {
    Connected {
        obs: String,
        ws: String,
    },
    Disconnected(String),
    Error(String),
    Inputs(Vec<InputState>),
    Offsets {
        restore: bool,
        results: Vec<ApplyResult>,
    },
    VideoDelay {
        source: String,
        ms: Option<i64>,
    },
}

pub struct ObsWorker {
    tx: Sender<ObsCmd>,
    pub events: Receiver<ObsEvent>,
}

impl Default for ObsWorker {
    fn default() -> Self {
        Self::start()
    }
}

impl ObsWorker {
    pub fn start() -> Self {
        let (tx, rx) = channel::<ObsCmd>();
        let (etx, erx) = sync_channel::<ObsEvent>(64);
        let _ = std::thread::Builder::new()
            .name("duetsync-obs".into())
            .spawn(move || worker(rx, etx));
        Self { tx, events: erx }
    }
    pub fn send(&self, c: ObsCmd) {
        let _ = self.tx.send(c);
    }
}

fn is_conn_err(e: &str) -> bool {
    e.contains("connection closed") || e.contains("OBS read") || e.contains("OBS send")
}

fn worker(rx: Receiver<ObsCmd>, ev: SyncSender<ObsEvent>) {
    let mut client: Option<ObsClient> = None;
    while let Ok(cmd) = rx.recv() {
        let result: Result<(), String> = (|| {
            match cmd {
                ObsCmd::Connect {
                    host,
                    port,
                    password,
                } => {
                    client = None;
                    let c = ObsClient::connect(&host, port, password.as_deref())?;
                    let _ = ev.send(ObsEvent::Connected {
                        obs: c.obs_version.clone(),
                        ws: c.ws_version.clone(),
                    });
                    client = Some(c);
                    let inputs = refresh(client.as_mut().unwrap())?;
                    let _ = ev.send(ObsEvent::Inputs(inputs));
                }
                ObsCmd::Disconnect => {
                    client = None;
                    let _ = ev.send(ObsEvent::Disconnected("disconnected by user".into()));
                }
                ObsCmd::Refresh => {
                    let c = client.as_mut().ok_or("OBS not connected")?;
                    let inputs = refresh(c)?;
                    let _ = ev.send(ObsEvent::Inputs(inputs));
                }
                ObsCmd::SetOffsets { restore, targets } => {
                    let c = client.as_mut().ok_or("OBS not connected")?;
                    let mut results = Vec::new();
                    let mut conn_err: Option<String> = None;
                    for (input, ms) in targets {
                        // After a connection failure, report every remaining input as failed
                        // instead of aborting silently.
                        let readback = match &conn_err {
                            Some(e) => Err(e.clone()),
                            None => {
                                let r = c
                                    .set_sync_offset(&input, ms)
                                    .and_then(|_| c.sync_offset(&input));
                                if let Err(e) = &r {
                                    if is_conn_err(e) {
                                        conn_err = Some(e.clone());
                                    }
                                }
                                r
                            }
                        };
                        results.push(ApplyResult {
                            input,
                            requested: ms,
                            readback,
                        });
                    }
                    let _ = ev.send(ObsEvent::Offsets { restore, results });
                    if let Some(e) = conn_err {
                        // The outer handler sends Disconnected and drops the client.
                        return Err(e);
                    }
                    let inputs = refresh(c)?;
                    let _ = ev.send(ObsEvent::Inputs(inputs));
                }
                ObsCmd::SetVideoDelay { source, ms } => {
                    let c = client.as_mut().ok_or("OBS not connected")?;
                    c.set_video_delay(&source, ms)?;
                    let ms = c.video_delay(&source)?;
                    let _ = ev.send(ObsEvent::VideoDelay { source, ms });
                }
                ObsCmd::RemoveVideoDelay { source } => {
                    let c = client.as_mut().ok_or("OBS not connected")?;
                    c.remove_video_delay(&source)?;
                    let _ = ev.send(ObsEvent::VideoDelay { source, ms: None });
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            if is_conn_err(&e) || client.is_none() {
                client = None;
                let _ = ev.send(ObsEvent::Disconnected(e));
            } else {
                let _ = ev.send(ObsEvent::Error(e));
            }
        }
    }
}

fn refresh(c: &mut ObsClient) -> Result<Vec<InputState>, String> {
    let list = c.input_list()?;
    let mut out = Vec::with_capacity(list.len());
    for i in list {
        let offset = match c.sync_offset(&i.name) {
            Ok(v) => Some(v),
            Err(e) if is_conn_err(&e) => return Err(e),
            Err(_) => None, // not an audio input
        };
        let (monitor, muted) = if offset.is_some() {
            (
                c.monitor_type(&i.name).unwrap_or_default(),
                c.muted(&i.name).unwrap_or(false),
            )
        } else {
            (String::new(), false)
        };
        out.push(InputState {
            name: i.name,
            kind: i.kind,
            offset_ms: offset,
            monitor,
            muted,
        });
    }
    Ok(out)
}

/// Routing sanity checks: duplicates, desktop-audio leakage, monitoring loops.
pub fn routing_warnings(
    inputs: &[InputState],
    host_mic: Option<&str>,
    guest: Option<&str>,
    music: Option<&str>,
) -> Vec<String> {
    let mut w = Vec::new();
    let get = |n: Option<&str>| n.and_then(|n| inputs.iter().find(|i| i.name == n));
    let roles = [
        ("Host mic", host_mic),
        ("Guest (Discord)", guest),
        ("Instrumental", music),
    ];
    for (label, name) in roles {
        if let Some(n) = name {
            match get(Some(n)) {
                None => w.push(format!("{label}: OBS input '{n}' no longer exists")),
                Some(i) if !i.is_audio() => w.push(format!("{label}: '{n}' is not an audio input")),
                Some(i) => {
                    if i.monitor.ends_with("MONITOR_AND_OUTPUT")
                        || i.monitor.ends_with("MONITOR_ONLY")
                    {
                        w.push(format!(
                            "{label}: audio monitoring is enabled on '{n}' — this can create echo or a feedback loop if the monitoring device is captured or heard by Discord"
                        ));
                    }
                    if i.muted {
                        w.push(format!("{label}: '{n}' is muted in OBS"));
                    }
                }
            }
        }
    }
    if let (Some(a), Some(b)) = (host_mic, guest) {
        if a == b {
            w.push("Host mic and guest are the same OBS input".into());
        }
    }
    for i in inputs
        .iter()
        .filter(|i| i.kind == "wasapi_output_capture" && !i.muted)
    {
        if Some(i.name.as_str()) != guest && Some(i.name.as_str()) != music {
            w.push(format!(
                "'{}' (desktop audio) is live: it also contains Discord and the instrumental, which would be heard twice and unsynchronized. Mute it or use Application Audio Capture sources.",
                i.name
            ));
        }
    }
    if let Some(g) = get(guest) {
        if g.kind == "wasapi_input_capture" {
            w.push(format!(
                "Guest source '{}' is a microphone input; usually it should be an Application Audio Capture of Discord",
                g.name
            ));
        }
    }
    if host_mic.is_some() && music.is_none() {
        w.push("No instrumental source selected: the backing track will not be delayed with the vocals".into());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(name: &str, kind: &str, monitor: &str, muted: bool) -> InputState {
        InputState {
            name: name.into(),
            kind: kind.into(),
            offset_ms: Some(0),
            monitor: monitor.into(),
            muted,
        }
    }

    #[test]
    fn warns_about_leakage_and_monitoring() {
        let inputs = vec![
            inp(
                "Mic",
                "wasapi_input_capture",
                "OBS_MONITORING_TYPE_MONITOR_AND_OUTPUT",
                false,
            ),
            inp(
                "Discord",
                "wasapi_process_output_capture",
                "OBS_MONITORING_TYPE_NONE",
                false,
            ),
            inp(
                "Desktop Audio",
                "wasapi_output_capture",
                "OBS_MONITORING_TYPE_NONE",
                false,
            ),
            inp(
                "Music",
                "wasapi_process_output_capture",
                "OBS_MONITORING_TYPE_NONE",
                false,
            ),
        ];
        let w = routing_warnings(&inputs, Some("Mic"), Some("Discord"), Some("Music"));
        assert!(w.iter().any(|s| s.contains("monitoring")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("desktop audio")), "{w:?}");
        let w = routing_warnings(&inputs, Some("Mic"), Some("Mic"), None);
        assert!(w.iter().any(|s| s.contains("same OBS input")));
        assert!(w.iter().any(|s| s.contains("microphone input")));
    }
}
