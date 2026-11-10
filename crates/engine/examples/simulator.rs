//! A fake phone for developing and testing the desktop without an iPhone.
//!
//! It speaks the camera side of the protocol and streams a moving test pattern that FFmpeg
//! encodes in real time:
//!
//! ```text
//! cargo run -p paircam-engine --example simulator -- --codec hevc
//! ```
//!
//! Then connect the desktop app (or the `headless` example) to `127.0.0.1:47810`.

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use paircam_engine::protocol::{
    CameraHandshake, CameraInfo, Capabilities, Codec, Configure, ControlCapabilities,
    ControlValues, DEFAULT_PORT, DeviceId, FocusMode, Format, Hello, Lens, Message, Mode, Opener,
    PairResult, PairingKey, Position, Range, Sealer, Stabilization, Stats, Status, Thermal,
    UNPAIRED, VideoConfig, VideoFrame, WhiteBalanceMode, random_bytes, read_record, write_record,
};

struct Options {
    port: u16,
    name: String,
    decline: bool,
    encoder: Option<String>,
    ffmpeg: PathBuf,
}

fn options() -> Options {
    let mut options = Options {
        port: DEFAULT_PORT,
        name: "Simulated iPhone".into(),
        decline: false,
        encoder: None,
        ffmpeg: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/ffmpeg/bin/ffmpeg.exe"),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().expect("the option needs a value");
        match argument.as_str() {
            "--port" => options.port = value().parse().expect("a port number"),
            "--name" => options.name = value(),
            "--decline" => options.decline = true,
            "--encoder" => options.encoder = Some(value()),
            "--ffmpeg" => options.ffmpeg = PathBuf::from(value()),
            other => panic!("unknown option {other}"),
        }
    }
    options
}

type Writer = Arc<Mutex<(TcpStream, Sealer)>>;

fn send(writer: &Writer, message: &Message) -> bool {
    let mut writer = writer.lock().unwrap();
    let mut body = message.encode();
    writer.1.seal(&mut body).is_ok() && write_record(&mut writer.0, &body).is_ok()
}

fn main() {
    let options = options();
    let camera_id = DeviceId::random();
    let mut pairings: HashMap<DeviceId, PairingKey> = HashMap::new();

    let listener = TcpListener::bind(("0.0.0.0", options.port)).expect("cannot listen");
    println!(
        "simulator: \"{}\" listening on port {}",
        options.name, options.port
    );

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let peer = stream.peer_addr().ok();
        println!("simulator: connection from {peer:?}");
        match serve(stream, &options, camera_id, &mut pairings) {
            Ok(()) => println!("simulator: session ended"),
            Err(error) => println!("simulator: session failed: {error}"),
        }
    }
}

fn serve(
    stream: TcpStream,
    options: &Options,
    camera_id: DeviceId,
    pairings: &mut HashMap<DeviceId, PairingKey>,
) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut raw = stream.try_clone()?;

    let host = CameraHandshake::read_host_hello(&read_record(&mut reader)?)?;
    let pairing_key = match host.mode {
        Mode::Pair => UNPAIRED,
        Mode::Resume => match pairings.get(&host.host_id) {
            Some(key) => *key,
            None => {
                println!("simulator: unknown host tried to resume; asking it to pair");
                write_record(
                    &mut raw,
                    &CameraHandshake::refusal(Status::NotPaired, camera_id),
                )?;
                return Ok(());
            }
        },
    };
    let handshake = CameraHandshake::accept(&host, camera_id, random_bytes());
    write_record(&mut raw, handshake.hello())?;
    let keys = handshake.finish(&read_record(&mut reader)?, &pairing_key)?;

    let mut opener = Opener::new(&keys.host_to_camera);
    let writer: Writer = Arc::new(Mutex::new((raw, Sealer::new(&keys.camera_to_host))));
    let mut read_message = || -> Result<Message, Box<dyn std::error::Error>> {
        let mut record = read_record(&mut reader)?;
        opener.open(&mut record)?;
        Ok(Message::decode(record)?)
    };

    let Message::Hello(host_hello) = read_message()? else {
        return Err("expected hello".into());
    };
    send(
        &writer,
        &Message::Hello(Hello {
            name: options.name.clone(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: "ios".into(),
            model: "Simulator".into(),
        }),
    );

    if host.mode == Mode::Pair {
        println!(
            "simulator: \"{}\" wants to pair. Code {}",
            host_hello.name,
            keys.pairing_code_digits()
        );
        thread::sleep(Duration::from_millis(1500));
        let accepted = !options.decline;
        send(&writer, &Message::PairResult(PairResult { accepted }));
        if !accepted {
            return Ok(());
        }
        pairings.insert(host.host_id, keys.pairing_key);
    } else {
        println!("simulator: \"{}\" resumed", host_hello.name);
    }

    send(&writer, &Message::Capabilities(capabilities()));
    let mut controls = ControlValues {
        zoom: Some(1.0),
        focus: Some(FocusMode::Auto),
        exposure_bias: Some(0.0),
        white_balance: Some(WhiteBalanceMode::Auto),
        torch: Some(false),
        stabilization: Some(Stabilization::Standard),
        ..Default::default()
    };
    send(&writer, &Message::ControlState(controls.clone()));

    let mut configuration: Option<Configure> = None;
    let mut stream_task: Option<StreamTask> = None;
    loop {
        let message = match read_message() {
            Ok(message) => message,
            Err(_) => break,
        };
        match message {
            Message::Configure(configure) => {
                println!("simulator: configure {configure:?}");
                let restart = stream_task.take().is_some();
                configuration = Some(configure);
                if restart {
                    stream_task = start(options, configuration.as_ref(), &writer);
                }
            }
            Message::StartStream => {
                drop(stream_task.take());
                stream_task = start(options, configuration.as_ref(), &writer);
            }
            Message::StopStream => stream_task = None,
            Message::Control(change) => {
                println!("simulator: control {change:?}");
                merge(&mut controls, change);
                send(&writer, &Message::ControlState(controls.clone()));
            }
            Message::Ping(timestamp) => {
                send(&writer, &Message::Pong(timestamp));
            }
            Message::RequestKeyframe => println!("simulator: keyframe requested"),
            Message::Bye(bye) => {
                println!("simulator: host said goodbye: {}", bye.reason);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

fn merge(state: &mut ControlValues, change: ControlValues) {
    state.zoom = change.zoom.or(state.zoom);
    state.focus = change.focus.or(state.focus);
    state.focus_position = change.focus_position.or(state.focus_position);
    state.exposure_bias = change.exposure_bias.or(state.exposure_bias);
    state.white_balance = change.white_balance.or(state.white_balance);
    state.torch = change.torch.or(state.torch);
    state.stabilization = change.stabilization.or(state.stabilization);
}

fn capabilities() -> Capabilities {
    let formats = |sizes: &[(u32, u32, u32)]| {
        sizes
            .iter()
            .map(|&(width, height, max_fps)| Format {
                width,
                height,
                max_fps,
            })
            .collect()
    };
    Capabilities {
        cameras: vec![
            CameraInfo {
                id: "back-wide".into(),
                name: "Back Camera".into(),
                position: Position::Back,
                lens: Lens::Wide,
                formats: formats(&[(1280, 720, 60), (1920, 1080, 60), (3840, 2160, 30)]),
            },
            CameraInfo {
                id: "back-ultra-wide".into(),
                name: "Back Ultra Wide Camera".into(),
                position: Position::Back,
                lens: Lens::UltraWide,
                formats: formats(&[(1280, 720, 60), (1920, 1080, 60)]),
            },
            CameraInfo {
                id: "front".into(),
                name: "Front Camera".into(),
                position: Position::Front,
                lens: Lens::Wide,
                formats: formats(&[(1280, 720, 30), (1920, 1080, 30)]),
            },
        ],
        codecs: vec![Codec::Hevc, Codec::H264],
        controls: ControlCapabilities {
            zoom: Some(Range {
                min: 1.0,
                max: 10.0,
            }),
            exposure_bias: Some(Range {
                min: -8.0,
                max: 8.0,
            }),
            focus: vec![FocusMode::Auto, FocusMode::Locked, FocusMode::Manual],
            white_balance: vec![WhiteBalanceMode::Auto, WhiteBalanceMode::Locked],
            torch: true,
            stabilization: vec![
                Stabilization::Off,
                Stabilization::Standard,
                Stabilization::Cinematic,
            ],
        },
    }
}

/// A running encoder and the threads forwarding its output. Dropping it stops the stream.
struct StreamTask {
    child: Child,
    running: Arc<AtomicBool>,
}

impl Drop for StreamTask {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(
    options: &Options,
    configuration: Option<&Configure>,
    writer: &Writer,
) -> Option<StreamTask> {
    let configure = configuration?.clone();
    let hevc = configure.codec == Codec::Hevc;
    let encoder = options
        .encoder
        .clone()
        .unwrap_or_else(|| if hevc { "hevc_nvenc" } else { "h264_nvenc" }.to_owned());
    let nvenc = encoder.ends_with("_nvenc");

    let mut command = Command::new(&options.ffmpeg);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-re",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!(
            "testsrc2=size={}x{}:rate={}",
            configure.width, configure.height, configure.fps
        ))
        .args(["-pix_fmt", "yuv420p", "-c:v", &encoder])
        .args(["-b:v", &configure.bitrate.to_string()])
        .args([
            "-g",
            &((configure.fps as f32 * configure.keyframe_interval) as u32)
                .max(1)
                .to_string(),
        ])
        .args(["-bf", "0"]);
    if nvenc {
        command.args(["-preset", "p1", "-tune", "ull", "-aud", "1"]);
    }
    command
        .args(["-f", if hevc { "hevc" } else { "h264" }, "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            println!(
                "simulator: cannot start {}: {error}",
                options.ffmpeg.display()
            );
            return None;
        }
    };
    let output = child.stdout.take()?;
    let running = Arc::new(AtomicBool::new(true));
    let frames = Arc::new(AtomicU32::new(0));
    let bytes = Arc::new(AtomicU32::new(0));

    {
        let (writer, running, frames, bytes) = (
            writer.clone(),
            running.clone(),
            frames.clone(),
            bytes.clone(),
        );
        let configure = configure.clone();
        thread::spawn(move || forward(output, &configure, &writer, &running, &frames, &bytes));
    }
    {
        let (writer, running) = (writer.clone(), running.clone());
        thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_secs(1));
                let stats = Stats {
                    fps: frames.swap(0, Ordering::Relaxed) as f32,
                    bitrate: bytes.swap(0, Ordering::Relaxed) * 8,
                    dropped_frames: 0,
                    thermal: Thermal::Nominal,
                    battery: 0.82,
                    charging: true,
                };
                if !send(&writer, &Message::Stats(stats)) {
                    break;
                }
            }
        });
    }
    println!(
        "simulator: streaming {}x{} at {} fps with {encoder}",
        configure.width, configure.height, configure.fps
    );
    Some(StreamTask { child, running })
}

/// Reads the encoder's Annex B byte stream, regroups it into access units and sends them.
fn forward(
    mut output: impl Read,
    configure: &Configure,
    writer: &Writer,
    running: &AtomicBool,
    frames: &AtomicU32,
    bytes: &AtomicU32,
) {
    let hevc = configure.codec == Codec::Hevc;
    let started = Instant::now();
    let mut splitter = NalSplitter::default();
    let mut parameter_sets = ParameterSets::default();
    let mut config_sent = false;
    let mut unit = AccessUnit::default();
    let mut chunk = vec![0u8; 64 * 1024];

    while running.load(Ordering::SeqCst) {
        let read = match output.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        splitter.push(&chunk[..read]);

        while let Some(nal) = splitter.next() {
            let kind = NalKind::of(&nal, hevc);
            if kind.starts_access_unit() && unit.has_picture {
                let finished = std::mem::take(&mut unit);
                if !config_sent && let Some(record) = parameter_sets.record(hevc) {
                    config_sent = send(
                        writer,
                        &Message::VideoConfig(VideoConfig {
                            codec: configure.codec,
                            width: configure.width as u16,
                            height: configure.height as u16,
                            rotation: 0,
                            mirrored: false,
                            record: Bytes::from(record),
                        }),
                    );
                }
                // Pictures that arrive before the parameter sets cannot be decoded; skip them.
                if config_sent {
                    frames.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(finished.data.len() as u32, Ordering::Relaxed);
                    let sent = send(
                        writer,
                        &Message::VideoFrame(VideoFrame {
                            timestamp_us: started.elapsed().as_micros() as u64,
                            keyframe: finished.keyframe,
                            data: Bytes::from(finished.data),
                        }),
                    );
                    if !sent {
                        return;
                    }
                }
            }

            match kind {
                NalKind::Delimiter => {}
                NalKind::ParameterSet => parameter_sets.remember(&nal, hevc),
                NalKind::Picture { keyframe, .. } => {
                    unit.keyframe |= keyframe;
                    unit.has_picture = true;
                    unit.push(&nal);
                }
                NalKind::PrefixSei | NalKind::Other => unit.push(&nal),
            }
        }
    }
}

#[derive(Default)]
struct AccessUnit {
    data: Vec<u8>,
    keyframe: bool,
    /// Whether a coded picture has been added, which makes the unit complete.
    has_picture: bool,
}

impl AccessUnit {
    fn push(&mut self, nal: &[u8]) {
        self.data
            .extend_from_slice(&(nal.len() as u32).to_be_bytes());
        self.data.extend_from_slice(nal);
    }
}

enum NalKind {
    Delimiter,
    ParameterSet,
    Picture {
        keyframe: bool,
        first_slice: bool,
    },
    /// Supplemental information that precedes the picture it belongs to.
    PrefixSei,
    Other,
}

impl NalKind {
    fn of(nal: &[u8], hevc: bool) -> Self {
        if hevc {
            let kind = (nal[0] >> 1) & 0x3F;
            match kind {
                35 => Self::Delimiter,
                32..=34 => Self::ParameterSet,
                39 => Self::PrefixSei,
                0..=31 => Self::Picture {
                    keyframe: (16..=23).contains(&kind),
                    // first_slice_segment_in_pic_flag is the first bit after the 2-byte header.
                    first_slice: nal.get(2).is_some_and(|byte| byte & 0x80 != 0),
                },
                _ => Self::Other,
            }
        } else {
            match nal[0] & 0x1F {
                9 => Self::Delimiter,
                7 | 8 => Self::ParameterSet,
                6 => Self::PrefixSei,
                kind @ (1 | 5) => Self::Picture {
                    keyframe: kind == 5,
                    // first_mb_in_slice == 0 is coded as a leading 1 bit.
                    first_slice: nal.get(1).is_some_and(|byte| byte & 0x80 != 0),
                },
                _ => Self::Other,
            }
        }
    }

    /// Whether this NAL unit begins a new access unit once the current one holds a picture.
    fn starts_access_unit(&self) -> bool {
        match self {
            Self::Delimiter | Self::ParameterSet => true,
            Self::Picture { first_slice, .. } => *first_slice,
            Self::PrefixSei => true,
            Self::Other => false,
        }
    }
}

/// Splits an Annex B byte stream into NAL units as the bytes arrive.
#[derive(Default)]
struct NalSplitter {
    buffer: Vec<u8>,
    /// Where the NAL unit being collected starts, once a start code has been seen.
    start: Option<usize>,
    scanned: usize,
}

impl NalSplitter {
    fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            let position = find_start_code(&self.buffer, self.scanned)?;
            let after = position + 3;
            match self.start {
                None => {
                    self.start = Some(after);
                    self.scanned = after;
                }
                Some(start) => {
                    // A four-byte start code has a zero before the three-byte one.
                    let mut end = position;
                    while end > start && self.buffer[end - 1] == 0 {
                        end -= 1;
                    }
                    let nal = self.buffer[start..end].to_vec();
                    self.buffer.drain(..after);
                    self.start = Some(0);
                    self.scanned = 0;
                    if !nal.is_empty() {
                        return Some(nal);
                    }
                }
            }
        }
    }
}

fn find_start_code(buffer: &[u8], from: usize) -> Option<usize> {
    buffer
        .get(from..)?
        .windows(3)
        .position(|window| window == [0, 0, 1])
        .map(|position| position + from)
}

/// The parameter sets seen so far, from which the decoder configuration record is built.
#[derive(Default)]
struct ParameterSets {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl ParameterSets {
    fn remember(&mut self, nal: &[u8], hevc: bool) {
        let slot = if hevc {
            match (nal[0] >> 1) & 0x3F {
                32 => &mut self.vps,
                33 => &mut self.sps,
                _ => &mut self.pps,
            }
        } else if nal[0] & 0x1F == 7 {
            &mut self.sps
        } else {
            &mut self.pps
        };
        *slot = Some(nal.to_vec());
    }

    fn record(&self, hevc: bool) -> Option<Vec<u8>> {
        let (sps, pps) = (self.sps.as_ref()?, self.pps.as_ref()?);
        if hevc {
            Some(hvcc(self.vps.as_ref()?, sps, pps))
        } else {
            Some(avcc(sps, pps))
        }
    }
}

/// Builds an `avcC` record (ISO/IEC 14496-15) with 4-byte NAL lengths.
fn avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut record = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    record.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    record.extend_from_slice(sps);
    record.push(1);
    record.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    record.extend_from_slice(pps);
    record
}

/// Builds an `hvcC` record (ISO/IEC 14496-15) with 4-byte NAL lengths.
fn hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    // The profile, tier and level sit at fixed positions at the start of the SPS, after the
    // 2-byte NAL header and one byte of ids, once emulation prevention bytes are removed.
    let plain = remove_emulation_prevention(sps);
    let mut record = vec![1];
    record.extend_from_slice(&plain[3..15]);
    record.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 3]);
    for (kind, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        record.push(0x80 | kind);
        record.extend_from_slice(&1u16.to_be_bytes());
        record.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        record.extend_from_slice(nal);
    }
    record
}

fn remove_emulation_prevention(nal: &[u8]) -> Vec<u8> {
    let mut plain = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &byte in nal {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        plain.push(byte);
    }
    plain
}
