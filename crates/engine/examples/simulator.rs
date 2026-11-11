//! A fake phone for developing and testing the desktop without an iPhone.
//!
//! It speaks the camera side of the protocol and streams a moving test pattern that FFmpeg
//! encodes in real time:
//!
//! ```text
//! cargo run -p paircam-engine --example simulator
//! ```
//!
//! The desktop app finds it on the network; the `headless` example connects to
//! `127.0.0.1:47810`.
//!
//! It also plays a person for face tracking and hand gestures. An orange square stands in for
//! a face: it wanders around the picture and leaves now and then. While face tracking is on,
//! the simulator reports where it is in `subject` messages, as the phone does. While gestures
//! are on, it performs one every few seconds (`--gesture-seconds`), acts on it as the phone
//! would and tells the desktop.

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
use paircam_engine::discovery::Advertisement;
use paircam_engine::protocol::{
    CameraHandshake, CameraInfo, Capabilities, Codec, Configure, ControlCapabilities,
    ControlValues, DEFAULT_PORT, DeviceId, FocusMode, Format, Gesture, GestureKind, GesturePhase,
    Hello, Lens, Message, Mode, Opener, PairResult, PairingKey, Position, Range, Rect,
    SERVICE_TYPE, Sealer, Stabilization, Stats, Status, Subject, Thermal, UNPAIRED, VideoConfig,
    VideoFrame, WhiteBalanceMode, random_bytes, read_record, write_record,
};

struct Options {
    port: u16,
    name: String,
    decline: bool,
    /// How long the simulated user takes to answer a pairing request, in milliseconds.
    pair_delay: u64,
    encoder: Option<String>,
    ffmpeg: PathBuf,
    /// Seconds between simulated hand gestures while gestures are on.
    gesture_seconds: f32,
    /// Start with face tracking on, as if the user had turned it on on the phone.
    tracking: bool,
    /// Start with hand gestures on.
    gestures: bool,
}

fn options() -> Options {
    let mut options = Options {
        port: DEFAULT_PORT,
        name: "Simulated iPhone".into(),
        decline: false,
        pair_delay: 1500,
        encoder: None,
        ffmpeg: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/ffmpeg/bin/ffmpeg.exe"),
        gesture_seconds: 6.0,
        tracking: false,
        gestures: false,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().expect("the option needs a value");
        match argument.as_str() {
            "--port" => options.port = value().parse().expect("a port number"),
            "--name" => options.name = value(),
            "--decline" => options.decline = true,
            "--pair-delay" => options.pair_delay = value().parse().expect("milliseconds"),
            "--encoder" => options.encoder = Some(value()),
            "--ffmpeg" => options.ffmpeg = PathBuf::from(value()),
            "--tracking" => options.tracking = true,
            "--gestures" => options.gestures = true,
            "--gesture-seconds" => {
                options.gesture_seconds = value().parse().expect("a number of seconds");
            }
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

    // Advertise like the phone does, so the desktop finds the simulator by itself.
    let id = camera_id.to_hex();
    let advertised = Advertisement::start(
        SERVICE_TYPE,
        &options.name,
        options.port,
        &[("v", "0"), ("id", &id), ("name", &options.name)],
    );
    if advertised.is_none() {
        println!("simulator: could not advertise on the network; connect by address instead");
    }

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
        thread::sleep(Duration::from_millis(options.pair_delay));
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
    let controls = Arc::new(Mutex::new(ControlValues {
        zoom: Some(1.0),
        focus: Some(FocusMode::Auto),
        exposure_bias: Some(0.0),
        white_balance: Some(WhiteBalanceMode::Auto),
        torch: Some(false),
        stabilization: Some(Stabilization::Standard),
        face_tracking: Some(options.tracking),
        gestures: Some(options.gestures),
        ..Default::default()
    }));
    send(
        &writer,
        &Message::ControlState(controls.lock().unwrap().clone()),
    );
    let scene: Scene = Arc::new(Mutex::new(None));
    let _person = Person::start(
        writer.clone(),
        controls.clone(),
        scene.clone(),
        options.gesture_seconds,
    );

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
                    stream_task = start(options, configuration.as_ref(), &writer, &scene);
                }
            }
            Message::StartStream => {
                drop(stream_task.take());
                stream_task = start(options, configuration.as_ref(), &writer, &scene);
            }
            Message::StopStream => stream_task = None,
            Message::Control(change) => {
                println!("simulator: control {change:?}");
                let state = {
                    let mut controls = controls.lock().unwrap();
                    merge(&mut controls, change);
                    controls.clone()
                };
                send(&writer, &Message::ControlState(state));
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
    state.face_tracking = change.face_tracking.or(state.face_tracking);
    state.gestures = change.gestures.or(state.gestures);
}

// ---------------------------------------------------------------------------------------------
// The simulated person
// ---------------------------------------------------------------------------------------------

/// When the running stream started and its picture size, so the person's position can be
/// worked out from the same clock the encoder draws them with.
type Scene = Arc<Mutex<Option<(Instant, u32, u32)>>>;

/// The face's height as a share of the picture's height, and its width as a share of its
/// height in pixels.
const FACE_HEIGHT: f32 = 0.16;
const FACE_ASPECT: f32 = 0.75;

/// The face's size in pixels for a picture, even as the encoder needs.
fn face_size(width: u32, height: u32) -> (u32, u32) {
    let face_height = ((height as f32 * FACE_HEIGHT / 2.0).round() as u32 * 2).max(2);
    let face_width = ((face_height as f32 * FACE_ASPECT / 2.0).round() as u32 * 2).clamp(2, width);
    (face_width, face_height)
}

// The person's path. `path_x` and `path_y` give the face's centre as a share of the picture,
// `present` whether they are in view; `face_filter` draws the same thing in FFmpeg's terms.
fn path_x(t: f32) -> f32 {
    0.5 + 0.28 * (0.45 * t).sin()
}

fn path_y(t: f32) -> f32 {
    0.42 + 0.12 * (0.31 * t + 1.0).sin()
}

fn present(t: f32) -> bool {
    // In view for the first twelve seconds, then away for about five in every thirty.
    (0.21 * t - 1.5).sin() < 0.85
}

/// A lavfi graph that draws the person over the test pattern.
fn face_filter(width: u32, height: u32, fps: u32) -> String {
    let (face_width, face_height) = face_size(width, height);
    format!(
        "testsrc2=size={width}x{height}:rate={fps}[pattern];\
         color=c=0xF5A623:size={face_width}x{face_height}:rate={fps}[face];\
         [pattern][face]overlay=\
         x='(0.5+0.28*sin(0.45*t))*main_w-overlay_w/2':\
         y='(0.42+0.12*sin(0.31*t+1))*main_h-overlay_h/2':\
         enable='lt(sin(0.21*t-1.5),0.85)':shortest=1[out0]"
    )
}

/// Where the face is at `t` seconds into the stream, with a little detector noise.
fn face_at(t: f32, width: u32, height: u32) -> Option<Rect> {
    if !present(t) {
        return None;
    }
    let (face_width, face_height) = face_size(width, height);
    let (face_width, face_height) = (
        face_width as f32 / width as f32,
        face_height as f32 / height as f32,
    );
    let noise = 0.004 * (7.3 * t).sin();
    Some(Rect {
        x: path_x(t) - face_width / 2.0 + noise,
        y: path_y(t) - face_height / 2.0 - noise,
        width: face_width,
        height: face_height,
    })
}

/// Reports the face and performs gestures, on a thread of its own, until dropped.
struct Person {
    running: Arc<AtomicBool>,
}

impl Person {
    fn start(
        writer: Writer,
        controls: Arc<Mutex<ControlValues>>,
        scene: Scene,
        gesture_seconds: f32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        {
            let running = running.clone();
            thread::spawn(move || {
                let mut next_gesture: Option<Instant> = None;
                let mut gestures = [GestureKind::Palm, GestureKind::Frame, GestureKind::Spread]
                    .into_iter()
                    .cycle();
                let mut spreading_until: Option<Instant> = None;
                while running.load(Ordering::SeqCst) {
                    // Up to 15 reports a second, as the phone sends.
                    thread::sleep(Duration::from_millis(66));
                    let state = controls.lock().unwrap().clone();
                    let streaming = *scene.lock().unwrap();

                    if state.face_tracking == Some(true)
                        && let Some((started, width, height)) = streaming
                    {
                        let t = started.elapsed().as_secs_f32();
                        let face = face_at(t, width, height);
                        if !send(&writer, &Message::Subject(Subject { face })) {
                            break;
                        }
                    }

                    if state.gestures != Some(true) {
                        next_gesture = None;
                        continue;
                    }
                    let now = Instant::now();
                    if let Some(until) = spreading_until
                        && now >= until
                    {
                        spreading_until = None;
                        let zoom = (state.zoom.unwrap_or(1.0) * 1.5).min(4.0);
                        controls.lock().unwrap().zoom = Some(zoom);
                        println!("simulator: hands stopped spreading; zoom {zoom:.1}");
                        perform(&writer, GestureKind::Spread, GesturePhase::Ended, &controls);
                    }
                    let due = *next_gesture
                        .get_or_insert(now + Duration::from_secs_f32(gesture_seconds.max(0.5)));
                    if now < due || spreading_until.is_some() {
                        continue;
                    }
                    next_gesture = Some(now + Duration::from_secs_f32(gesture_seconds.max(0.5)));
                    let kind = gestures.next().unwrap_or(GestureKind::Palm);
                    match kind {
                        GestureKind::Palm => {
                            let on = state.face_tracking != Some(true);
                            controls.lock().unwrap().face_tracking = Some(on);
                            println!("simulator: open palm; face tracking {on}");
                            perform(&writer, kind, GesturePhase::Recognised, &controls);
                        }
                        GestureKind::Frame => {
                            let zoom = if state.zoom.unwrap_or(1.0) > 1.5 {
                                1.0
                            } else {
                                2.0
                            };
                            controls.lock().unwrap().zoom = Some(zoom);
                            println!("simulator: L-shaped frame; zoom {zoom:.1}");
                            perform(&writer, kind, GesturePhase::Recognised, &controls);
                        }
                        _ => {
                            println!("simulator: hands spreading");
                            spreading_until = Some(now + Duration::from_millis(1200));
                            let _ = send(
                                &writer,
                                &Message::Gesture(Gesture {
                                    kind,
                                    phase: GesturePhase::Began,
                                }),
                            );
                        }
                    }
                }
            });
        }
        Self { running }
    }
}

impl Drop for Person {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Tells the desktop about a gesture the simulated phone acted on, then reports the controls
/// it changed, as the phone does.
fn perform(
    writer: &Writer,
    kind: GestureKind,
    phase: GesturePhase,
    controls: &Mutex<ControlValues>,
) {
    let _ = send(writer, &Message::Gesture(Gesture { kind, phase }));
    let state = controls.lock().unwrap().clone();
    let _ = send(writer, &Message::ControlState(state));
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
            face_tracking: true,
            gestures: vec![GestureKind::Palm, GestureKind::Frame, GestureKind::Spread],
        },
    }
}

/// A running encoder and the threads forwarding its output. Dropping it stops the stream.
struct StreamTask {
    child: Child,
    running: Arc<AtomicBool>,
    scene: Scene,
}

impl Drop for StreamTask {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        *self.scene.lock().unwrap() = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(
    options: &Options,
    configuration: Option<&Configure>,
    writer: &Writer,
    scene: &Scene,
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
        .arg(face_filter(
            configure.width,
            configure.height,
            configure.fps,
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
        let (writer, running, frames, bytes, scene) = (
            writer.clone(),
            running.clone(),
            frames.clone(),
            bytes.clone(),
            scene.clone(),
        );
        let configure = configure.clone();
        thread::spawn(move || {
            forward(
                output, &configure, &writer, &running, &frames, &bytes, &scene,
            );
        });
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
    Some(StreamTask {
        child,
        running,
        scene: scene.clone(),
    })
}

/// Reads the encoder's Annex B byte stream, regroups it into access units and sends them.
fn forward(
    mut output: impl Read,
    configure: &Configure,
    writer: &Writer,
    running: &AtomicBool,
    frames: &AtomicU32,
    bytes: &AtomicU32,
    scene: &Scene,
) {
    let hevc = configure.codec == Codec::Hevc;
    let started = Instant::now();
    let mut splitter = NalSplitter::default();
    let mut parameter_sets = ParameterSets::default();
    let mut config_sent = false;
    let mut frames_sent = 0u64;
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
                    if frames_sent == 0 {
                        // The encoder draws the person on its own clock, which started one
                        // frame before this first picture was complete.
                        let frame = Duration::from_secs_f32(1.0 / configure.fps.max(1) as f32);
                        let started = Instant::now().checked_sub(frame).unwrap_or(started);
                        *scene.lock().unwrap() = Some((started, configure.width, configure.height));
                    }
                    frames_sent += 1;
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
