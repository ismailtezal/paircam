//! Runs the engine without an interface: connects, prints what happens and saves one preview
//! picture, so the receiving pipeline can be checked from a terminal.
//!
//! ```text
//! cargo run -p paircam-engine --example headless -- 127.0.0.1:47810 --seconds 6 --save shot.bmp
//! ```
//!
//! The away screen can be tried too: `--away <seconds>` turns the automatic switch on with
//! that delay, `--away-media <file>` and `--away-message <text>` set what it shows, and
//! `--away-now` shows it from the start. With any of them, `--save` keeps the picture from
//! one second after the away screen first showed instead of the last one.
//!
//! `--cinematic` turns Cinematic mode on as soon as the phone announces it, and
//! `--cinematic-track <x,y>` then asks it to follow whatever is at that point of the picture.

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc as Shared, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use paircam_engine::protocol::{CinematicFocus, ControlValues, Point};
use paircam_engine::{AwaySettings, CameraFrame, Engine, Event, FrameSink, PreviewFrame};

struct CountingSink {
    frames: Arc<AtomicU32>,
}

impl FrameSink for CountingSink {
    fn wants_frames(&self) -> bool {
        true
    }

    fn frame(&mut self, _: &CameraFrame<'_>) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    fn stopped(&mut self) {}
}

fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let mut address: Option<SocketAddr> = None;
    let mut seconds = 5u64;
    let mut save: Option<PathBuf> = None;
    let mut data_directory = std::env::temp_dir().join("paircam-headless");
    let mut away = AwaySettings::default();
    let mut away_now = false;
    let mut cinematic = false;
    let mut cinematic_track: Option<Point> = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().expect("the option needs a value");
        match argument.as_str() {
            "--seconds" => seconds = value().parse().expect("a number of seconds"),
            "--save" => save = Some(PathBuf::from(value())),
            "--data" => data_directory = PathBuf::from(value()),
            "--away" => {
                away.automatic = true;
                away.delay = value().parse().expect("a number of seconds");
            }
            "--away-media" => away.media = Some(PathBuf::from(value())),
            "--away-message" => away.message = Some(value()),
            "--away-now" => away_now = true,
            "--cinematic" => cinematic = true,
            "--cinematic-track" => {
                let value = value();
                let (x, y) = value.split_once(',').expect("a point such as 0.5,0.5");
                cinematic_track = Some(Point {
                    x: x.parse().expect("a number"),
                    y: y.parse().expect("a number"),
                });
            }
            other => address = Some(other.parse().expect("an address such as 127.0.0.1:47810")),
        }
    }
    let address = address.expect("pass the phone's address");

    let (engine, channels) = Engine::new(&data_directory, "Headless test");
    let frames = Arc::new(AtomicU32::new(0));
    engine.set_frame_sink(Box::new(CountingSink {
        frames: frames.clone(),
    }));
    let trying_away = away != AwaySettings::default() || away_now;
    engine.set_away_fallback_message("We'll be right back");
    engine.set_away(away);
    engine.set_away_now(away_now);
    engine.connect(address);

    let started = Instant::now();
    // When the away screen first showed.
    let shown: Shared<Mutex<Option<Instant>>> = Shared::default();
    let events = channels.events.clone();
    {
        let shown = shown.clone();
        let engine = engine.clone();
        thread::spawn(move || {
            let mut tracked = false;
            while let Ok(event) = events.recv_blocking() {
                let at = started.elapsed().as_secs_f32();
                match event {
                    Event::Capabilities(capabilities) => {
                        println!(
                            "{at:6.2}s event: capabilities with {} cameras, cinematic {:?}",
                            capabilities.cameras.len(),
                            capabilities.controls.cinematic
                        );
                        if cinematic && capabilities.controls.cinematic.is_some() {
                            engine.control(ControlValues {
                                cinematic: Some(true),
                                ..Default::default()
                            });
                        }
                    }
                    Event::Cinematic(state) => {
                        if let (Some(point), false) = (cinematic_track, tracked) {
                            tracked = true;
                            engine.control(ControlValues {
                                cinematic_focus: Some(CinematicFocus::Track {
                                    point,
                                    strong: true,
                                }),
                                ..Default::default()
                            });
                        }
                        println!("{at:6.2}s event: {state:?}");
                    }
                    Event::Away(status) => {
                        if status.showing {
                            shown.lock().unwrap().get_or_insert_with(Instant::now);
                        }
                        println!("{at:6.2}s event: {status:?}");
                    }
                    other => println!("{at:6.2}s event: {other:?}"),
                }
            }
        });
    }

    let deadline = started + Duration::from_secs(seconds);
    let mut last_preview: Option<PreviewFrame> = None;
    let mut kept = false;
    let mut previews = 0u32;
    while Instant::now() < deadline {
        match channels.preview.try_recv() {
            Ok(frame) => {
                previews += 1;
                if !kept {
                    last_preview = Some(frame);
                    kept = trying_away
                        && shown
                            .lock()
                            .unwrap()
                            .is_some_and(|at| at.elapsed() >= Duration::from_secs(1));
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
    engine.disconnect();

    println!(
        "result: {} pictures decoded, {previews} preview pictures",
        frames.load(Ordering::Relaxed)
    );
    if let (Some(path), Some(frame)) = (save, last_preview) {
        fs::write(&path, bmp(&frame)).expect("cannot save the picture");
        println!(
            "result: saved {}x{} to {}",
            frame.width,
            frame.height,
            path.display()
        );
    }
}

/// Encodes a BGRA picture as an uncompressed 32-bit BMP.
fn bmp(frame: &PreviewFrame) -> Vec<u8> {
    let pixels = frame.bgra.len() as u32;
    let mut file = Vec::with_capacity(54 + frame.bgra.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(54 + pixels).to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(&40u32.to_le_bytes());
    file.extend_from_slice(&(frame.width as i32).to_le_bytes());
    // A negative height stores the rows top to bottom.
    file.extend_from_slice(&(-(frame.height as i32)).to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&32u16.to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&pixels.to_le_bytes());
    file.extend_from_slice(&[0; 16]);
    file.extend_from_slice(&frame.bgra);
    file
}
