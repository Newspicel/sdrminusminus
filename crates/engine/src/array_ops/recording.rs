use std::path::Path;

use sdrmm_device::lock;
use sdrmm_recorder::{CollectionArray, CollectionWriter, LaneMeta, SigmfError};
use sdrmm_wire::{
    ArrayRecordingRequest, DcArtifact, LaneKey, NoiseSource, ServerEvent, StateScope,
    recording_stem_valid,
};

use super::{find, find_mut};
use crate::{
    Engine, EngineError,
    array::{ArrayRecording, Command, record_tap},
};

const MAX_STEM_TRIES: u32 = 1_000;
const BAD_NAME: &str = "Bad name";

struct Plan {
    lanes: Vec<LaneMeta>,
    rate: f64,
    array: CollectionArray,
}

fn base_name(
    node: &str,
    request: &ArrayRecordingRequest,
    at: jiff::Timestamp,
) -> Result<String, EngineError> {
    match &request.name {
        Some(name) if recording_stem_valid(name) => Ok(name.clone()),
        Some(_) => Err(EngineError::Recording(BAD_NAME.to_owned())),
        None => {
            let node: String = node
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            Ok(format!("array_{node}_{}", at.strftime("%Y%m%dT%H%M%SZ")))
        }
    }
}

fn create_collection(
    dir: &Path,
    base: &str,
    plan: &Plan,
) -> Result<(CollectionWriter, String), EngineError> {
    for attempt in 1..=MAX_STEM_TRIES {
        let name = if attempt == 1 {
            base.to_owned()
        } else {
            format!("{base}-{attempt}")
        };
        match CollectionWriter::create(&dir.join(&name), &plan.lanes, plan.rate, plan.array.clone())
        {
            Ok(writer) => return Ok((writer, name)),
            Err(SigmfError::StemTaken(_)) => {}
            Err(error) => {
                return Err(EngineError::RecordingIo(format!(
                    "create {}: {error}",
                    dir.join(&name).display()
                )));
            }
        }
    }
    Err(EngineError::Recording(format!("no free name for {base}")))
}

impl Engine {
    fn recording_plan(&self, node: &str) -> Result<Plan, EngineError> {
        let inner = self.lock();
        let state = find(&inner, node)?;
        if state.recording.is_some() {
            return Err(EngineError::Recording("already recording".to_owned()));
        }
        let lanes = state
            .spec
            .lanes
            .iter()
            .enumerate()
            .map(|(slot, lane)| {
                let device = lane
                    .and_then(|lane| inner.device_sets.get(&lane.device_set))
                    .map(|device| device.info.id())
                    .unwrap_or_default();
                LaneMeta {
                    lane: LaneKey {
                        device,
                        stream: lane.map_or(0, |lane| lane.stream),
                    },
                    center_hz: state
                        .frame
                        .lane_centers_hz
                        .get(slot)
                        .copied()
                        .unwrap_or(state.frame.center_hz),
                }
            })
            .collect();
        let anchor = state.anchor.and_then(|ds| inner.device_sets.get(&ds));
        Ok(Plan {
            lanes,
            rate: state.frame.sample_rate,
            array: CollectionArray {
                node: node.to_owned(),
                tier: state.tier.tier,
                geometry: state.spec.settings.geometry.clone(),
                noise_source: anchor
                    .map_or(NoiseSource::None, |device| device.capabilities.noise_source),
                retune_keeps_phase: state.tier.keeps_phase,
                dc_artifact: anchor
                    .map_or(DcArtifact::None, |device| device.capabilities.dc_artifact),
            },
        })
    }

    pub fn start_array_recording(
        &self,
        node: &str,
        request: ArrayRecordingRequest,
    ) -> Result<String, EngineError> {
        let _edits = lock(&self.array_edits);
        let dir = self.recordings_dir.clone().ok_or_else(|| {
            EngineError::Recording("no recordings directory configured".to_owned())
        })?;
        let plan = self.recording_plan(node)?;
        let started_at = jiff::Timestamp::now();
        let base = base_name(node, &request, started_at)?;
        std::fs::create_dir_all(&dir).map_err(|error| {
            EngineError::RecordingIo(format!("create {}: {error}", dir.display()))
        })?;
        let (writer, stem) = create_collection(&dir, &base, &plan)?;
        let (tap, reader) = record_tap(plan.lanes.len(), plan.rate);
        let recording = ArrayRecording::start(
            "sdrmm-array-rec".to_owned(),
            stem.clone(),
            started_at.to_string(),
            writer,
            &tap,
            reader,
        )?;
        let mut pending = Some(recording);
        let sent = {
            let mut inner = self.lock();
            let sent = find_mut(&mut inner, node).and_then(|state| {
                state.send(Command::Record { writer: Some(tap) })?;
                state.recording = pending.take();
                Ok(())
            });
            if sent.is_ok() {
                inner.revision += 1;
            }
            sent
        };
        if let Some(recording) = pending {
            self.finish_recording(node, recording);
        }
        sent?;
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        Ok(stem)
    }

    pub fn stop_array_recording(&self, node: &str) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        let recording = {
            let mut inner = self.lock();
            let state = find_mut(&mut inner, node)?;
            if state.recording.is_none() {
                return Err(EngineError::Recording("not recording".to_owned()));
            }
            state.send(Command::Record { writer: None })?;
            inner.revision += 1;
            find_mut(&mut inner, node)?.recording.take()
        };
        if let Some(recording) = recording {
            self.finish_recording(node, recording);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        Ok(())
    }

    pub(super) fn finish_recording(&self, node: &str, recording: ArrayRecording) {
        let finished = recording.finish();
        if let Some(error) = finished.error {
            tracing::warn!(array = %node, stem = %finished.stem, %error, "array recording ended with an error");
            self.emit(ServerEvent::Error {
                message: format!("Recording {}: {error}", finished.stem),
            });
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Recordings,
        });
    }
}
