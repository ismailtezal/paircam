//! Gives `paircam.exe` its Windows icon and version information (shown in Explorer, Task
//! Manager, the taskbar and "Installed apps").
//!
//! The icon is drawn by `installer/assets/make-icon.ps1`. The version comes from the workspace
//! Cargo.toml; the copyright line from the repository's LICENSE.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // Not canonicalize(): its \\?\ prefix confuses the resource compiler.
    let repository = manifest_dir.ancestors().nth(2).unwrap().to_path_buf();
    let icon = repository.join("installer/assets/paircam.ico");
    let license = repository.join("LICENSE");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", license.display());

    let copyright = fs::read_to_string(&license)
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("Copyright"))
                .map(str::to_owned)
        })
        .unwrap_or_default();

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    // The numeric version must be four 16-bit numbers; pre-release suffixes only appear in the
    // string version.
    let numeric = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap())
        .join(",");

    let strings = [
        ("CompanyName", "PairCam"),
        ("FileDescription", "PairCam"),
        ("FileVersion", version.as_str()),
        ("InternalName", "paircam"),
        ("LegalCopyright", copyright.as_str()),
        ("OriginalFilename", "paircam.exe"),
        ("ProductName", "PairCam"),
        ("ProductVersion", version.as_str()),
    ];

    // The resource script is generated so that the version and the icon path come from one
    // place. Resource compiler strings use C escapes (so a backslash is doubled) and escape a
    // quote by doubling it.
    let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\"\""));
    let mut script = String::new();
    writeln!(script, "#pragma code_page(65001)").unwrap();
    // Icon ID 1 is the one Explorer shows for the file.
    writeln!(script, "1 ICON {}", quote(&icon.display().to_string())).unwrap();
    writeln!(script, "1 VERSIONINFO").unwrap();
    writeln!(script, "FILEVERSION {numeric},0").unwrap();
    writeln!(script, "PRODUCTVERSION {numeric},0").unwrap();
    writeln!(script, "FILEOS 0x40004").unwrap(); // VOS_NT_WINDOWS32
    writeln!(script, "FILETYPE 0x1").unwrap(); // VFT_APP
    writeln!(script, "BEGIN").unwrap();
    writeln!(script, "  BLOCK \"StringFileInfo\"").unwrap();
    writeln!(script, "  BEGIN").unwrap();
    // US English, Unicode.
    writeln!(script, "    BLOCK \"040904B0\"").unwrap();
    writeln!(script, "    BEGIN").unwrap();
    for (name, value) in strings {
        writeln!(script, "      VALUE \"{name}\", {}", quote(value)).unwrap();
    }
    writeln!(script, "    END").unwrap();
    writeln!(script, "  END").unwrap();
    writeln!(script, "  BLOCK \"VarFileInfo\"").unwrap();
    writeln!(script, "  BEGIN").unwrap();
    writeln!(script, "    VALUE \"Translation\", 0x0409, 1200").unwrap();
    writeln!(script, "  END").unwrap();
    writeln!(script, "END").unwrap();

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let resource = out_dir.join("paircam.rc");
    fs::write(&resource, script).unwrap();
    embed_resource::compile(&resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
