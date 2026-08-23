use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use sdrmm_cps::{CpsError, SerialBackend, SerialLink};
use sdrmm_wire::cps::{
    CpsCodeplugDetail, CpsConvertResponse, CpsJob, CpsJobState, CpsLibraryResponse,
    CpsPortsResponse, IssueScope, RadioIdent, RadioModelsResponse,
};

use super::*;

const PORT: &str = "/dev/fixture-anytone";
const MODEL: &str = "anytone-d890uv";

#[derive(Default)]
struct Flash {
    bytes: HashMap<u32, u8>,
    writes: usize,
}

#[derive(Clone, Default)]
struct EmulatedRadio {
    flash: Arc<Mutex<Flash>>,
}

impl EmulatedRadio {
    fn loaded() -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/cps/anytone-d890uv-v100.img");
        let raw = std::fs::read(path).expect("the recorded D890UV image");
        let image = sdrmm_cps::Image::from_bytes(&raw).expect("parse");
        let radio = Self::default();
        {
            let mut flash = radio.lock();
            for (addr, data) in image.segments() {
                for (offset, byte) in data.iter().enumerate() {
                    flash.bytes.insert(addr + offset as u32, *byte);
                }
            }
        }
        radio
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Flash> {
        self.flash
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn writes(&self) -> usize {
        self.lock().writes
    }
}

impl SerialBackend for EmulatedRadio {
    fn ports(&self) -> Result<Vec<serialport::SerialPortInfo>, String> {
        Ok(vec![serialport::SerialPortInfo {
            port_name: PORT.to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid: 0x0483,
                pid: 0x5740,
                serial_number: Some("fixture".to_owned()),
                manufacturer: Some("STMicroelectronics".to_owned()),
                product: Some("STM32 Virtual ComPort".to_owned()),
            }),
        }])
    }

    fn open(&self, port: &str, _baud: u32) -> Result<Box<dyn SerialLink>, String> {
        if port != PORT {
            return Err(format!("no radio on {port}"));
        }
        Ok(Box::new(EmulatedLink {
            radio: self.clone(),
            pending: Vec::new(),
        }))
    }
}

struct EmulatedLink {
    radio: EmulatedRadio,
    pending: Vec<u8>,
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

impl SerialLink for EmulatedLink {
    fn send(&mut self, data: &[u8]) -> Result<(), CpsError> {
        if data == b"PROGRAM" {
            self.pending.extend_from_slice(b"QX\x06");
            return Ok(());
        }
        if data == b"END" {
            self.pending.push(0x06);
            return Ok(());
        }
        if data == [0x02] {
            let mut frame = vec![0u8; 16];
            frame[0] = b'I';
            frame[1..7].copy_from_slice(b"D890UV");
            frame[8] = 0x0e;
            frame[9..13].copy_from_slice(b"V100");
            frame[15] = 0x06;
            self.pending.extend_from_slice(&frame);
            return Ok(());
        }
        if data.len() == 6 && data[0] == b'R' {
            let addr = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            let flash = self.radio.lock();
            let mut frame = vec![0u8; 24];
            frame[0] = b'W';
            frame[1..5].copy_from_slice(&addr.to_be_bytes());
            frame[5] = 16;
            for offset in 0..16u32 {
                frame[6 + offset as usize] =
                    flash.bytes.get(&(addr + offset)).copied().unwrap_or(0xff);
            }
            frame[22] = checksum(&frame[1..22]);
            frame[23] = 0x06;
            self.pending.extend_from_slice(&frame);
            return Ok(());
        }
        if data.len() == 24 && data[0] == b'W' {
            let addr = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            let mut flash = self.radio.lock();
            for offset in 0..16u32 {
                flash.bytes.insert(addr + offset, data[6 + offset as usize]);
            }
            flash.writes += 1;
            self.pending.push(0x06);
            return Ok(());
        }
        Err(CpsError::Transport(format!(
            "unexpected {} byte frame",
            data.len()
        )))
    }

    fn receive(&mut self, buffer: &mut [u8], _timeout: Duration) -> Result<(), CpsError> {
        if self.pending.len() < buffer.len() {
            return Err(CpsError::Timeout {
                wanted: buffer.len(),
                got: self.pending.len(),
            });
        }
        let rest = self.pending.split_off(buffer.len());
        buffer.copy_from_slice(&self.pending);
        self.pending = rest;
        Ok(())
    }

    fn discard_input(&mut self) -> Result<(), CpsError> {
        self.pending.clear();
        Ok(())
    }

    fn set_control_lines(&mut self, _asserted: bool) -> Result<(), CpsError> {
        Ok(())
    }
}

fn radio_router() -> (Router, EmulatedRadio) {
    let radio = EmulatedRadio::loaded();
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let mut state = state_over(store);
    state.cps = Arc::new(crate::cps::CpsHub::with_backend(Arc::new(radio.clone())));
    let (router, background) = router_with_state(state, &ServerOptions::default());
    background.detach();
    (router, radio)
}

async fn settle(app: &Router, id: u64) -> CpsJob {
    for _ in 0..600 {
        let (status, body) =
            request(app.clone(), "GET", &format!("/api/cps/jobs/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        let job: CpsJob = serde_json::from_slice(&body).expect("json");
        if job.state.is_final() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the transfer never finished");
}

#[tokio::test]
async fn the_model_list_names_every_radio_this_build_can_program() {
    let app = test_router();
    let (status, body) = request(app, "GET", "/api/cps/models", None).await;
    assert_eq!(status, StatusCode::OK);
    let models: RadioModelsResponse = serde_json::from_slice(&body).expect("json");
    assert!(models.models.iter().any(|model| model.id == MODEL));
    assert!(models.models.iter().any(|model| model.id == "radtel-rt4d"));
    let anytone = models
        .models
        .iter()
        .find(|model| model.id == MODEL)
        .expect("the D890UV");
    assert!(anytone.needs_explicit_selection);
    assert_eq!(anytone.limits.channels, 4096);
}

#[tokio::test]
async fn port_discovery_offers_the_radio_and_names_the_models_that_could_be_on_it() {
    let (app, _radio) = radio_router();
    let (status, body) = request(app, "GET", "/api/cps/ports", None).await;
    assert_eq!(status, StatusCode::OK);
    let ports: CpsPortsResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(ports.ports.len(), 1);
    assert_eq!(ports.ports[0].port, PORT);
    assert_eq!(ports.ports[0].candidate_models, vec![MODEL.to_owned()]);
}

#[tokio::test]
async fn identify_reports_what_the_radio_answers() {
    let (app, _radio) = radio_router();
    let (status, body) = request(
        app,
        "POST",
        "/api/cps/identify",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ident: RadioIdent = serde_json::from_slice(&body).expect("json");
    assert_eq!(ident.reported_model, "D890UV");
    assert_eq!(ident.firmware.as_deref(), Some("V100"));
}

#[tokio::test]
async fn a_read_stores_the_codeplug_it_pulled_off_the_radio() {
    let (app, _radio) = radio_router();
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(
            r#"{{"model_id":"{MODEL}","port":"{PORT}","name":"Bench radio"}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let job = settle(&app, job.id).await;
    assert_eq!(job.state, CpsJobState::Done, "{:?}", job.error);
    let codeplug_id = job.codeplug_id.expect("the read stored a codeplug");

    let (status, body) = request(
        app.clone(),
        "GET",
        &format!("/api/cps/codeplugs/{codeplug_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: CpsCodeplugDetail = serde_json::from_slice(&body).expect("json");
    assert_eq!(detail.info.name, "Bench radio");
    assert_eq!(detail.info.counts.channels, 35);
    assert_eq!(detail.codeplug.channels[0].name, "PMR FM 1");
    assert_eq!(detail.codeplug.meta.firmware.as_deref(), Some("V100"));
}

#[tokio::test]
async fn writing_back_what_was_read_touches_nothing_on_the_radio() {
    let (app, radio) = radio_router();
    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let read = settle(&app, job.id).await;
    let codeplug_id = read.codeplug_id.expect("stored");
    assert_eq!(radio.writes(), 0);

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/cps/write",
        Some(&format!(
            r#"{{"model_id":"{MODEL}","port":"{PORT}","codeplug_id":{codeplug_id},"confirm":true}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let job = settle(&app, job.id).await;
    assert_eq!(job.state, CpsJobState::Done, "{:?}", job.error);
    assert_eq!(
        radio.writes(),
        0,
        "a codeplug written straight back must not change a byte"
    );
}

#[tokio::test]
async fn an_unconfirmed_write_is_refused_before_the_port_is_opened() {
    let (app, radio) = radio_router();
    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let codeplug_id = settle(&app, job.id).await.codeplug_id.expect("stored");

    let (status, body) = request(
        app,
        "POST",
        "/api/cps/write",
        Some(&format!(
            r#"{{"model_id":"{MODEL}","port":"{PORT}","codeplug_id":{codeplug_id},"confirm":false}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert!(error.error.contains("confirm"), "{}", error.error);
    assert_eq!(radio.writes(), 0);
}

#[tokio::test]
async fn editing_a_codeplug_writes_only_the_blocks_that_changed() {
    let (app, radio) = radio_router();
    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let codeplug_id = settle(&app, job.id).await.codeplug_id.expect("stored");

    let (_, body) = request(
        app.clone(),
        "GET",
        &format!("/api/cps/codeplugs/{codeplug_id}"),
        None,
    )
    .await;
    let mut detail: CpsCodeplugDetail = serde_json::from_slice(&body).expect("json");
    detail.codeplug.channels[0].name = "Renamed".to_owned();
    for zone in &mut detail.codeplug.zones {
        for name in &mut zone.channels_a {
            if name == "PMR FM 1" {
                *name = "Renamed".to_owned();
            }
        }
    }
    let update = serde_json::json!({
        "name": detail.info.name,
        "model_id": detail.info.model_id,
        "codeplug": detail.codeplug,
    });
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/cps/codeplugs/{codeplug_id}"),
        Some(&update.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/write",
        Some(&format!(
            r#"{{"model_id":"{MODEL}","port":"{PORT}","codeplug_id":{codeplug_id},"confirm":true}}"#
        )),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let job = settle(&app, job.id).await;
    assert_eq!(job.state, CpsJobState::Done, "{:?}", job.error);
    let writes = radio.writes();
    assert!(
        (1..=8).contains(&writes),
        "one renamed channel should touch a handful of 16-byte blocks, not {writes}"
    );
}

#[tokio::test]
async fn a_codeplug_converted_for_another_radio_reports_what_would_not_fit() {
    let (app, _radio) = radio_router();
    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let codeplug_id = settle(&app, job.id).await.codeplug_id.expect("stored");

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/cps/codeplugs/{codeplug_id}/convert"),
        Some(r#"{"target_model_id":"radtel-rt4d","name":"On the RT4D","store":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let converted: CpsConvertResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(converted.report.target_model, "radtel-rt4d");
    assert_eq!(converted.report.before.channels, 35);
    assert_eq!(converted.report.after.channels, 35);
    assert!(converted.codeplug.scan_lists.is_empty());
    assert!(
        converted
            .report
            .issues
            .iter()
            .any(|issue| issue.scope == IssueScope::ScanList),
        "the RT4D has no scan lists and the report must say so"
    );
    let stored = converted.stored_id.expect("stored under a new id");

    let (_, body) = request(app.clone(), "GET", "/api/cps/library", None).await;
    let library: CpsLibraryResponse = serde_json::from_slice(&body).expect("json");
    assert!(library.codeplugs.iter().any(|item| item.id == stored));
}

#[tokio::test]
async fn an_operator_is_stamped_onto_the_codeplug_that_gets_written() {
    let (app, _radio) = radio_router();
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/cps/users",
        Some(r#"{"name":"Julian","callsign":"OE1TEST","dmr_id":2328001}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let user: CreatedRowId = serde_json::from_slice(&body).expect("json");

    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let codeplug_id = settle(&app, job.id).await.codeplug_id.expect("stored");

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/cps/codeplugs/{codeplug_id}/convert"),
        Some(&format!(
            r#"{{"target_model_id":"{MODEL}","user_id":{}}}"#,
            user.id
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let converted: CpsConvertResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(converted.codeplug.radio_ids[0].number, 2_328_001);
    assert_eq!(converted.codeplug.radio_ids[0].name, "OE1TEST");
    assert_eq!(
        converted.codeplug.settings.default_radio_id.as_deref(),
        Some("OE1TEST")
    );
}

#[tokio::test]
async fn a_second_radio_and_its_owner_are_kept_side_by_side() {
    let app = test_router();
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/cps/users",
        Some(r#"{"name":"Club station","callsign":"OE1XYZ","dmr_id":2328002}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let owner: CreatedRowId = serde_json::from_slice(&body).expect("json");

    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/cps/devices",
        Some(&format!(
            r#"{{"name":"Handheld","model_id":"{MODEL}","owner_id":{}}}"#,
            owner.id
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/cps/devices",
        Some(r#"{"name":"Spare","model_id":"radtel-rt4d"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(app.clone(), "GET", "/api/cps/library", None).await;
    assert_eq!(status, StatusCode::OK);
    let library: CpsLibraryResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(library.users.len(), 1);
    assert_eq!(library.devices.len(), 2);
    assert_eq!(library.devices[0].owner_id, Some(owner.id));

    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/cps/devices",
        Some(r#"{"name":"Spare","model_id":"radtel-rt4d"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = request(
        app,
        "POST",
        "/api/cps/devices",
        Some(r#"{"name":"Mystery","model_id":"not-a-radio"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn merging_one_codeplug_into_another_keeps_both_sets_of_channels() {
    let (app, _radio) = radio_router();
    let (_, body) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"{MODEL}","port":"{PORT}"}}"#)),
    )
    .await;
    let job: CpsJob = serde_json::from_slice(&body).expect("json");
    let source = settle(&app, job.id).await.codeplug_id.expect("stored");

    let target = serde_json::json!({
        "name": "Empty plan",
        "model_id": MODEL,
        "codeplug": {
            "version": sdrmm_wire::cps::CODEPLUG_VERSION,
            "channels": [{
                "name": "Calling",
                "rx_hz": 145_500_000u64,
                "tx_hz": 145_500_000u64,
                "mode": "fm"
            }]
        }
    });
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/cps/codeplugs",
        Some(&target.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let target: CreatedRowId = serde_json::from_slice(&body).expect("json");

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/cps/codeplugs/{}/merge", target.id),
        Some(&format!(
            r#"{{"source_id":{source},"mode":"union","parts":["channels","contacts"]}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let merged: CpsConvertResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(merged.codeplug.channels.len(), 36);
    assert_eq!(merged.codeplug.channels[0].name, "Calling");
    assert!(
        merged
            .codeplug
            .channels
            .iter()
            .any(|channel| channel.name == "PMR FM 1")
    );
    assert_eq!(merged.codeplug.contacts.len(), 1);
}

#[tokio::test]
async fn a_job_for_an_unknown_model_or_id_is_refused() {
    let (app, _radio) = radio_router();
    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/cps/read",
        Some(&format!(r#"{{"model_id":"nothing","port":"{PORT}"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(app.clone(), "GET", "/api/cps/jobs/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(app, "GET", "/api/cps/codeplugs/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
