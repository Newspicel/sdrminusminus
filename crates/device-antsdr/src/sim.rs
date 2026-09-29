use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use sdrmm_device::lock;

use crate::{
    ad9361::testing::FakeChip,
    chdr::{self, Header, Kind},
    control::Target,
    link::{MTU, Ports},
    regs::{self, READBACK, core, radio},
    rx::PACKET_SAMPLES,
};

const SAMPLES_PER_SECOND: f64 = 400e3;
const STOP: u32 = 1 << 28;
const NOW: u32 = 1 << 31;
const TONE: f64 = 16384.0;

struct Registers {
    chip: FakeChip,
    values: HashMap<(usize, u32), u32>,
    spi_readback: u32,
    streaming: [bool; 2],
    sent: [u64; 2],
    peer: Option<SocketAddr>,
    epoch: Instant,
}

impl Default for Registers {
    fn default() -> Self {
        Self {
            chip: FakeChip::default(),
            values: HashMap::new(),
            spi_readback: 0,
            streaming: [false; 2],
            sent: [0; 2],
            peer: None,
            epoch: Instant::now(),
        }
    }
}

impl Registers {
    fn now(&self) -> u64 {
        (self.epoch.elapsed().as_secs_f64() * SAMPLES_PER_SECOND) as u64
    }

    fn value(&self, target: Target, register: u32) -> u32 {
        self.values
            .get(&(slot(target), register))
            .copied()
            .unwrap_or(0)
    }

    fn decimation(&self, lane: usize) -> u64 {
        let word = self.value(Target::Radio(lane), radio::DDC_DECIMATION);
        let halfbands = u64::from(word >> 9 & 1) + u64::from(word >> 8 & 1);
        (u64::from(word & 0xff).max(1)) << halfbands
    }
}

const fn slot(target: Target) -> usize {
    match target {
        Target::Local => 0,
        Target::Radio(lane) => lane + 1,
    }
}

pub(crate) struct Sim {
    pub(crate) ports: Ports,
    registers: Arc<Mutex<Registers>>,
    pub(crate) drop_next: Arc<AtomicUsize>,
    pub(crate) overflow: Arc<AtomicBool>,
    pub(crate) transmitted: Arc<Mutex<[usize; 2]>>,
    pub(crate) bursts_ended: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Sim {
    pub(crate) fn start() -> Self {
        let (ports, sockets) = bind();
        let [find, control, tx0, tx1, rx] = sockets;
        let registers = Arc::new(Mutex::new(Registers::default()));
        let running = Arc::new(AtomicBool::new(true));
        let drop_next = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicBool::new(false));
        let transmitted = Arc::new(Mutex::new([0usize; 2]));
        let bursts_ended = Arc::new(AtomicUsize::new(0));
        let mut threads = vec![
            spawn_find(find, running.clone()),
            spawn_control(control, registers.clone(), running.clone()),
            spawn_rx(
                rx,
                registers.clone(),
                running.clone(),
                drop_next.clone(),
                overflow.clone(),
            ),
        ];
        for (lane, socket) in [tx0, tx1].into_iter().enumerate() {
            threads.push(spawn_tx(
                socket,
                lane,
                running.clone(),
                transmitted.clone(),
                bursts_ended.clone(),
            ));
        }
        Self {
            ports,
            registers,
            drop_next,
            overflow,
            transmitted,
            bursts_ended,
            running,
            threads,
        }
    }

    pub(crate) fn key(&self) -> String {
        format!("127.0.0.1:{}", self.ports.control)
    }

    pub(crate) fn chip<T>(&self, look: impl FnOnce(&FakeChip) -> T) -> T {
        look(&lock(&self.registers).chip)
    }

    pub(crate) fn streaming(&self) -> [bool; 2] {
        lock(&self.registers).streaming
    }

    pub(crate) fn register(&self, target: Target, register: u32) -> u32 {
        lock(&self.registers).value(target, register)
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn bind() -> (Ports, [UdpSocket; 5]) {
    for attempt in 0..200u16 {
        let control = 20_000 + (std::process::id() as u16 % 97) * 211 + attempt * 13;
        let ports = Ports { control };
        let wanted = [ports.find(), control, ports.tx(0), ports.tx(1), ports.rx()];
        let sockets: Vec<UdpSocket> = wanted
            .iter()
            .filter_map(|port| UdpSocket::bind(("127.0.0.1", *port)).ok())
            .collect();
        if let Ok(sockets) = <[UdpSocket; 5]>::try_from(sockets) {
            for socket in &sockets {
                let _ = socket.set_read_timeout(Some(Duration::from_millis(5)));
            }
            return (ports, sockets);
        }
    }
    panic!("no free ports for the simulated radio");
}

fn spawn_find(socket: UdpSocket, running: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 128];
        while running.load(Ordering::Acquire) {
            let Ok((_, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let mut answer = [0u8; 56];
            for (slot, byte) in b"1M0c".iter().copied().enumerate() {
                answer[slot * 4 + 3] = byte;
            }
            answer[16..22].copy_from_slice(b"sim310");
            answer[48..56].copy_from_slice(b"E310  v2");
            let _ = socket.send_to(&answer, from);
        }
    })
}

fn spawn_control(
    socket: UdpSocket,
    registers: Arc<Mutex<Registers>>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; MTU];
        while running.load(Ordering::Acquire) {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let Ok(packet) = chdr::read(&buf[..n]) else {
                continue;
            };
            let target = match packet.header.sid {
                0x40 => Target::Local,
                0x10 => Target::Radio(0),
                0x20 => Target::Radio(1),
                _ => continue,
            };
            let register = chdr::word(packet.payload, 0);
            let value = chdr::word(packet.payload, 1);
            let answer = handle(&mut lock(&registers), target, register, value);
            let mut out = [0u8; 16];
            let header = Header::context(chdr::flip(packet.header.sid), packet.header.seq);
            let Ok(len) = header.write(8, &mut out) else {
                continue;
            };
            chdr::put(&mut out[len..], 0, (answer >> 32) as u32);
            chdr::put(&mut out[len..], 1, answer as u32);
            let _ = socket.send_to(&out[..len + 8], from);
        }
    })
}

fn handle(registers: &mut Registers, target: Target, register: u32, value: u32) -> u64 {
    if register == READBACK {
        return readback(registers, target, value);
    }
    registers.values.insert((slot(target), register), value);
    match (target, register) {
        (Target::Local, core::SPI_DATA) => spi(registers, value >> 8),
        (Target::Radio(lane), radio::RX_COMMAND_TIME_LO) => {
            let command = registers.value(target, radio::RX_COMMAND);
            let on = command & STOP == 0;
            registers.streaming[lane] = on;
            registers.sent[lane] = if command & NOW == 0 {
                let high = u64::from(registers.value(target, radio::RX_COMMAND_TIME_HI));
                (high << 32 | u64::from(value)) / registers.decimation(lane)
            } else {
                registers.now()
            };
        }
        _ => {}
    }
    0
}

fn spi(registers: &mut Registers, word: u32) {
    let select = registers.value(Target::Local, core::SPI_CONTROL) & 0xffffff;
    if select != 1 {
        return;
    }
    let address = ((word >> 8) & 0x3fff) as u16;
    if word & 0x80_0000 != 0 {
        registers.chip.apply(address, word as u8);
    } else {
        registers.spi_readback = u32::from(registers.chip.value(address));
    }
}

fn readback(registers: &Registers, target: Target, word: u32) -> u64 {
    match (target, word) {
        (Target::Local, 0) => u64::from(regs::SIGNATURE) << 32 | u64::from(regs::FPGA_COMPAT) << 16,
        (Target::Local, 1) => u64::from(registers.spi_readback),
        (Target::Local, 2) => (2u64 << 8 | 0x83) << 32,
        (Target::Radio(_), 0) => u64::from(registers.value(target, radio::TEST)),
        (Target::Radio(lane), 1) => registers.now() * registers.decimation(lane),
        (Target::Radio(_), 3) => {
            let idle = u64::from(registers.value(target, radio::CODEC_IDLE));
            idle << 32 | idle
        }
        _ => 0,
    }
}

fn spawn_rx(
    socket: UdpSocket,
    registers: Arc<Mutex<Registers>>,
    running: Arc<AtomicBool>,
    drop_next: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let mut seq = [0u16; 2];
        while running.load(Ordering::Acquire) {
            if let Ok((8, from)) = socket.recv_from(&mut buf) {
                lock(&registers).peer = Some(from);
            }
            let mut state = lock(&registers);
            let Some(peer) = state.peer else {
                continue;
            };
            if overflow.swap(false, Ordering::AcqRel) {
                for (lane, seq) in seq.iter().enumerate() {
                    if state.streaming[lane] {
                        state.streaming[lane] = false;
                        let _ = socket.send_to(&overflow_packet(lane, *seq), peer);
                    }
                }
                continue;
            }
            for (lane, seq) in seq.iter_mut().enumerate() {
                if !state.streaming[lane] {
                    continue;
                }
                let due = state.now();
                let decimation = state.decimation(lane);
                while state.sent[lane] + PACKET_SAMPLES as u64 <= due {
                    let first = state.sent[lane];
                    state.sent[lane] += PACKET_SAMPLES as u64;
                    let skip = drop_next
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                        .is_ok();
                    if !skip {
                        let packet = data_packet(lane, *seq, first, decimation);
                        let _ = socket.send_to(&packet, peer);
                    }
                    *seq = chdr::next_seq(*seq);
                }
            }
        }
    })
}

fn data_packet(lane: usize, seq: u16, first: u64, decimation: u64) -> Vec<u8> {
    let mut out = vec![0u8; MTU];
    let header = Header {
        kind: Kind::Data,
        seq,
        eob: false,
        sid: crate::radio::RX_STREAM_IDS[lane],
        time: Some(first * decimation),
    };
    let Ok(start) = header.write(PACKET_SAMPLES * 4, &mut out) else {
        return out;
    };
    for slot in 0..PACKET_SAMPLES {
        let phase = (first + slot as u64) as f64 * 0.1 + lane as f64;
        let i = (TONE * phase.cos()) as i16 as u16;
        let q = (TONE * phase.sin()) as i16 as u16;
        chdr::put(&mut out[start..], slot, u32::from(i) << 16 | u32::from(q));
    }
    out.truncate(start + PACKET_SAMPLES * 4);
    out
}

fn overflow_packet(lane: usize, seq: u16) -> Vec<u8> {
    let mut out = vec![0u8; 24];
    let header = Header {
        kind: Kind::Context,
        seq,
        eob: false,
        sid: crate::radio::RX_STREAM_IDS[lane],
        time: None,
    };
    let Ok(start) = header.write(8, &mut out) else {
        return out;
    };
    chdr::put(&mut out[start..], 0, 0x08);
    out.truncate(start + 8);
    out
}

fn spawn_tx(
    socket: UdpSocket,
    lane: usize,
    running: Arc<AtomicBool>,
    transmitted: Arc<Mutex<[usize; 2]>>,
    bursts_ended: Arc<AtomicUsize>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; MTU];
        let mut packets = 0usize;
        while running.load(Ordering::Acquire) {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let Ok(packet) = chdr::read(&buf[..n]) else {
                continue;
            };
            lock(&transmitted)[lane] += packet.payload.len() / 4;
            if packet.header.eob {
                bursts_ended.fetch_add(1, Ordering::AcqRel);
            }
            packets += 1;
            if packets.is_multiple_of(30) || packet.header.eob {
                let mut out = [0u8; 16];
                let header = Header::context(chdr::flip(packet.header.sid), 0);
                let Ok(len) = header.write(8, &mut out) else {
                    continue;
                };
                chdr::put(&mut out[len..], 0, 0);
                chdr::put(&mut out[len..], 1, u32::from(packet.header.seq));
                let _ = socket.send_to(&out[..len + 8], from);
            }
        }
    })
}
