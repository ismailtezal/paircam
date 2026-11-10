//! Runs the engine without an interface: connects, prints what happens and saves one preview
//! picture, so the receiving pipeline can be checked from a terminal.
//!
//! ```text
//! cargo run -p paircam-engine --example headless -- 127.0.0.1:47810 --seconds 6 --save shot.bmp
//! ```

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use paircam_engine::{Engine, Event, FrameSink, Nv12Frame, PreviewFrame};

struct CountingSink {
    frames: Arc<AtomicU32>,
}

impl FrameSink for CountingSink {
    fn frame(&mut self, _: &Nv12Frame<'_>) {
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
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().expect("the option needs a value");
        match argument.as_str() {
            "--seconds" => seconds = value().parse().expect("a number of seconds"),
            "--save" => save = Some(PathBuf::from(value())),
            "--data" => data_directory = PathBuf::from(value()),
            other => address = Some(other.parse().expect("an address such as 127.0.0.1:47810")),
        }
    }
    let address = address.expect("pass the phone's address");

    let (engine, channels) = Engine::new(&data_directory, "Headless test");
    let frames = Arc::new(AtomicU32::new(0));
    engine.set_frame_sink(Box::new(CountingSink {
        frames: frames.clone(),
    }));
    engine.connect(address);

    let events = channels.events.clone();
    thread::spawn(move || {
        while let Ok(event) = events.recv_blocking() {
            match event {
                Event::Capabilities(capabilities) => println!(
                    "event: capabilities with {} cameras",
                    capabilities.cameras.len()
                ),
                other => println!("event: {other:?}"),
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last_preview: Option<PreviewFrame> = None;
    let mut previews = 0u32;
    while Instant::now() < deadline {
        match channels.preview.try_recv() {
            Ok(frame) => {
                previews += 1;
                last_preview = Some(frame);
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
