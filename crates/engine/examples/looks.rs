//! Contact sheets for judging the looks on real pictures rather than test patterns.
//!
//! Every look is applied to each still image through the same C colour stage the picture thread
//! uses (the image is converted to BT.709 limited-range NV12 first, as the phone's stream is),
//! and the results are written as one sheet per image, plus a sheet of skin-tone chips, grey
//! ramps and each look's swatch strip:
//!
//! ```text
//! cargo run --release -p paircam-engine --example looks -- --out <folder> [image ...]
//! ```
//!
//! `--time` instead times every look on each picture filled to the 1920x1080 camera frame.
//!
//! Without images, or with `--synthetic`, three procedural portraits are included: fair skin
//! by a window, medium skin in a cool office, and deep skin under a warm lamp. Use only
//! pictures you are licensed to use, and do not commit them.

use std::path::{Path, PathBuf};
use std::time::Instant;

use image::imageops::FilterType;
use image::{Rgb, RgbImage};
use paircam_engine::colour::{
    ColourSettings, ColourTables, SWATCH_COLOURS, codes_to_rgb, rgb_to_ypbpr, swatches,
};
use paircam_engine::decoder::{Nv12PlanesMut, apply_colour};
use paircam_engine::looks::LookId;
use paircam_engine::picture::PixelRect;

const TILE_WIDTH: u32 = 480;
const TILE_HEIGHT: u32 = 360;
const COLUMNS: u32 = 4;
const GAP: u32 = 8;
const LABEL: u32 = 30;
const BACKGROUND: Rgb<u8> = Rgb([24, 24, 24]);
const INK: Rgb<u8> = Rgb([230, 230, 230]);

fn main() {
    let mut out = PathBuf::from("looks");
    let mut inputs = Vec::new();
    let mut synthetic = false;
    let mut timing = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out = PathBuf::from(arguments.next().expect("--out needs a folder")),
            "--synthetic" => synthetic = true,
            "--time" => timing = true,
            _ => inputs.push(PathBuf::from(argument)),
        }
    }
    std::fs::create_dir_all(&out).expect("the output folder can be created");

    let mut pictures: Vec<(String, RgbImage)> = Vec::new();
    if inputs.is_empty() || synthetic {
        for (index, scene) in SCENES.iter().enumerate() {
            pictures.push((format!("synthetic-{}", index + 1), portrait(scene)));
        }
    }
    for path in &inputs {
        let picture = image::open(path)
            .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()))
            .to_rgb8();
        pictures.push((stem(path), picture));
    }

    let tables: Vec<(LookId, Box<ColourTables>)> = LookId::ALL
        .iter()
        .map(|look| {
            (
                *look,
                ColourTables::build(&ColourSettings {
                    look: *look,
                    ..Default::default()
                }),
            )
        })
        .collect();

    if timing {
        for (name, picture) in &pictures {
            time(name, picture, &tables);
        }
        return;
    }

    for (name, picture) in &pictures {
        let tile = fill(picture, TILE_WIDTH, TILE_HEIGHT);
        let rows = (tables.len() as u32).div_ceil(COLUMNS);
        let mut sheet = RgbImage::from_pixel(
            GAP + COLUMNS * (TILE_WIDTH + GAP),
            GAP + rows * (TILE_HEIGHT + LABEL + GAP),
            BACKGROUND,
        );
        for (index, (look, tables)) in tables.iter().enumerate() {
            let graded = grade(&tile, tables);
            let x = GAP + (index as u32 % COLUMNS) * (TILE_WIDTH + GAP);
            let y = GAP + (index as u32 / COLUMNS) * (TILE_HEIGHT + LABEL + GAP);
            image::imageops::replace(&mut sheet, &graded, i64::from(x), i64::from(y));
            text(&mut sheet, look.key(), x, y + TILE_HEIGHT + 8, 4);
        }
        let path = out.join(format!("sheet-{name}.png"));
        sheet.save(&path).expect("the sheet can be written");
        println!("{}", path.display());
    }

    let path = out.join("chips.png");
    chips(&tables)
        .save(&path)
        .expect("the chips can be written");
    println!("{}", path.display());
}

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(
        || "image".into(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// The image scaled and cropped to cover `width` x `height` (both even).
fn fill(picture: &RgbImage, width: u32, height: u32) -> RgbImage {
    image::imageops::resize(
        &image::DynamicImage::ImageRgb8(picture.clone())
            .resize_to_fill(width, height, FilterType::Lanczos3)
            .to_rgb8(),
        width,
        height,
        FilterType::Triangle,
    )
}

/// Times every look on the picture filled to the 1920x1080 camera frame, as the picture thread
/// grades it, and prints the median and best per frame.
fn time(name: &str, picture: &RgbImage, tables: &[(LookId, Box<ColourTables>)]) {
    let frame = fill(picture, 1920, 1080);
    let original = to_nv12(&frame);
    let mut nv12 = original.clone();
    let luma_size = 1920 * 1080;
    let area = PixelRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    for (look, tables) in &tables[1..] {
        let mut frames: Vec<f64> = (0..60)
            .map(|_| {
                nv12.copy_from_slice(&original);
                let (y, uv) = nv12.split_at_mut(luma_size);
                let planes = Nv12PlanesMut {
                    width: 1920,
                    height: 1080,
                    y,
                    y_stride: 1920,
                    uv,
                    uv_stride: 1920,
                };
                let started = Instant::now();
                apply_colour(tables, planes, area).expect("the picture can be graded");
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        frames.sort_by(f64::total_cmp);
        println!(
            "{name} {}: {:.2} ms median, {:.2} ms best",
            look.key(),
            frames[frames.len() / 2],
            frames[0]
        );
    }
}

/// The picture as BT.709 limited-range NV12, with chroma averaged over each 2x2 block.
fn to_nv12(picture: &RgbImage) -> Vec<u8> {
    let (width, height) = picture.dimensions();
    let luma_size = (width * height) as usize;
    let mut nv12 = vec![0u8; luma_size + luma_size / 2];
    let code = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    for block_y in 0..height / 2 {
        for block_x in 0..width / 2 {
            let mut chroma = [0f32; 2];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = (block_x * 2 + dx, block_y * 2 + dy);
                let rgb = picture
                    .get_pixel(x, y)
                    .0
                    .map(|value| f32::from(value) / 255.0);
                let [luma, pb, pr] = rgb_to_ypbpr(rgb);
                nv12[(y * width + x) as usize] = code(16.0 + 219.0 * luma);
                chroma[0] += pb / 4.0;
                chroma[1] += pr / 4.0;
            }
            let at = luma_size + (block_y * width + block_x * 2) as usize;
            nv12[at] = code(128.0 + 224.0 * chroma[0]);
            nv12[at + 1] = code(128.0 + 224.0 * chroma[1]);
        }
    }
    nv12
}

/// Converts to NV12, grades through the C layer and converts back.
fn grade(picture: &RgbImage, tables: &ColourTables) -> RgbImage {
    let (width, height) = picture.dimensions();
    let luma_size = (width * height) as usize;
    let mut nv12 = to_nv12(picture);
    let (y, uv) = nv12.split_at_mut(luma_size);
    let planes = Nv12PlanesMut {
        width,
        height,
        y,
        y_stride: width as usize,
        uv,
        uv_stride: width as usize,
    };
    let area = PixelRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    apply_colour(tables, planes, area).expect("the picture can be graded");

    RgbImage::from_fn(width, height, |x, y| {
        let at = luma_size + ((y / 2) * width + (x / 2) * 2) as usize;
        let rgb = codes_to_rgb([nv12[(y * width + x) as usize], nv12[at], nv12[at + 1]]);
        Rgb(rgb.map(|channel| channel.round().clamp(0.0, 255.0) as u8))
    })
}

/// Skin from very fair to very dark, in light and in shade.
const SKIN: [[u8; 3]; 10] = [
    [250, 222, 205],
    [241, 194, 167],
    [224, 172, 140],
    [212, 158, 122],
    [198, 134, 102],
    [176, 120, 86],
    [161, 102, 72],
    [130, 84, 60],
    [104, 66, 48],
    [72, 46, 36],
];

/// One row per look: its name, skin chips, a grey ramp, and the swatch strip the interface
/// shows (with the reference colours above the first row).
fn chips(tables: &[(LookId, Box<ColourTables>)]) -> RgbImage {
    const CHIP: u32 = 36;
    const NAME: u32 = 150;
    let ramp = 12;
    let columns = SKIN.len() as u32 + 1 + ramp + 1 + SWATCH_COLOURS.len() as u32;
    let rows = tables.len() as u32 + 1;
    let mut sheet = RgbImage::from_pixel(
        NAME + columns * CHIP + GAP,
        GAP + rows * (CHIP + GAP),
        BACKGROUND,
    );
    let draw_row = |sheet: &mut RgbImage, row: u32, colours: Vec<Option<[u8; 3]>>| {
        let y = GAP + row * (CHIP + GAP);
        for (column, colour) in colours.into_iter().enumerate() {
            let Some(colour) = colour else { continue };
            let x = NAME + column as u32 * CHIP;
            for dy in 0..CHIP {
                for dx in 0..CHIP - 2 {
                    sheet.put_pixel(x + dx, y + dy, Rgb(colour));
                }
            }
        }
    };
    let greys: Vec<[u8; 3]> = (0..ramp)
        .map(|step| {
            let value = (step * 255 / (ramp - 1)) as u8;
            [value; 3]
        })
        .collect();
    let row_for = |apply: &dyn Fn([u8; 3]) -> [u8; 3], strip: [[u8; 3]; 6]| {
        let mut colours: Vec<Option<[u8; 3]>> =
            SKIN.iter().map(|skin| Some(apply(*skin))).collect();
        colours.push(None);
        colours.extend(greys.iter().map(|grey| Some(apply(*grey))));
        colours.push(None);
        colours.extend(strip.iter().map(|colour| Some(*colour)));
        colours
    };
    text(&mut sheet, "source", GAP, GAP + 12, 3);
    draw_row(&mut sheet, 0, row_for(&|colour| colour, SWATCH_COLOURS));
    for (index, (look, tables)) in tables.iter().enumerate() {
        let row = index as u32 + 1;
        text(
            &mut sheet,
            look.key(),
            GAP,
            GAP + row * (CHIP + GAP) + 12,
            3,
        );
        draw_row(
            &mut sheet,
            row,
            row_for(&|colour| tables.apply_rgb(colour), swatches(*look)),
        );
    }
    sheet
}

// ---------------------------------------------------------------------------------------------
// Labels, in a 3x5 pixel font
// ---------------------------------------------------------------------------------------------

fn glyph(letter: char) -> [u8; 5] {
    match letter.to_ascii_uppercase() {
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '-' => [0, 0, 7, 0, 0],
        _ => [0; 5],
    }
}

fn text(sheet: &mut RgbImage, label: &str, x: u32, y: u32, scale: u32) {
    for (index, letter) in label.chars().enumerate() {
        let left = x + index as u32 * 4 * scale;
        for (row, bits) in glyph(letter).iter().enumerate() {
            for column in 0..3 {
                if bits & (4 >> column) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (left + column * scale + dx, y + row as u32 * scale + dy);
                        if px < sheet.width() && py < sheet.height() {
                            sheet.put_pixel(px, py, INK);
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Synthetic portraits
// ---------------------------------------------------------------------------------------------

struct Scene {
    skin: [u8; 3],
    shirt: [u8; 3],
    hair: [u8; 3],
    wall: [[u8; 3]; 2],
    /// Where the key light comes from (x, y, z toward the viewer), and its colour and strength.
    light: [f32; 3],
    light_colour: [f32; 3],
    ambient: f32,
    window: bool,
    plant: bool,
    shelf: bool,
}

const SCENES: [Scene; 3] = [
    // Fair skin by a bright window, beige wall, white shirt.
    Scene {
        skin: [236, 188, 160],
        shirt: [232, 232, 226],
        hair: [120, 84, 52],
        wall: [[214, 198, 176], [176, 160, 140]],
        light: [0.7, -0.3, 0.65],
        light_colour: [1.0, 0.98, 0.95],
        ambient: 0.35,
        window: true,
        plant: true,
        shelf: false,
    },
    // Medium skin in a cool office lit by a screen, navy shirt and a colourful shelf.
    Scene {
        skin: [196, 140, 104],
        shirt: [36, 52, 92],
        hair: [36, 28, 24],
        wall: [[150, 160, 170], [110, 118, 128]],
        light: [-0.2, -0.1, 1.0],
        light_colour: [0.9, 0.95, 1.0],
        ambient: 0.3,
        window: false,
        plant: false,
        shelf: true,
    },
    // Deep skin under a warm lamp in a dark room, rust shirt.
    Scene {
        skin: [120, 78, 56],
        shirt: [150, 70, 44],
        hair: [20, 16, 14],
        wall: [[70, 60, 52], [28, 26, 26]],
        light: [-0.75, -0.4, 0.55],
        light_colour: [1.0, 0.85, 0.65],
        ambient: 0.18,
        window: false,
        plant: true,
        shelf: false,
    },
];

fn linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

fn portrait(scene: &Scene) -> RgbImage {
    let (width, height) = (640u32, 480u32);
    let mut noise = 0x1234_5678_u32;
    let length = scene.light.iter().map(|v| v * v).sum::<f32>().sqrt();
    let light = scene.light.map(|v| v / length);
    let shade = |albedo: [u8; 3], normal: [f32; 3], extra: f32| {
        let diffuse = (normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]).max(0.0);
        std::array::from_fn(|channel| {
            linear(albedo[channel])
                * (scene.ambient + diffuse * scene.light_colour[channel] * 1.1 + extra)
        })
    };

    RgbImage::from_fn(width, height, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        // The wall: a vertical gradient.
        let t = fy / height as f32;
        let mut colour: [f32; 3] = std::array::from_fn(|channel| {
            let top = linear(scene.wall[0][channel]);
            let bottom = linear(scene.wall[1][channel]);
            top + (bottom - top) * t
        });
        if scene.window && fx > 470.0 && fy < 300.0 {
            // A window, bright enough to clip toward its middle.
            let glow = 1.6 - ((fx - 560.0).abs() / 120.0 + (fy - 140.0).abs() / 200.0);
            colour = [glow.max(0.7) * 0.95, glow.max(0.7), glow.max(0.7) * 1.02];
        }
        if scene.shelf && fx < 200.0 && fy > 60.0 && fy < 330.0 {
            let book = (fx / 18.0) as u32;
            let shelf_row = ((fy - 60.0) / 90.0) as u32;
            let books = [
                [180, 40, 40],
                [40, 90, 160],
                [220, 180, 50],
                [50, 130, 80],
                [200, 200, 190],
                [120, 60, 130],
                [230, 120, 40],
            ];
            let pick = books[((book + shelf_row * 3) % books.len() as u32) as usize];
            if (fy - 60.0) % 90.0 > 12.0 && fx % 18.0 > 2.0 {
                colour = pick.map(linear).map(|v| v * 0.7);
            } else {
                colour = [0.05, 0.04, 0.03];
            }
        }
        if scene.plant {
            // Overlapping leaves on the left.
            for (cx, cy, r) in [
                (70.0, 330.0, 60.0),
                (110.0, 260.0, 45.0),
                (40.0, 220.0, 40.0),
            ] {
                let d = ((fx - cx) * (fx - cx) + (fy - cy) * (fy - cy) * 2.2).sqrt();
                if d < r {
                    let leaf = 0.5 + 0.5 * ((fx * 0.11).sin() * (fy * 0.07).cos());
                    colour = [0.04 + 0.03 * leaf, 0.16 + 0.12 * leaf, 0.03 + 0.02 * leaf];
                }
            }
        }

        // Shoulders.
        let (sx, sy) = ((fx - 320.0) / 230.0, (fy - 520.0) / 170.0);
        if sx * sx + sy * sy < 1.0 {
            let normal = [sx * 0.8, -0.5, (1.0 - (sx * sx).min(1.0)).sqrt()];
            colour = shade(scene.shirt, normal, 0.0);
        }
        // Neck.
        if (fx - 320.0).abs() < 46.0 && fy > 270.0 && fy < 370.0 {
            let nx = (fx - 320.0) / 46.0;
            let normal = [nx, 0.0, (1.0 - nx * nx).max(0.0).sqrt()];
            colour = shade(scene.skin, normal, 0.0).map(|v| v * 0.75);
        }
        // Head: an ellipsoid with hair on top.
        let (hx, hy) = ((fx - 320.0) / 88.0, (fy - 200.0) / 112.0);
        let head = hx * hx + hy * hy;
        if head < 1.0 {
            let nz = (1.0 - head).sqrt();
            let normal = [hx, hy, nz];
            let hairline = -0.55 + 0.25 * hx * hx;
            if hy < hairline || (hx.abs() > 0.9 && hy < 0.1) {
                colour = shade(scene.hair, normal, 0.0);
            } else {
                // A soft specular sheen on the lit side of the forehead and cheeks.
                let half = [light[0], light[1], light[2] + 1.0];
                let half_length = half.iter().map(|v| v * v).sum::<f32>().sqrt();
                let specular = ((normal[0] * half[0] + normal[1] * half[1] + normal[2] * half[2])
                    / half_length)
                    .max(0.0)
                    .powi(24)
                    * 0.25;
                colour = shade(scene.skin, normal, 0.0).map(|v| v + specular);
                // Cheeks a little redder.
                let cheek = (-((hx.abs() - 0.45).powi(2) + (hy - 0.25).powi(2)) / 0.03).exp();
                colour[0] *= 1.0 + 0.12 * cheek;
                colour[2] *= 1.0 - 0.06 * cheek;
                // Eyes, brows and lips.
                for side in [-1.0f32, 1.0] {
                    let (ex, ey) = ((hx - side * 0.35) / 0.14, (hy + 0.08) / 0.06);
                    if ex * ex + ey * ey < 1.0 {
                        colour = if ex * ex + ey * ey < 0.3 {
                            [0.01, 0.008, 0.006]
                        } else {
                            [0.6, 0.6, 0.58].map(|v: f32| v * (0.4 + nz * 0.5))
                        };
                    }
                    let (bx, by) = ((hx - side * 0.35) / 0.18, (hy + 0.22) / 0.035);
                    if bx * bx + by * by < 1.0 {
                        colour = scene.hair.map(linear);
                    }
                }
                let (lx, ly) = (hx / 0.26, (hy - 0.52) / 0.07);
                if lx * lx + ly * ly < 1.0 {
                    colour = [colour[0] * 0.95, colour[1] * 0.62, colour[2] * 0.62];
                }
            }
        }

        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let grain = (noise % 1000) as f32 / 1000.0 - 0.5;
        Rgb(colour.map(|v| encoded(v * (1.0 + grain * 0.04))))
    })
}
