use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

use sdrmm_wire::{
    NmeaDeviceInfo, NmeaDevicesResponse, NodeBody, PatchGraph, PositionFix, PositionSource,
    ServerEvent,
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::broadcast,
    task::JoinHandle,
};
use tokio_serial::{SerialPortBuilderExt, SerialPortInfo, SerialPortType};

use crate::{AppState, Store, workspace};

const EVENT_CAPACITY: usize = 128;
const RETRY_DELAY: Duration = Duration::from_secs(3);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const STABLE_SESSION: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_DEVICE_PUBLISH_INTERVAL: Duration = Duration::from_millis(50);
const GPSD_MAX_LINE: usize = 16 * 1024;
const NMEA_MAX_LINE: usize = 512;
const MAX_ERROR_LEN: usize = 256;

#[derive(Clone, Debug, PartialEq)]
struct PositionState {
    fix: Option<PositionFix>,
    error: Option<String>,
}

#[derive(Default, PartialEq, Eq)]
struct GpsConfiguration {
    sources: HashMap<String, PositionSource>,
    routes: Vec<(String, String)>,
}

struct SourceTask {
    source: PositionSource,
    handle: JoinHandle<()>,
}

#[derive(Clone)]
struct RouteState {
    engine: Arc<sdrmm_engine::Engine>,
    store: Arc<Store>,
}

impl From<&AppState> for RouteState {
    fn from(state: &AppState) -> Self {
        Self {
            engine: state.engine.clone(),
            store: state.store.clone(),
        }
    }
}

pub(crate) struct GpsHub {
    latest: Arc<Mutex<HashMap<String, PositionState>>>,
    device_publish_at: Mutex<HashMap<String, Instant>>,
    device_publish_interval_ms: AtomicU64,
    tasks: Mutex<HashMap<String, SourceTask>>,
    configuration: Mutex<GpsConfiguration>,
    route_signal: Option<SyncSender<RouteState>>,
    route_worker: Option<std::thread::JoinHandle<()>>,
    clear_before_route: Arc<AtomicBool>,
    events: broadcast::Sender<ServerEvent>,
}

impl Default for GpsHub {
    fn default() -> Self {
        let latest = Arc::new(Mutex::new(HashMap::<String, PositionState>::new()));
        let route_latest = latest.clone();
        let clear_before_route = Arc::new(AtomicBool::new(false));
        let route_clear = clear_before_route.clone();
        let (route_signal, route_rx) = std::sync::mpsc::sync_channel::<RouteState>(1);
        let route_worker = match std::thread::Builder::new()
            .name("sdrmm-gps-route".to_owned())
            .spawn(move || {
                while let Ok(state) = route_rx.recv() {
                    if route_clear.swap(false, Ordering::AcqRel) {
                        clear_position_consumers(&state);
                    }
                    let positions = route_latest
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    for (node, current) in positions {
                        route_position(&state, &node, current.fix);
                    }
                }
            }) {
            Ok(worker) => Some(worker),
            Err(error) => {
                tracing::error!(%error, "could not start GPS routing thread");
                None
            }
        };
        Self {
            latest,
            device_publish_at: Mutex::new(HashMap::new()),
            device_publish_interval_ms: AtomicU64::new(
                MIN_DEVICE_PUBLISH_INTERVAL.as_millis() as u64
            ),
            tasks: Mutex::new(HashMap::new()),
            configuration: Mutex::new(GpsConfiguration::default()),
            route_signal: Some(route_signal),
            route_worker,
            clear_before_route,
            events: broadcast::channel(EVENT_CAPACITY).0,
        }
    }
}

impl GpsHub {
    #[cfg(test)]
    pub(crate) fn set_device_publish_interval(&self, interval: Duration) {
        self.device_publish_interval_ms
            .store(interval.as_millis() as u64, Ordering::Relaxed);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    pub(crate) fn snapshot(&self) -> Vec<ServerEvent> {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(node, state)| position_event(node, state))
            .collect()
    }

    pub(crate) fn reconcile(self: &Arc<Self>, state: &AppState) {
        let active = match state.store.active_workspace() {
            Ok(active) => active,
            Err(error) => {
                tracing::warn!(%error, "could not reconcile GPS sources");
                return;
            }
        };
        let wanted = active
            .as_ref()
            .map(|active| {
                active
                    .snapshot
                    .graph
                    .nodes
                    .iter()
                    .filter_map(|node| match &node.body {
                        NodeBody::Gps(gps) => Some((node.id.clone(), gps.source.clone())),
                        _ => None,
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let mut routes = active
            .as_ref()
            .map(|active| {
                active
                    .snapshot
                    .graph
                    .edges
                    .iter()
                    .filter(|edge| edge.from.port == "position" && edge.to.port == "position")
                    .map(|edge| (edge.from.node.clone(), edge.to.node.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        routes.sort();

        let next = GpsConfiguration {
            sources: wanted.clone(),
            routes,
        };
        let (configuration_changed, changed_sources) = {
            let mut current = self
                .configuration
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let changed_sources = next
                .sources
                .keys()
                .filter(|node| current.sources.get(*node) != next.sources.get(*node))
                .cloned()
                .collect::<Vec<_>>();
            let changed = *current != next;
            *current = next;
            (changed, changed_sources)
        };

        if configuration_changed {
            self.clear_before_route.store(true, Ordering::Release);
        }

        {
            let mut latest = self
                .latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            latest.retain(|node, _| wanted.contains_key(node));
            for node in &changed_sources {
                latest.remove(node);
            }
        }
        self.device_publish_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|node, _| matches!(wanted.get(node), Some(PositionSource::Device)));
        for node in &changed_sources {
            self.publish_state(
                state,
                node,
                None,
                Some("waiting for a position fix".to_owned()),
            );
        }

        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed: Vec<String> = tasks
            .keys()
            .filter(|node| !wanted.contains_key(*node))
            .cloned()
            .collect();
        for node in removed {
            if let Some(task) = tasks.remove(&node) {
                task.handle.abort();
            }
        }

        for (node, source) in &wanted {
            let unchanged = tasks
                .get(node)
                .is_some_and(|task| task.source == *source && !task.handle.is_finished());
            if unchanged {
                continue;
            }
            if let Some(old) = tasks.remove(node) {
                old.handle.abort();
            }
            if matches!(source, PositionSource::Device) {
                continue;
            }
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                self.publish_state(
                    state,
                    node,
                    None,
                    Some("no async runtime is available for this GPS source".to_owned()),
                );
                continue;
            };
            let hub = self.clone();
            let app = state.clone();
            let task_node = node.clone();
            let task_source = source.clone();
            let handle = runtime.spawn(async move {
                match task_source {
                    PositionSource::Gpsd { address } => {
                        run_gpsd(hub, app, task_node, address).await
                    }
                    PositionSource::Nmea {
                        device,
                        baud,
                        update_interval_ms,
                    } => {
                        run_nmea(
                            hub,
                            app,
                            task_node,
                            device,
                            baud,
                            Duration::from_millis(u64::from(update_interval_ms)),
                        )
                        .await;
                    }
                    PositionSource::Device => {}
                }
            });
            tasks.insert(
                node.clone(),
                SourceTask {
                    source: source.clone(),
                    handle,
                },
            );
        }
        drop(tasks);
        self.route_current(state);
    }

    pub(crate) fn publish_device(
        &self,
        state: &AppState,
        node: &str,
        fix: Option<PositionFix>,
        error: Option<String>,
    ) -> Result<(), String> {
        let active = state
            .store
            .active_workspace()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "no active workspace".to_owned())?;
        let valid = active.snapshot.graph.node(node).is_some_and(|candidate| {
            matches!(
                &candidate.body,
                NodeBody::Gps(gps) if gps.source == PositionSource::Device
            )
        });
        if !valid {
            return Err(
                "position node is not a device GPS source in the active workspace".to_owned(),
            );
        }
        validate_update(fix.as_ref(), error.as_deref())?;
        let too_fast = {
            let mut published = self
                .device_publish_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if published.get(node).is_some_and(|last| {
                last.elapsed()
                    < Duration::from_millis(self.device_publish_interval_ms.load(Ordering::Relaxed))
            }) {
                true
            } else {
                published.insert(node.to_owned(), Instant::now());
                false
            }
        };
        if too_fast {
            return Err("position updates are limited to 20 Hz per node".to_owned());
        }
        self.publish_state(state, node, fix, error.map(limit_error));
        Ok(())
    }

    fn publish_state(
        &self,
        state: &AppState,
        node: &str,
        fix: Option<PositionFix>,
        error: Option<String>,
    ) {
        let next = PositionState { fix, error };
        let changed = {
            let mut latest = self
                .latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if latest.get(node) == Some(&next) {
                false
            } else {
                latest.insert(node.to_owned(), next.clone());
                true
            }
        };
        if changed {
            self.queue_route(state);
            let _ = self.events.send(position_event(node, &next));
        }
    }

    pub(crate) fn route_current(&self, state: &AppState) {
        self.queue_route(state);
    }

    fn queue_route(&self, state: &AppState) {
        let Some(route_signal) = &self.route_signal else {
            return;
        };
        match route_signal.try_send(RouteState::from(state)) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                tracing::error!("GPS routing thread stopped");
            }
        }
    }
}

impl Drop for GpsHub {
    fn drop(&mut self) {
        self.route_signal.take();
        if let Some(worker) = self.route_worker.take()
            && worker.join().is_err()
        {
            tracing::error!("GPS routing thread panicked");
        }
    }
}

fn validate_update(fix: Option<&PositionFix>, error: Option<&str>) -> Result<(), String> {
    if fix.is_some() == error.is_some() {
        return Err("position update needs either a fix or an error".to_owned());
    }
    if let Some(fix) = fix {
        fix.validate().map_err(str::to_owned)?;
    }
    if error.is_some_and(str::is_empty) {
        return Err("position error must not be empty".to_owned());
    }
    Ok(())
}

fn position_event(node: &str, state: &PositionState) -> ServerEvent {
    ServerEvent::PositionChanged {
        node: node.to_owned(),
        fix: state.fix.clone(),
        error: state.error.clone(),
    }
}

fn route_position(state: &RouteState, source: &str, fix: Option<PositionFix>) {
    let Ok(Some(active)) = state.store.active_workspace() else {
        return;
    };
    let graph = &active.snapshot.graph;
    let snapshot = state.engine.snapshot();
    let bindings = workspace::bind(graph, &snapshot);
    for target in graph.targets_of(source, "position") {
        let Some(node) = graph.node(target) else {
            continue;
        };
        match node.body {
            NodeBody::Channel(_) => {
                let channel = bindings.iter().find_map(|binding| {
                    binding
                        .channels
                        .iter()
                        .find(|(channel_node, _)| channel_node == target)
                        .map(|(_, channel)| (binding.device_set, *channel))
                });
                if let Some((device_set, channel)) = channel
                    && let Err(error) =
                        state
                            .engine
                            .update_channel_position(device_set, channel, fix.clone())
                {
                    tracing::debug!(%error, node = target, "could not route GPS fix to channel");
                }
            }
            NodeBody::Recorder => {
                let device_set = wired_device_set(graph, &bindings, target);
                if let Some(device_set) = device_set
                    && snapshot
                        .device_sets
                        .iter()
                        .any(|set| set.id == device_set && set.recording.is_some())
                    && let Err(error) = state
                        .engine
                        .update_recording_position(device_set, fix.clone())
                {
                    tracing::debug!(%error, node = target, "could not geotag recording");
                }
            }
            NodeBody::TimeMachine(_) => {
                let device_set = wired_device_set(graph, &bindings, target);
                if let Some(device_set) = device_set
                    && snapshot
                        .device_sets
                        .iter()
                        .any(|set| set.id == device_set && set.time_machine.is_some())
                    && let Err(error) = state
                        .engine
                        .update_time_machine_position(device_set, fix.clone())
                {
                    tracing::debug!(%error, node = target, "could not geotag held history");
                }
            }
            _ => {}
        }
    }
}

fn wired_device_set(
    graph: &PatchGraph,
    bindings: &[workspace::DeviceBinding],
    node: &str,
) -> Option<u32> {
    let device = graph.sources_of(node, "iq").next()?;
    bindings
        .iter()
        .find(|binding| binding.node == device)
        .map(|binding| binding.device_set)
}

fn clear_position_consumers(state: &RouteState) {
    let snapshot = state.engine.snapshot();
    for set in snapshot.device_sets {
        for channel in set.channels {
            if let Err(error) = state
                .engine
                .update_channel_position(set.id, channel.id, None)
            {
                tracing::debug!(%error, channel = channel.id, "could not clear channel GPS fix");
            }
        }
        if set.recording.is_some()
            && let Err(error) = state.engine.update_recording_position(set.id, None)
        {
            tracing::debug!(%error, device_set = set.id, "could not clear recording GPS fix");
        }
        if set.time_machine.is_some()
            && let Err(error) = state.engine.update_time_machine_position(set.id, None)
        {
            tracing::debug!(%error, device_set = set.id, "could not clear held history GPS fix");
        }
    }
}

async fn run_gpsd(hub: Arc<GpsHub>, state: AppState, node: String, address: String) {
    retry_forever(&hub, &state, &node, || {
        gpsd_session(&hub, &state, &node, &address)
    })
    .await;
}

async fn retry_forever<F, Fut>(hub: &GpsHub, state: &AppState, node: &str, mut session: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut retry = RETRY_DELAY;
    loop {
        let started = std::time::Instant::now();
        if let Err(error) = session().await {
            hub.publish_state(state, node, None, Some(limit_error(error)));
        }
        let delay = retry;
        retry = if started.elapsed() >= STABLE_SESSION {
            RETRY_DELAY
        } else {
            retry.saturating_mul(2).min(MAX_RETRY_DELAY)
        };
        tokio::time::sleep(delay).await;
    }
}

async fn gpsd_session(
    hub: &GpsHub,
    state: &AppState,
    node: &str,
    address: &str,
) -> Result<(), String> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| format!("gpsd {address}: connection timed out"))?
        .map_err(|error| format!("gpsd {address}: {error}"))?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(b"?WATCH={\"enable\":true,\"json\":true};\n")
        .await
        .map_err(|error| format!("gpsd watch: {error}"))?;
    let mut reader = BufReader::new(read);
    while let Some(line) =
        tokio::time::timeout(READ_TIMEOUT, read_bounded_line(&mut reader, GPSD_MAX_LINE))
            .await
            .map_err(|_| "gpsd read timed out".to_owned())?
            .map_err(|error| format!("gpsd read: {error}"))?
    {
        let Ok(tpv) = serde_json::from_str::<GpsdTpv>(&line) else {
            continue;
        };
        if tpv.class != "TPV" {
            continue;
        }
        if tpv.mode.unwrap_or_default() < 2 {
            hub.publish_state(
                state,
                node,
                None,
                Some("gpsd has no position fix".to_owned()),
            );
            continue;
        }
        let (Some(latitude), Some(longitude)) = (tpv.lat, tpv.lon) else {
            continue;
        };
        let fix = PositionFix {
            latitude,
            longitude,
            altitude_m: tpv.alt,
            accuracy_m: tpv.epx.into_iter().chain(tpv.epy).reduce(f64::max),
            speed_mps: tpv.speed,
            track_deg: tpv.track,
            time: tpv.time.unwrap_or_else(now),
        };
        if validate_update(Some(&fix), None).is_err() {
            continue;
        }
        hub.publish_state(state, node, Some(fix), None);
    }
    Err("gpsd connection closed".to_owned())
}

#[derive(Deserialize)]
struct GpsdTpv {
    class: String,
    mode: Option<u8>,
    lat: Option<f64>,
    lon: Option<f64>,
    alt: Option<f64>,
    epx: Option<f64>,
    epy: Option<f64>,
    speed: Option<f64>,
    track: Option<f64>,
    time: Option<String>,
}

async fn run_nmea(
    hub: Arc<GpsHub>,
    state: AppState,
    node: String,
    device: String,
    baud: u32,
    update_interval: Duration,
) {
    retry_forever(&hub, &state, &node, || {
        nmea_session(&hub, &state, &node, &device, baud, update_interval)
    })
    .await;
}

async fn nmea_session(
    hub: &GpsHub,
    state: &AppState,
    node: &str,
    device: &str,
    baud: u32,
    update_interval: Duration,
) -> Result<(), String> {
    let device_name = device.to_owned();
    let open_device = device_name.clone();
    let serial = tokio::task::spawn_blocking(move || {
        tokio_serial::new(open_device, baud).open_native_async()
    })
    .await
    .map_err(|error| format!("NMEA {device_name}: open task failed: {error}"))?
    .map_err(|error| format!("NMEA {device_name}: {error}"))?;
    let mut reader = BufReader::new(serial);
    let mut parser = NmeaState::default();
    let mut published_at: Option<std::time::Instant> = None;
    while let Some(line) =
        tokio::time::timeout(READ_TIMEOUT, read_bounded_line(&mut reader, NMEA_MAX_LINE))
            .await
            .map_err(|_| "NMEA read timed out".to_owned())?
            .map_err(|error| format!("NMEA read: {error}"))?
    {
        if let Some(fix) = parser.parse(&line)
            && published_at.is_none_or(|last| last.elapsed() >= update_interval)
        {
            hub.publish_state(state, node, Some(fix), None);
            published_at = Some(std::time::Instant::now());
        }
    }
    Err("NMEA device closed".to_owned())
}

async fn read_bounded_line<R>(reader: &mut R, max_len: usize) -> std::io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::with_capacity(max_len.min(256));
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        if line.len().saturating_add(take) > max_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "position source line is too long",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "position source line is not UTF-8",
        )
    })
}

pub(crate) fn nmea_devices() -> Result<NmeaDevicesResponse, String> {
    let ports = tokio_serial::available_ports()
        .map_err(|error| format!("list serial devices: {error}"))?
        .into_iter()
        .filter(is_openable_port)
        .map(nmea_device_info)
        .collect();
    Ok(NmeaDevicesResponse {
        devices: offered_ports(ports),
    })
}

fn offered_ports(mut ports: Vec<NmeaDeviceInfo>) -> Vec<NmeaDeviceInfo> {
    ports.sort_by(|left, right| {
        receiver_rank(left)
            .cmp(&receiver_rank(right))
            .then_with(|| left.path.cmp(&right.path))
    });
    let receivers = ports
        .iter()
        .filter(|port| receiver_rank(port) == RECEIVER)
        .cloned()
        .collect::<Vec<_>>();
    if receivers.is_empty() {
        ports
    } else {
        receivers
    }
}

const PSEUDO_PORTS: [&str; 4] = [
    "Bluetooth-Incoming-Port",
    "debug-console",
    "wlan-debug",
    "WirelessiAP",
];

fn is_openable_port(info: &SerialPortInfo) -> bool {
    let name = info.port_name.rsplit('/').next().unwrap_or(&info.port_name);
    !name.starts_with("tty.") && !PSEUDO_PORTS.iter().any(|pseudo| name.contains(pseudo))
}

const GNSS_USB_VIDS: [u16; 2] = [0x1546, 0x091e];

const GNSS_WORDS: [&str; 10] = [
    "gps", "gnss", "glonass", "galileo", "beidou", "u-blox", "ublox", "garmin", "sirf", "navilock",
];

const RECEIVER: u8 = 0;

fn receiver_rank(device: &NmeaDeviceInfo) -> u8 {
    let described = device
        .product
        .iter()
        .chain(device.manufacturer.iter())
        .any(|text| {
            let text = text.to_lowercase();
            GNSS_WORDS.iter().any(|word| text.contains(word))
        });
    if described
        || device
            .usb_vid
            .is_some_and(|vid| GNSS_USB_VIDS.contains(&vid))
    {
        RECEIVER
    } else if device.usb_vid.is_some() {
        1
    } else {
        2
    }
}

fn nmea_device_info(info: SerialPortInfo) -> NmeaDeviceInfo {
    let mut device = NmeaDeviceInfo {
        path: info.port_name,
        product: None,
        manufacturer: None,
        serial: None,
        usb_vid: None,
        usb_pid: None,
    };
    match info.port_type {
        SerialPortType::UsbPort(usb) => {
            device.product = usb.product;
            device.manufacturer = usb.manufacturer;
            device.serial = usb.serial_number;
            device.usb_vid = Some(usb.vid);
            device.usb_pid = Some(usb.pid);
        }
        SerialPortType::BluetoothPort => device.product = Some("Bluetooth serial".to_owned()),
        SerialPortType::PciPort => device.product = Some("PCI serial".to_owned()),
        SerialPortType::Unknown => {}
    }
    device
}

#[derive(Default)]
struct NmeaState {
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude_m: Option<f64>,
    speed_mps: Option<f64>,
    track_deg: Option<f64>,
}

impl NmeaState {
    fn parse(&mut self, sentence: &str) -> Option<PositionFix> {
        let body = checked_nmea(sentence)?;
        let fields: Vec<&str> = body.split(',').collect();
        let kind = fields.first()?.get(2..)?;
        match kind {
            "GGA" => {
                if fields.get(6)?.parse::<u8>().ok()? == 0 {
                    return None;
                }
                self.latitude = nmea_coordinate(fields.get(2)?, fields.get(3)?, false);
                self.longitude = nmea_coordinate(fields.get(4)?, fields.get(5)?, true);
                self.altitude_m = fields.get(9).and_then(|value| value.parse().ok());
            }
            "RMC" => {
                if *fields.get(2)? != "A" {
                    return None;
                }
                self.latitude = nmea_coordinate(fields.get(3)?, fields.get(4)?, false);
                self.longitude = nmea_coordinate(fields.get(5)?, fields.get(6)?, true);
                self.speed_mps = fields
                    .get(7)
                    .and_then(|value| value.parse::<f64>().ok())
                    .map(|knots| knots * 0.514_444);
                self.track_deg = fields.get(8).and_then(|value| value.parse().ok());
            }
            _ => return None,
        }
        let fix = PositionFix {
            latitude: self.latitude?,
            longitude: self.longitude?,
            altitude_m: self.altitude_m,
            accuracy_m: None,
            speed_mps: self.speed_mps,
            track_deg: self.track_deg,
            time: now(),
        };
        fix.validate().ok()?;
        Some(fix)
    }
}

fn checked_nmea(sentence: &str) -> Option<&str> {
    let sentence = sentence.trim();
    let body = sentence.strip_prefix('$')?;
    let (payload, checksum) = body.rsplit_once('*')?;
    let expected = u8::from_str_radix(checksum.get(..2)?, 16).ok()?;
    let actual = payload.bytes().fold(0, |sum, byte| sum ^ byte);
    (actual == expected).then_some(payload)
}

fn nmea_coordinate(value: &str, hemisphere: &str, longitude: bool) -> Option<f64> {
    let degree_digits = if longitude { 3 } else { 2 };
    let degrees: f64 = value.get(..degree_digits)?.parse().ok()?;
    let minutes: f64 = value.get(degree_digits..)?.parse().ok()?;
    if !(0.0..60.0).contains(&minutes) {
        return None;
    }
    let sign = match hemisphere {
        "N" | "E" => 1.0,
        "S" | "W" => -1.0,
        _ => return None,
    };
    Some(sign * (degrees + minutes / 60.0))
}

fn now() -> String {
    jiff::Timestamp::now().to_string()
}

fn limit_error(error: impl Into<String>) -> String {
    error.into().chars().take(MAX_ERROR_LEN).collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_engine::Engine;
    use sdrmm_wire::{GpsNode, PatchNode, Position, WorkspaceSnapshot};
    use tokio_serial::UsbPortInfo;

    use super::*;

    #[test]
    fn parses_checked_gga_and_rmc_sentences() {
        let mut state = NmeaState::default();
        let gga = state
            .parse("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47")
            .expect("GGA fix");
        assert!((gga.latitude - 48.1173).abs() < 0.000_001);
        assert!((gga.longitude - 11.516_666_7).abs() < 0.000_001);
        assert_eq!(gga.altitude_m, Some(545.4));

        let rmc = state
            .parse("$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A")
            .expect("RMC fix");
        assert!((rmc.speed_mps.expect("speed") - 11.523_545_6).abs() < 0.000_001);
        assert_eq!(rmc.track_deg, Some(84.4));
        assert_eq!(rmc.altitude_m, Some(545.4));
    }

    #[test]
    fn rejects_a_bad_checksum_and_invalid_coordinates() {
        let mut state = NmeaState::default();
        assert!(
            state
                .parse("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00")
                .is_none()
        );
        assert!(nmea_coordinate("1260.0", "N", false).is_none());
        assert!(
            state
                .parse("$GPGGA,123519,9100.000,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*4F")
                .is_none()
        );
    }

    #[tokio::test]
    async fn bounded_lines_reject_oversized_input_before_allocating_it() {
        let mut normal = BufReader::new(&b"$GPGGA,test*00\r\n"[..]);
        assert_eq!(
            read_bounded_line(&mut normal, 64).await.unwrap().as_deref(),
            Some("$GPGGA,test*00")
        );

        let mut oversized = BufReader::new(&b"123456789\n"[..]);
        let error = read_bounded_line(&mut oversized, 8).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn serial_discovery_preserves_usb_identity() {
        let device = nmea_device_info(SerialPortInfo {
            port_name: "/dev/ttyACM0".to_owned(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x1546,
                pid: 0x01a7,
                serial_number: Some("GPS-1".to_owned()),
                manufacturer: Some("u-blox".to_owned()),
                product: Some("GNSS receiver".to_owned()),
            }),
        });
        assert_eq!(device.path, "/dev/ttyACM0");
        assert_eq!(device.product.as_deref(), Some("GNSS receiver"));
        assert_eq!(device.serial.as_deref(), Some("GPS-1"));
        assert_eq!(
            (device.usb_vid, device.usb_pid),
            (Some(0x1546), Some(0x01a7))
        );
    }

    #[test]
    fn only_openable_ports_are_offered_and_receivers_rank_first() {
        for pseudo in [
            "/dev/cu.Bluetooth-Incoming-Port",
            "/dev/cu.debug-console",
            "/dev/tty.usbmodem11401",
        ] {
            assert!(
                !is_openable_port(&SerialPortInfo {
                    port_name: pseudo.to_owned(),
                    port_type: SerialPortType::Unknown,
                }),
                "offered {pseudo}"
            );
        }
        assert!(is_openable_port(&SerialPortInfo {
            port_name: "/dev/cu.usbmodem11401".to_owned(),
            port_type: SerialPortType::Unknown,
        }));
        assert!(is_openable_port(&SerialPortInfo {
            port_name: "/dev/ttyUSB0".to_owned(),
            port_type: SerialPortType::Unknown,
        }));

        let port = |product: Option<&str>, vid: Option<u16>| NmeaDeviceInfo {
            path: "/dev/ttyUSB0".to_owned(),
            product: product.map(ToOwned::to_owned),
            manufacturer: None,
            serial: None,
            usb_vid: vid,
            usb_pid: None,
        };
        assert_eq!(receiver_rank(&port(Some("u-blox GNSS receiver"), None)), 0);
        assert_eq!(receiver_rank(&port(None, Some(0x1546))), 0);
        assert_eq!(
            receiver_rank(&port(Some("LG Monitor Controls"), Some(1))),
            1
        );
        assert_eq!(
            receiver_rank(&port(Some("STM32 Virtual ComPort"), Some(1))),
            1
        );
        assert_eq!(
            receiver_rank(&port(Some("FT232R USB UART"), Some(0x0403))),
            1
        );
        assert_eq!(receiver_rank(&port(None, None)), 2);
    }

    #[test]
    fn the_picker_is_offered_receivers_alone_unless_there_are_none() {
        let named = |path: &str, product: Option<&str>| NmeaDeviceInfo {
            path: path.to_owned(),
            product: product.map(ToOwned::to_owned),
            manufacturer: None,
            serial: None,
            usb_vid: Some(0x1234),
            usb_pid: None,
        };
        let monitor = named("/dev/cu.usbmodem306", Some("LG Monitor Controls"));
        let board = named("/dev/cu.usbmodem9A4", Some("STM32 Virtual ComPort"));
        let puck = named("/dev/cu.usbmodem11401", Some("u-blox GNSS receiver"));

        assert_eq!(
            offered_ports(vec![monitor.clone(), board.clone(), puck.clone()])
                .iter()
                .map(|port| port.path.as_str())
                .collect::<Vec<_>>(),
            ["/dev/cu.usbmodem11401"]
        );

        assert_eq!(offered_ports(vec![monitor, board]).len(), 2);
        assert!(offered_ports(Vec::new()).is_empty());
    }

    #[test]
    fn device_fixes_are_accepted_only_for_an_active_device_gps_node() {
        let store = crate::Store::open(None).expect("store");
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(PatchNode {
            id: "position".to_owned(),
            body: NodeBody::Gps(GpsNode {
                source: PositionSource::Device,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        let workspace_id = store
            .create_workspace("mobile", &snapshot)
            .expect("workspace");
        store
            .activate_workspace(workspace_id)
            .expect("activate workspace");
        let app = crate::AppState::new(Engine::new(None), Arc::new(store));
        let mut events = app.gps.subscribe();
        app.gps.reconcile(&app);
        assert_eq!(
            events.try_recv().expect("waiting event"),
            ServerEvent::PositionChanged {
                node: "position".to_owned(),
                fix: None,
                error: Some("waiting for a position fix".to_owned()),
            }
        );
        let fix = PositionFix {
            latitude: 52.52,
            longitude: 13.405,
            altitude_m: None,
            accuracy_m: Some(4.0),
            speed_mps: None,
            track_deg: None,
            time: "2026-08-14T12:00:00Z".to_owned(),
        };
        app.gps
            .publish_device(&app, "position", Some(fix.clone()), None)
            .expect("accepted");
        assert_eq!(
            events.try_recv().expect("event"),
            ServerEvent::PositionChanged {
                node: "position".to_owned(),
                fix: Some(fix),
                error: None,
            }
        );
        assert!(
            app.gps
                .publish_device(&app, "other", None, Some("lost".to_owned()))
                .is_err()
        );
    }
}
