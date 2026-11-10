//! Compiles the C layer over FFmpeg and makes the FFmpeg DLLs available next to the binaries.
//!
//! FFmpeg comes from `scripts/fetch-ffmpeg.ps1`, which unpacks a pinned build into
//! `third_party/ffmpeg`. Set `PAIRCAM_FFMPEG_DIR` to use a different one.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const RUNTIME_DLLS: [&str; 4] = [
    "avcodec-62.dll",
    "avutil-60.dll",
    "swresample-6.dll",
    "swscale-9.dll",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ffmpeg = env::var_os("PAIRCAM_FFMPEG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../third_party/ffmpeg"));

    println!("cargo:rerun-if-env-changed=PAIRCAM_FFMPEG_DIR");
    println!("cargo:rerun-if-changed=native/decoder.c");
    println!(
        "cargo:rerun-if-changed={}",
        ffmpeg.join(".version").display()
    );

    if !ffmpeg.join("include/libavcodec/avcodec.h").exists() {
        panic!(
            "FFmpeg was not found in {}. Run scripts/fetch-ffmpeg.ps1 first.",
            ffmpeg.display()
        );
    }

    cc::Build::new()
        .file("native/decoder.c")
        .include(ffmpeg.join("include"))
        .warnings(true)
        .compile("paircam_decoder");

    println!(
        "cargo:rustc-link-search=native={}",
        ffmpeg.join("lib").display()
    );
    for library in ["avcodec", "avutil", "swscale"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }

    copy_runtime_dlls(&ffmpeg.join("bin"));
}

/// Copies the FFmpeg DLLs beside the built executables, tests and examples so they start
/// without changing `PATH`.
fn copy_runtime_dlls(bin: &Path) {
    // OUT_DIR is <target>/<profile>/build/<crate>-<hash>/out.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        return;
    };

    for directory in [
        profile_dir.to_path_buf(),
        profile_dir.join("deps"),
        profile_dir.join("examples"),
    ] {
        if fs::create_dir_all(&directory).is_err() {
            continue;
        }
        for dll in RUNTIME_DLLS {
            let target = directory.join(dll);
            if !target.exists() {
                let _ = fs::copy(bin.join(dll), target);
            }
        }
    }
}
