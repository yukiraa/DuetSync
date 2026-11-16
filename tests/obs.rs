//! OBS integration against an in-process mock OBS WebSocket 5.x server.

use duetsync::obs::client::{auth_string, ObsClient};
use duetsync::obs::compensation::{plan, OffsetLedger, SourceLatency};
use duetsync::obs::{ObsCmd, ObsEvent, ObsWorker};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tungstenite::Message;

#[derive(Default)]
struct MockState {
    offsets: BTreeMap<String, i64>,
    kinds: BTreeMap<String, String>,
    filters: BTreeMap<(String, String), Value>,
    log: Vec<(String, Value)>,
}

const SALT: &str = "c2FsdA==";
const CHALLENGE: &str = "Y2hhbGxlbmdl";

fn mock_obs(password: &'static str) -> (u16, Arc<Mutex<MockState>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(MockState::default()));
    {
        let mut s = state.lock().unwrap();
        for (n, k, o) in [
            ("Mic/Aux", "wasapi_input_capture", 0),
            ("Discord", "wasapi_process_output_capture", 0),
            ("Instrumental", "wasapi_process_output_capture", 25),
        ] {
            s.kinds.insert(n.into(), k.into());
            s.offsets.insert(n.into(), o);
        }
        s.kinds.insert("Avatar".into(), "window_capture".into());
    }
    let st = state.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let st = st.clone();
            std::thread::spawn(move || {
                let mut ws = tungstenite::accept(stream).unwrap();
                let send = |ws: &mut tungstenite::WebSocket<_>, v: Value| {
                    ws.send(Message::Text(v.to_string().into())).unwrap()
                };
                send(
                    &mut ws,
                    json!({"op":0,"d":{"obsWebSocketVersion":"5.5.0","rpcVersion":1,
                        "authentication":{"challenge":CHALLENGE,"salt":SALT}}}),
                );
                let Ok(Message::Text(t)) = ws.read() else {
                    return;
                };
                let id: Value = serde_json::from_str(&t).unwrap();
                if id["d"]["authentication"].as_str()
                    != Some(&auth_string(password, SALT, CHALLENGE))
                {
                    let _ = ws.close(Some(tungstenite::protocol::CloseFrame {
                        code: 4009u16.into(),
                        reason: "Authentication failed.".into(),
                    }));
                    let _ = ws.flush();
                    std::thread::sleep(Duration::from_millis(100));
                    return;
                }
                send(&mut ws, json!({"op":2,"d":{"negotiatedRpcVersion":1}}));
                // Unsolicited event to verify the client ignores it.
                send(
                    &mut ws,
                    json!({"op":5,"d":{"eventType":"Noise","eventIntent":1}}),
                );
                loop {
                    let Ok(msg) = ws.read() else { return };
                    let Message::Text(t) = msg else { continue };
                    let v: Value = serde_json::from_str(&t).unwrap();
                    let d = &v["d"];
                    let rt = d["requestType"].as_str().unwrap_or("").to_string();
                    let rd = d["requestData"].clone();
                    let mut s = st.lock().unwrap();
                    s.log.push((rt.clone(), rd.clone()));
                    let name = rd["inputName"].as_str().unwrap_or("").to_string();
                    let src = rd["sourceName"].as_str().unwrap_or("").to_string();
                    let fname = rd["filterName"].as_str().unwrap_or("").to_string();
                    let (ok, code, data) = match rt.as_str() {
                        "GetVersion" => (true, 100, json!({"obsVersion":"31.0.0"})),
                        "GetInputList" => (
                            true,
                            100,
                            json!({"inputs": s.kinds.iter().map(|(n,k)| json!({"inputName":n,"inputKind":k})).collect::<Vec<_>>()}),
                        ),
                        "GetInputAudioSyncOffset" => match s.offsets.get(&name) {
                            Some(o) => (true, 100, json!({"inputAudioSyncOffset": o})),
                            None => (false, 604, json!({})),
                        },
                        "SetInputAudioSyncOffset" => {
                            let o = rd["inputAudioSyncOffset"].as_i64().unwrap();
                            if !(-950..=20000).contains(&o) || !s.offsets.contains_key(&name) {
                                (false, 402, json!({}))
                            } else {
                                s.offsets.insert(name, o);
                                (true, 100, json!({}))
                            }
                        }
                        "GetInputAudioMonitorType" => {
                            (true, 100, json!({"monitorType":"OBS_MONITORING_TYPE_NONE"}))
                        }
                        "GetInputMute" => (true, 100, json!({"inputMuted": false})),
                        "GetSourceFilter" => match s.filters.get(&(src, fname)) {
                            Some(f) => (true, 100, json!({"filterSettings": f})),
                            None => (false, 600, json!({})),
                        },
                        "CreateSourceFilter" => {
                            assert_eq!(rd["filterKind"], "gpu_delay");
                            s.filters.insert((src, fname), rd["filterSettings"].clone());
                            (true, 100, json!({}))
                        }
                        "SetSourceFilterSettings" => {
                            s.filters.insert((src, fname), rd["filterSettings"].clone());
                            (true, 100, json!({}))
                        }
                        "RemoveSourceFilter" => {
                            s.filters.remove(&(src, fname));
                            (true, 100, json!({}))
                        }
                        _ => (false, 204, json!({})),
                    };
                    drop(s);
                    send(
                        &mut ws,
                        json!({"op":7,"d":{"requestType":rt,"requestId":d["requestId"],
                            "requestStatus":{"result":ok,"code":code,"comment": if ok {Value::Null} else {json!("mock error")}},
                            "responseData":data}}),
                    );
                }
            });
        }
    });
    (port, state)
}

#[test]
fn auth_and_requests() {
    let (port, state) = mock_obs("hunter2");
    assert!(ObsClient::connect("127.0.0.1", port, Some("wrong")).is_err());
    assert!(ObsClient::connect("127.0.0.1", port, None).is_err());
    let mut c = ObsClient::connect("127.0.0.1", port, Some("hunter2")).unwrap();
    assert_eq!(c.obs_version, "31.0.0");
    let inputs = c.input_list().unwrap();
    assert_eq!(inputs.len(), 4);
    assert_eq!(c.sync_offset("Instrumental").unwrap(), 25);
    c.set_sync_offset("Mic/Aux", 180).unwrap();
    assert_eq!(c.sync_offset("Mic/Aux").unwrap(), 180);
    assert!(
        c.set_sync_offset("Mic/Aux", 25_000).is_err(),
        "range checked client-side"
    );
    assert!(c.sync_offset("Avatar").is_err());
    // Request wire format.
    let s = state.lock().unwrap();
    let (rt, rd) = s
        .log
        .iter()
        .find(|(t, _)| t == "SetInputAudioSyncOffset")
        .unwrap();
    assert_eq!(rt, "SetInputAudioSyncOffset");
    assert_eq!(
        rd,
        &json!({"inputName":"Mic/Aux","inputAudioSyncOffset":180})
    );
    assert!(!s
        .log
        .iter()
        .any(|(_, d)| d["inputAudioSyncOffset"] == 25_000));
}

#[test]
fn video_delay_filter_lifecycle() {
    let (port, state) = mock_obs("pw");
    let mut c = ObsClient::connect("127.0.0.1", port, Some("pw")).unwrap();
    assert_eq!(c.video_delay("Avatar").unwrap(), None);
    c.set_video_delay("Avatar", 120).unwrap();
    assert_eq!(c.video_delay("Avatar").unwrap(), Some(120));
    c.set_video_delay("Avatar", 200).unwrap();
    assert_eq!(c.video_delay("Avatar").unwrap(), Some(200));
    assert!(c.set_video_delay("Avatar", 900).is_err());
    c.remove_video_delay("Avatar").unwrap();
    assert_eq!(c.video_delay("Avatar").unwrap(), None);
    let n_create = state
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|(t, _)| t == "CreateSourceFilter")
        .count();
    assert_eq!(n_create, 1);
}

fn next_offsets(w: &ObsWorker) -> Vec<duetsync::obs::ApplyResult> {
    loop {
        match w
            .events
            .recv_timeout(Duration::from_secs(5))
            .expect("obs event")
        {
            ObsEvent::Offsets { results, .. } => return results,
            ObsEvent::Error(e) | ObsEvent::Disconnected(e) => panic!("{e}"),
            _ => {}
        }
    }
}

fn next_inputs(w: &ObsWorker) -> Vec<duetsync::obs::InputState> {
    loop {
        match w
            .events
            .recv_timeout(Duration::from_secs(5))
            .expect("obs event")
        {
            ObsEvent::Inputs(i) => return i,
            ObsEvent::Error(e) | ObsEvent::Disconnected(e) => panic!("{e}"),
            _ => {}
        }
    }
}

#[test]
fn apply_recalibrate_restore_via_worker() {
    let (port, state) = mock_obs("pw");
    let w = ObsWorker::start();
    w.send(ObsCmd::Connect {
        host: "127.0.0.1".into(),
        port,
        password: Some("pw".into()),
    });
    let inputs = next_inputs(&w);
    let audio: Vec<_> = inputs.iter().filter(|i| i.is_audio()).collect();
    assert_eq!(audio.len(), 3, "window capture is not audio");

    let mut ledger = OffsetLedger::default();
    let roles = |guest: f64| {
        vec![
            SourceLatency {
                input: "Mic/Aux".into(),
                latency_ms: 0.0,
            },
            SourceLatency {
                input: "Instrumental".into(),
                latency_ms: 0.0,
            },
            SourceLatency {
                input: "Discord".into(),
                latency_ms: guest,
            },
        ]
    };
    for guest in [150.0, 150.0, 90.0] {
        let cur: BTreeMap<String, i64> = state.lock().unwrap().offsets.clone();
        let (changes, _) = plan(&roles(guest), 20.0, &cur, &ledger).unwrap();
        w.send(ObsCmd::SetOffsets {
            restore: false,
            targets: changes
                .iter()
                .map(|c| (c.input.clone(), c.target_ms))
                .collect(),
        });
        let res = next_offsets(&w);
        assert!(res.iter().all(|r| r.verified()), "{res:?}");
        changes.iter().for_each(|c| ledger.record(c));
        let s = state.lock().unwrap();
        let g = guest as i64;
        assert_eq!(s.offsets["Mic/Aux"], g + 20);
        assert_eq!(
            s.offsets["Instrumental"],
            25 + g + 20,
            "user's 25 ms baseline kept"
        );
        assert_eq!(s.offsets["Discord"], 20);
    }
    // Restore original offsets.
    let targets: Vec<(String, i64)> = ledger
        .baseline
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    w.send(ObsCmd::SetOffsets {
        restore: true,
        targets,
    });
    assert!(next_offsets(&w).iter().all(|r| r.verified()));
    let s = state.lock().unwrap();
    assert_eq!(s.offsets["Mic/Aux"], 0);
    assert_eq!(s.offsets["Instrumental"], 25);
    assert_eq!(s.offsets["Discord"], 0);
}

#[test]
fn worker_reports_unreachable_obs() {
    let w = ObsWorker::start();
    w.send(ObsCmd::Connect {
        host: "127.0.0.1".into(),
        port: 1,
        password: None,
    });
    match w.events.recv_timeout(Duration::from_secs(6)).unwrap() {
        ObsEvent::Disconnected(e) | ObsEvent::Error(e) => {
            assert!(e.contains("cannot reach OBS"), "{e}")
        }
        e => panic!("unexpected {e:?}"),
    }
}
