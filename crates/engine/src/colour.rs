//! The colour stage: a look and the basic adjustments, folded into lookup tables that the C layer
//! applies to the finished NV12 frame, so the preview and the camera other apps see are graded
//! identically.
//!
//! Nothing here converts pixels to RGB. Each 2x2 block of an NV12 picture shares one chroma
//! sample, and the grade is a handful of table lookups per block:
//!
//! 1. A **chroma map**, a 256x256 table indexed by U and V, scales and turns colour by its hue
//!    and saturation (saturation, vibrance with skin protection, hue shifts, user saturation),
//!    and says how far the block's luma moves for its colour: the **luma mix**, a coloured
//!    filter in effect.
//! 2. A **tone curve** maps luma through a 256-entry table: exposure, contrast, highlight
//!    roll-off and black lift.
//! 3. A **gain** by the block's luma scales the colour again: saturation by brightness, and the
//!    colour change a tone curve applied to R, G and B would have caused.
//! 4. An **offset** by the block's luma adds split toning and white balance.
//! 5. A **gamut limit** pulls a block's colour toward neutral, keeping its hue, when it would
//!    otherwise leave RGB and clip in the app receiving the camera.
//!
//! The adjustments act first (exposure, contrast, saturation, temperature, tint), then the look
//! on top. The look's intensity blends its tables with the identity, never pixels.
//!
//! The picture is BT.709 limited range: luma 16 to 235, chroma 16 to 240 around 128.

use serde::{Deserialize, Serialize};

use crate::looks::{Look, LookId, Tint};

/// BT.709 luma weights.
const KR: f32 = 0.2126;
const KB: f32 = 0.0722;
const KG: f32 = 1.0 - KR - KB;
/// Pb and Pr are (B - Y) and (R - Y) divided by these.
const PB_SCALE: f32 = 2.0 * (1.0 - KB);
const PR_SCALE: f32 = 2.0 * (1.0 - KR);
/// Limited range: black and white luma codes, and the chroma span.
const LUMA_BLACK: f32 = 16.0;
const LUMA_SPAN: f32 = 219.0;
const CHROMA_SPAN: f32 = 224.0;

/// The display gamma exposure is applied through, so a stop means what it means on a camera.
const DISPLAY_GAMMA: f32 = 2.4;
/// How much colour follows the slope of the tone curve, as it would if the curve were applied
/// to R, G and B: 1 follows fully, 0 not at all. Slightly under half keeps contrast from
/// saturating and roll-off from greying faces.
const CHROMA_FOLLOWS_TONE: f32 = 0.45;
/// The hue of skin of every complexion, on the colour wheel. Darker and lighter skin differ in
/// brightness and saturation far more than in hue.
const SKIN_HUE: f32 = 25.0;
/// How far either side of [`SKIN_HUE`] skin protection reaches, in chroma-plane degrees.
const SKIN_WIDTH: f32 = 24.0;
/// Colour this saturated, in Pb/Pr units, is near the edge of what RGB can show; extra
/// saturation beyond it is compressed instead of clipped.
const GAMUT_KNEE: f32 = 0.28;
const GAMUT_LIMIT: f32 = 0.45;

/// The adjustment ranges at the ends of their sliders.
const EXPOSURE_STOPS: f32 = 1.5;
const CONTRAST_STRENGTH: f32 = 0.35;
const CONTRAST_PIVOT: f32 = 0.45;
/// Red and blue gains at full temperature, and green at full tint, on grey.
const TEMPERATURE_GAIN: f32 = 0.12;
const TINT_GAIN: f32 = 0.08;

/// Fixed-point scale of the gain, offset and mix tables.
const ONE: f32 = 4096.0;

/// The look and the basic adjustments. Kept per computer with the other picture settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColourSettings {
    pub look: LookId,
    /// How much of the look, from 0 (none) to 1 (all).
    pub intensity: f32,
    /// Each adjustment runs from -1 to 1, with 0 changing nothing.
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Positive is warmer.
    pub temperature: f32,
    /// Positive is more magenta, negative more green.
    pub tint: f32,
}

impl Default for ColourSettings {
    fn default() -> Self {
        Self {
            look: LookId::None,
            intensity: 1.0,
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
        }
    }
}

impl ColourSettings {
    /// The settings with every value in range.
    pub fn normalized(self) -> Self {
        let clamp = |value: f32, low: f32, high: f32, fallback: f32| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                fallback
            }
        };
        Self {
            look: self.look,
            intensity: clamp(self.intensity, 0.0, 1.0, 1.0),
            exposure: clamp(self.exposure, -1.0, 1.0, 0.0),
            contrast: clamp(self.contrast, -1.0, 1.0, 0.0),
            saturation: clamp(self.saturation, -1.0, 1.0, 0.0),
            temperature: clamp(self.temperature, -1.0, 1.0, 0.0),
            tint: clamp(self.tint, -1.0, 1.0, 0.0),
        }
    }

    /// Whether any adjustment is away from zero.
    pub fn is_adjusted(&self) -> bool {
        [
            self.exposure,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
        ]
        .iter()
        .any(|value| *value != 0.0)
    }

    /// Whether these settings leave the picture exactly as it is.
    pub fn is_identity(&self) -> bool {
        !self.is_adjusted() && (self.look == LookId::None || self.intensity == 0.0)
    }
}

// ---------------------------------------------------------------------------------------------
// Colour spaces
// ---------------------------------------------------------------------------------------------

/// Gamma-encoded R'G'B' (0 to 1) to Y' (0 to 1), Pb and Pr (-0.5 to 0.5), BT.709.
pub fn rgb_to_ypbpr(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let y = KR * r + KG * g + KB * b;
    [y, (b - y) / PB_SCALE, (r - y) / PR_SCALE]
}

/// Y'PbPr back to R'G'B', unclamped.
pub fn ypbpr_to_rgb(ypbpr: [f32; 3]) -> [f32; 3] {
    let [y, pb, pr] = ypbpr;
    let r = y + PR_SCALE * pr;
    let b = y + PB_SCALE * pb;
    let g = (y - KR * r - KB * b) / KG;
    [r, g, b]
}

/// 8-bit R'G'B' to limited-range Y, U and V codes.
pub fn rgb_to_codes(rgb: [u8; 3]) -> [u8; 3] {
    let [y, pb, pr] = rgb_to_ypbpr(rgb.map(|channel| f32::from(channel) / 255.0));
    [
        code(LUMA_BLACK + LUMA_SPAN * y),
        code(128.0 + CHROMA_SPAN * pb),
        code(128.0 + CHROMA_SPAN * pr),
    ]
}

/// Limited-range Y, U and V codes to R'G'B' on a 0 to 255 scale, unclamped, so callers can
/// tell when a colour falls outside what a screen can show.
pub fn codes_to_rgb(codes: [u8; 3]) -> [f32; 3] {
    let [y, u, v] = codes.map(f32::from);
    ypbpr_to_rgb([
        (y - LUMA_BLACK) / LUMA_SPAN,
        (u - 128.0) / CHROMA_SPAN,
        (v - 128.0) / CHROMA_SPAN,
    ])
    .map(|channel| channel * 255.0)
}

fn code(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// The direction of a hue on the Pb/Pr plane, as a unit vector.
fn hue_direction(hue: f32) -> [f32; 2] {
    let [_, pb, pr] = rgb_to_ypbpr(hsv_rgb(hue));
    let length = (pb * pb + pr * pr).sqrt().max(f32::EPSILON);
    [pb / length, pr / length]
}

/// The angle of a hue on the Pb/Pr plane, in degrees.
fn chroma_angle(hue: f32) -> f32 {
    let [pb, pr] = hue_direction(hue);
    pr.atan2(pb).to_degrees()
}

/// A fully saturated, full brightness colour of `hue` degrees.
fn hsv_rgb(hue: f32) -> [f32; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

fn tint_vector(tint: Tint) -> [f32; 2] {
    let [pb, pr] = hue_direction(tint.hue);
    [pb * tint.amount, pr * tint.amount]
}

/// The chroma a white balance shift gives white, so grey of luma `y` gets `y` times it.
fn white_balance_vector(temperature: f32, tint: f32) -> [f32; 2] {
    let rgb = [
        1.0 + TEMPERATURE_GAIN * temperature,
        1.0 - TINT_GAIN * tint,
        1.0 - TEMPERATURE_GAIN * temperature,
    ];
    let [_, pb, pr] = rgb_to_ypbpr(rgb);
    [pb, pr]
}

// ---------------------------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------------------------

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The cubic from (`x0`, `y0`) with slope `m0` to (`x1`, `y1`) with slope `m1`.
fn hermite(x: f32, (x0, y0, m0): (f32, f32, f32), (x1, y1, m1): (f32, f32, f32)) -> f32 {
    let h = x1 - x0;
    let t = (x - x0) / h;
    let (t2, t3) = (t * t, t * t * t);
    y0 * (2.0 * t3 - 3.0 * t2 + 1.0)
        + h * m0 * (t3 - 2.0 * t2 + t)
        + y1 * (3.0 * t2 - 2.0 * t3)
        + h * m1 * (t3 - t2)
}

/// Brightens or darkens by `stops` in linear light. Brightening eases highlights toward white
/// instead of clipping them.
fn exposure(x: f32, stops: f32) -> f32 {
    if stops == 0.0 {
        return x;
    }
    let gain = 2f32.powf(stops);
    let exposed = if x > 0.0 {
        (x.powf(DISPLAY_GAMMA) * gain).powf(1.0 / DISPLAY_GAMMA)
    } else {
        x * gain.powf(1.0 / DISPLAY_GAMMA)
    };
    if stops > 0.0 && exposed > 0.8 {
        let soft = 0.8 + 0.2 * ((exposed - 0.8) / 0.2).tanh();
        lerp(exposed, soft, (stops / 0.25).min(1.0))
    } else {
        exposed
    }
}

/// An S-curve through (`pivot`, `pivot`) with slope `1 + strength` there and `1 - strength` at
/// black and white, which stay put. Monotonic for strengths between -1 and 1.
fn contrast(x: f32, strength: f32, pivot: f32) -> f32 {
    if strength == 0.0 {
        return x;
    }
    let (end, middle) = (1.0 - strength, 1.0 + strength);
    if x <= 0.0 {
        x * end
    } else if x >= 1.0 {
        1.0 + (x - 1.0) * end
    } else if x < pivot {
        hermite(x, (0.0, 0.0, end), (pivot, pivot, middle))
    } else {
        hermite(x, (pivot, pivot, middle), (1.0, 1.0, end))
    }
}

/// A highlight shoulder: from a knee that drops as `amount` grows, the curve bends to end a
/// little below white with a shallow slope.
fn rolloff(x: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return x;
    }
    let knee = 1.0 - 0.4 * amount;
    let white = 1.0 - 0.06 * amount;
    let slope = 1.0 - 0.8 * amount;
    if x <= knee {
        x
    } else if x < 1.0 {
        hermite(x, (knee, knee, 1.0), (1.0, white, slope))
    } else {
        white + (x - 1.0) * slope
    }
}

/// Lifts black to `amount`, fading out by the lower midtones. Monotonic up to 0.3.
fn fade(x: f32, amount: f32) -> f32 {
    x + amount * (1.0 - smoothstep(0.0, 0.45, x))
}

/// What the adjustments do to luma.
fn adjusted_luma(x: f32, settings: &ColourSettings) -> f32 {
    contrast(
        exposure(x, settings.exposure * EXPOSURE_STOPS),
        settings.contrast * CONTRAST_STRENGTH,
        CONTRAST_PIVOT,
    )
}

/// What the look does to luma, at full intensity.
fn look_luma(x: f32, look: &Look) -> f32 {
    fade(look_shape(x, look), look.fade)
}

/// The part of the look's tone curve that would change colour too if it were applied to R, G
/// and B: everything but the black lift, which adds the same to every channel.
fn look_shape(x: f32, look: &Look) -> f32 {
    rolloff(
        contrast(exposure(x, look.exposure), look.contrast, look.pivot),
        look.rolloff,
    )
}

/// The look's saturation at luma `x`.
fn look_saturation_at(x: f32, look: &Look) -> f32 {
    let shadows = 1.0 - smoothstep(0.0, 0.45, x);
    let highlights = smoothstep(0.55, 1.0, x);
    1.0 + (look.shadow_saturation - 1.0) * shadows + (look.highlight_saturation - 1.0) * highlights
}

/// The look's colour offset at luma `x`, in Pb/Pr: split toning and white balance.
fn look_offset_at(x: f32, look: &Look) -> [f32; 2] {
    let shadows = 1.0 - smoothstep(0.0, 0.55 + look.balance, x);
    let highlights = smoothstep(0.45 + look.balance, 1.0, x);
    let [sb, sr] = tint_vector(look.shadows);
    let [hb, hr] = tint_vector(look.highlights);
    let [wb, wr] = white_balance_vector(look.warmth, look.tint);
    let grey = x.max(0.0);
    [
        sb * shadows + hb * highlights + wb * grey,
        sr * shadows + hr * highlights + wr * grey,
    ]
}

/// How much colour a pixel can keep once its luma moves from `before` to `after`. Near white
/// and black there is little room for colour: brightening a highlight or darkening a shadow
/// with its colour unchanged would push a channel past what a screen can show, so colour
/// shrinks with the room left, as it would if the curve were applied to R, G and B.
fn headroom(before: f32, after: f32) -> f32 {
    const MARGIN: f32 = 0.04;
    let mut factor = 1.0;
    if after > before {
        let room = ((1.0 + MARGIN - after) / (1.0 + MARGIN - before).max(MARGIN)).clamp(0.1, 1.0);
        factor *= lerp(1.0, room.sqrt(), smoothstep(0.55, 0.85, before));
    }
    if after < before {
        let room = ((after + MARGIN) / (before + MARGIN).max(MARGIN)).clamp(0.1, 1.0);
        factor *= lerp(1.0, room.sqrt(), 1.0 - smoothstep(0.1, 0.35, before));
    }
    factor
}

/// The smallest difference between two angles in degrees, from -180 to 180.
fn angle_between(a: f32, b: f32) -> f32 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// The look's colour change at full intensity, for chroma `(pb, pr)`.
fn look_chroma(pb: f32, pr: f32, look: &Look) -> [f32; 2] {
    let radius = (pb * pb + pr * pr).sqrt();
    if radius < 1e-6 {
        return [pb, pr];
    }
    let angle = pr.atan2(pb).to_degrees();
    // Hue only means something once there is some colour.
    let coloured = smoothstep(0.0, 0.03, radius);
    let skin = coloured
        * (-0.5 * (angle_between(angle, chroma_angle(SKIN_HUE)) / SKIN_WIDTH).powi(2)).exp();
    let unprotected = 1.0 - look.skin_protection * skin;

    let mut turn = 0.0;
    let mut scale = 1.0;
    for shift in &look.hue_shifts {
        if shift.shift == 0.0 && shift.saturation == 1.0 {
            continue;
        }
        let centre = chroma_angle(shift.hue);
        let weight =
            coloured * (-0.5 * (angle_between(angle, centre) / shift.width.max(1.0)).powi(2)).exp();
        let target = angle_between(chroma_angle(shift.hue + shift.shift), centre);
        turn += target * weight;
        scale *= 1.0 + (shift.saturation - 1.0) * weight;
    }

    let vibrance = 1.0 + look.vibrance * (1.0 - smoothstep(0.0, 0.3, radius));
    let gain = look.saturation * vibrance * scale;
    let gain = 1.0 + (gain - 1.0) * unprotected;
    let turn = (turn * unprotected).to_radians();

    let (sin, cos) = turn.sin_cos();
    let mut out = [(pb * cos - pr * sin) * gain, (pb * sin + pr * cos) * gain];
    gamut_compress(radius, &mut out);
    out
}

/// Compresses saturation added beyond [`GAMUT_KNEE`] smoothly toward [`GAMUT_LIMIT`], so a
/// saturating look never pushes colours that were already vivid into clipping. Colours that
/// were not made more saturated are left alone.
fn gamut_compress(radius_before: f32, chroma: &mut [f32; 2]) {
    let radius = (chroma[0] * chroma[0] + chroma[1] * chroma[1]).sqrt();
    let knee = GAMUT_KNEE.max(radius_before);
    if radius <= knee || radius <= radius_before {
        return;
    }
    let room = (GAMUT_LIMIT - knee).max(0.01);
    let compressed = knee + room * ((radius - knee) / room).tanh();
    let scale = compressed / radius;
    chroma[0] *= scale;
    chroma[1] *= scale;
}

// ---------------------------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------------------------

/// The tables the C layer applies. Mirrors `PcColour` in `native/decoder.c`.
#[repr(C)]
pub struct ColourTables {
    /// The tone curve, indexed by luma plus the block's luma shift plus 128.
    pub(crate) luma: [u8; 512],
    /// Chroma gain by the block's average luma, in 1/4096.
    pub(crate) gain: [i32; 256],
    /// Chroma offsets by the block's average luma, in 1/4096 of a code.
    pub(crate) offset_u: [i32; 256],
    pub(crate) offset_v: [i32; 256],
    /// Whether to pull colours that would leave RGB back toward neutral: on for every grade,
    /// off for the identity so it stays bit for bit.
    pub(crate) gamut: i32,
    /// 65536 / n, so the gamut limit multiplies instead of dividing.
    pub(crate) reciprocals: [i32; RECIPROCALS],
    /// The chroma map, indexed by `u << 8 | v`, holding `u' | v' << 8 | (shift + 128) << 16`:
    /// the new chroma, and how far the block's luma moves for its colour (the luma mix).
    pub(crate) chroma: [u32; 65536],
}

/// The chroma map entry that changes nothing.
fn identity_entry(index: usize) -> u32 {
    ((index & 0xFF) << 8 | index >> 8 | 128 << 16) as u32
}

/// Spacing of the grid the chroma map is computed on before it is filled in between.
const GRID_STEP: usize = 8;
const GRID_SIZE: usize = 256 / GRID_STEP + 1;

impl ColourTables {
    /// The tables for `settings`.
    pub fn build(settings: &ColourSettings) -> Box<Self> {
        let settings = settings.normalized();
        let look = settings.look.look();
        let k = settings.intensity;
        // SAFETY: every field is an integer array or integer, for which all zeros is valid.
        let mut tables: Box<Self> = unsafe { Box::new_zeroed().assume_init() };

        let adjusted = settings.is_adjusted();
        let curve = |x: f32| {
            let y = if adjusted {
                adjusted_luma(x, &settings)
            } else {
                x
            };
            if k == 0.0 {
                y
            } else {
                lerp(y, look_luma(y, look), k)
            }
        };
        // The curve colour follows: the same without the black lift.
        let shape = |x: f32| {
            let y = if adjusted {
                adjusted_luma(x, &settings)
            } else {
                x
            };
            if k == 0.0 {
                y
            } else {
                lerp(y, look_shape(y, look), k)
            }
        };

        // Tone.
        let mut luma = [0u8; 256];
        for (index, out) in luma.iter_mut().enumerate() {
            let x = (index as f32 - LUMA_BLACK) / LUMA_SPAN;
            *out = code(LUMA_BLACK + LUMA_SPAN * curve(x));
        }
        for (index, out) in tables.luma.iter_mut().enumerate() {
            *out = luma[index.saturating_sub(128).min(255)];
        }

        // Gain and offset by luma.
        let user_balance = white_balance_vector(settings.temperature, settings.tint);
        let step = 1.0 / LUMA_SPAN;
        for index in 0..256 {
            let x = (index as f32 - LUMA_BLACK) / LUMA_SPAN;
            let slope = (shape(x + step) - shape(x - step)) / (2.0 * step);
            let y = if adjusted {
                adjusted_luma(x, &settings)
            } else {
                x
            };
            let gain = slope.clamp(0.2, 2.5).powf(CHROMA_FOLLOWS_TONE)
                * lerp(1.0, look_saturation_at(y, look), k)
                * headroom(x, curve(x));
            let grey = x.max(0.0);
            let look_offset = look_offset_at(y, look);
            let offset = [
                gain * user_balance[0] * grey + k * look_offset[0],
                gain * user_balance[1] * grey + k * look_offset[1],
            ];
            tables.gain[index] = (gain * ONE).round() as i32;
            tables.offset_u[index] = (offset[0] * CHROMA_SPAN * ONE).round() as i32;
            tables.offset_v[index] = (offset[1] * CHROMA_SPAN * ONE).round() as i32;
        }

        // The luma mix: luma added per code of U and V away from neutral, in 1/4096.
        let [mix_b, mix_r] = tint_vector(look.luma_mix);
        let per_code = k * LUMA_SPAN / CHROMA_SPAN * ONE;
        let (mix_u, mix_v) = (
            (mix_b * per_code).round() as i32,
            (mix_r * per_code).round() as i32,
        );

        // The chroma map, computed on a coarse grid and filled in between: it varies slowly,
        // and the grid keeps rebuilding cheap enough to follow a slider.
        let user_saturation = 1.0 + settings.saturation;
        let mut grid = vec![[[0f32; 2]; GRID_SIZE]; GRID_SIZE];
        for (u_index, row) in grid.iter_mut().enumerate() {
            for (v_index, out) in row.iter_mut().enumerate() {
                let pb = ((u_index * GRID_STEP) as f32 - 128.0) / CHROMA_SPAN * user_saturation;
                let pr = ((v_index * GRID_STEP) as f32 - 128.0) / CHROMA_SPAN * user_saturation;
                let [graded_b, graded_r] = if k == 0.0 {
                    [pb, pr]
                } else {
                    look_chroma(pb, pr, look)
                };
                *out = [
                    128.0 + CHROMA_SPAN * lerp(pb, graded_b, k),
                    128.0 + CHROMA_SPAN * lerp(pr, graded_r, k),
                ];
            }
        }
        for u in 0..256 {
            let (u_index, u_fraction) = (u / GRID_STEP, (u % GRID_STEP) as f32 / GRID_STEP as f32);
            for v in 0..256 {
                let (v_index, v_fraction) =
                    (v / GRID_STEP, (v % GRID_STEP) as f32 / GRID_STEP as f32);
                let at = |du: usize, dv: usize| grid[u_index + du][v_index + dv];
                let mut value = [0f32; 2];
                for (channel, out) in value.iter_mut().enumerate() {
                    let top = lerp(at(0, 0)[channel], at(0, 1)[channel], v_fraction);
                    let bottom = lerp(at(1, 0)[channel], at(1, 1)[channel], v_fraction);
                    *out = lerp(top, bottom, u_fraction);
                }
                let shift = ((mix_u * (u as i32 - 128) + mix_v * (v as i32 - 128) + 2048) >> 12)
                    .clamp(-128, 127);
                tables.chroma[u << 8 | v] = u32::from(code(value[0]))
                    | u32::from(code(value[1])) << 8
                    | ((shift + 128) as u32) << 16;
            }
        }
        tables.gamut = i32::from(!tables.is_identity());
        tables.reciprocals[0] = 65536;
        for (n, reciprocal) in tables.reciprocals.iter_mut().enumerate().skip(1) {
            *reciprocal = 65536 / n as i32;
        }
        tables
    }

    /// Whether the tables change nothing.
    pub fn is_identity(&self) -> bool {
        self.gamut == 0
            && self
                .luma
                .iter()
                .enumerate()
                .all(|(index, value)| usize::from(*value) == index.saturating_sub(128).min(255))
            && self.gain.iter().all(|gain| *gain == ONE as i32)
            && self.offset_u.iter().all(|offset| *offset == 0)
            && self.offset_v.iter().all(|offset| *offset == 0)
            && self
                .chroma
                .iter()
                .enumerate()
                .all(|(index, value)| *value == identity_entry(index))
    }

    /// Grades one 2x2 block, exactly as the C layer does: four luma values and their chroma.
    pub fn apply_block(&self, luma: [u8; 4], u: u8, v: u8) -> ([u8; 4], u8, u8) {
        let average =
            ((luma.iter().map(|value| usize::from(*value)).sum::<usize>() + 2) >> 2).min(255);
        let mapped = self.chroma[usize::from(u) << 8 | usize::from(v)];
        let curve = &self.luma[(mapped >> 16) as usize..];
        let graded = luma.map(|value| curve[usize::from(value)]);

        let mu = (mapped & 0xFF) as i32 - 128;
        let mv = (mapped >> 8 & 0xFF) as i32 - 128;
        let gain = self.gain[average];
        let mut out_u = (mu * gain + self.offset_u[average] + 2048) >> 12;
        let mut out_v = (mv * gain + self.offset_v[average] + 2048) >> 12;
        if self.gamut != 0 {
            (out_u, out_v) = self.keep_in_gamut(i32::from(curve[average]), out_u, out_v);
        }
        let byte = |value: i32| (128 + value).clamp(0, 255) as u8;
        (graded, byte(out_u), byte(out_v))
    }

    /// Grades one flat colour, given and returned as 8-bit R'G'B'.
    pub fn apply_rgb(&self, rgb: [u8; 3]) -> [u8; 3] {
        let [y, u, v] = rgb_to_codes(rgb);
        let ([y, ..], u, v) = self.apply_block([y; 4], u, v);
        codes_to_rgb([y, u, v]).map(|channel| channel.round().clamp(0.0, 255.0) as u8)
    }
}

/// How far R, G and B sit from luma per code of chroma, in luma codes times 4096 (BT.709,
/// limited range). Mirrors `PC_R_V` and its neighbours in `native/decoder.c`.
const R_PER_V: i32 = 6306;
const B_PER_U: i32 = 7431;
const G_PER_U: i32 = 750;
const G_PER_V: i32 = 1875;
/// The length of the reciprocal table.
const RECIPROCALS: usize = 1024;

impl ColourTables {
    /// Scales chroma `u`, `v` (codes away from 128) toward neutral just enough that a block of
    /// luma `luma` stays inside RGB, keeping its hue. Mirrors `keep_in_gamut` in the C layer.
    fn keep_in_gamut(&self, luma: i32, u: i32, v: i32) -> (i32, i32) {
        // How much fits when a channel reaches `excess` with `room` codes left, rounding the
        // excess up so the result errs toward fitting.
        let fit = |room: i32, excess: i32| {
            let codes = ((excess + 4095) >> 12).clamp(0, RECIPROCALS as i32 - 1);
            (room * self.reciprocals[codes as usize]) >> 4
        };
        let above = (luma - 16).clamp(0, 219);
        let below = 219 - above;
        // Most colours are nowhere near the edge.
        if u.abs().max(v.abs()) * B_PER_U <= above.min(below) << 12 {
            return (u, v);
        }
        let r = v * R_PER_V;
        let g = -(u * G_PER_U + v * G_PER_V);
        let b = u * B_PER_U;
        let high = r.max(g).max(b);
        let low = r.min(g).min(b);
        let over = if high > below << 12 {
            fit(below, high)
        } else {
            4096
        };
        let under = if -low > above << 12 {
            fit(above, -low)
        } else {
            4096
        };
        let scale = over.min(under);
        ((u * scale + 2048) >> 12, (v * scale + 2048) >> 12)
    }
}

/// The colours a look's swatch strip shows, from shadow to highlight: a deep shadow, skin in
/// shade, skin in light, a cool sky blue, foliage, and a bright highlight. Graded by the look's
/// own tables, so the strip previews what the look really does.
pub const SWATCH_COLOURS: [[u8; 3]; 6] = [
    [38, 38, 40],
    [138, 92, 70],
    [224, 172, 140],
    [104, 150, 196],
    [92, 132, 72],
    [226, 226, 222],
];

/// The look's swatch strip at full intensity, with no adjustments, as 8-bit sRGB.
pub fn swatches(look: LookId) -> [[u8; 3]; 6] {
    let tables = ColourTables::build(&ColourSettings {
        look,
        ..Default::default()
    });
    SWATCH_COLOURS.map(|colour| tables.apply_rgb(colour))
}

/// The hue of an 8-bit R'G'B' colour in degrees, and its HSV saturation.
pub fn hue_and_saturation(rgb: [f32; 3]) -> (f32, f32) {
    let max = rgb.iter().copied().fold(f32::MIN, f32::max);
    let min = rgb.iter().copied().fold(f32::MAX, f32::min);
    let chroma = max - min;
    if chroma <= f32::EPSILON || max <= 0.0 {
        return (0.0, 0.0);
    }
    let [r, g, b] = rgb;
    let hue = if max == r {
        60.0 * ((g - b) / chroma)
    } else if max == g {
        60.0 * ((b - r) / chroma + 2.0)
    } else {
        60.0 * ((r - g) / chroma + 4.0)
    };
    (hue.rem_euclid(360.0), chroma / max)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn full(look: LookId) -> ColourSettings {
        ColourSettings {
            look,
            ..Default::default()
        }
    }

    fn identity_luma(index: usize) -> u8 {
        index.saturating_sub(128).min(255) as u8
    }

    #[test]
    fn no_look_and_no_adjustment_change_nothing() {
        let tables = ColourTables::build(&ColourSettings::default());
        assert!(tables.is_identity());
        assert!(ColourSettings::default().is_identity());
        for (index, value) in tables.luma.iter().enumerate() {
            assert_eq!(*value, identity_luma(index));
        }
        // Every block, bit for bit.
        for y in (0..=255u8).step_by(5) {
            for u in (0..=255u8).step_by(3) {
                for v in (0..=255u8).step_by(7) {
                    let luma = [y, y.wrapping_add(1), y.saturating_sub(3), 255 - y];
                    assert_eq!(tables.apply_block(luma, u, v), (luma, u, v));
                }
            }
        }
    }

    #[test]
    fn intensity_zero_is_the_identity_for_every_look() {
        for look in LookId::ALL {
            let tables = ColourTables::build(&ColourSettings {
                look,
                intensity: 0.0,
                ..Default::default()
            });
            assert!(tables.is_identity(), "{look:?}");
        }
    }

    #[test]
    fn every_look_changes_the_picture() {
        for look in &LookId::ALL[1..] {
            assert!(!ColourTables::build(&full(*look)).is_identity(), "{look:?}");
        }
    }

    #[test]
    fn tone_curves_never_turn_back() {
        for look in LookId::ALL {
            for intensity in [0.25, 0.5, 1.0] {
                for (exposure, contrast) in [(0.0, 0.0), (1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                    let tables = ColourTables::build(&ColourSettings {
                        look,
                        intensity,
                        exposure,
                        contrast,
                        ..Default::default()
                    });
                    for pair in tables.luma.windows(2) {
                        assert!(pair[0] <= pair[1], "{look:?} {intensity} {exposure}");
                    }
                    assert!(tables.gain.iter().all(|gain| *gain > 0));
                }
            }
        }
    }

    #[test]
    fn tones_keep_their_order_through_every_look() {
        // Black stays darker than shadow, shadow than mid grey, mid grey than white.
        for look in LookId::ALL {
            let tables = ColourTables::build(&full(look));
            let greys =
                [16u8, 60, 126, 190, 235].map(|y| tables.apply_block([y; 4], 128, 128).0[0]);
            for pair in greys.windows(2) {
                assert!(pair[0] < pair[1], "{look:?} {greys:?}");
            }
        }
    }

    /// The largest colour cast a look gives the greys from black to white, in chroma codes.
    fn largest_cast(tables: &ColourTables) -> i32 {
        (16..=235u8)
            .map(|y| {
                let (_, u, v) = tables.apply_block([y; 4], 128, 128);
                (i32::from(u) - 128).abs().max((i32::from(v) - 128).abs())
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn greys_stay_neutral_unless_the_look_tints_them() {
        for look in LookId::ALL {
            let tables = ColourTables::build(&full(look));
            let parameters = look.look();
            let tints = parameters.shadows.amount != 0.0
                || parameters.highlights.amount != 0.0
                || parameters.warmth != 0.0
                || parameters.tint != 0.0;
            let cast = largest_cast(&tables);
            if tints {
                // A tint is a hint, never a colour wash.
                assert!(cast <= 10, "{look:?} casts {cast}");
            } else {
                assert_eq!(cast, 0, "{look:?}");
            }
        }
        // The adjustments alone leave grey grey too, unless they are white balance.
        let adjusted = ColourTables::build(&ColourSettings {
            exposure: 0.5,
            contrast: 0.5,
            saturation: 0.5,
            ..Default::default()
        });
        assert_eq!(largest_cast(&adjusted), 0);
    }

    #[test]
    fn temperature_and_tint_move_grey_the_right_way() {
        let warm = ColourTables::build(&ColourSettings {
            temperature: 1.0,
            ..Default::default()
        });
        let [r, _, b] = codes_to_rgb({
            let ([y, ..], u, v) = warm.apply_block([126; 4], 128, 128);
            [y, u, v]
        });
        assert!(r > b + 10.0, "{r} {b}");
        let magenta = ColourTables::build(&ColourSettings {
            tint: 1.0,
            ..Default::default()
        });
        let [r, g, b] = codes_to_rgb({
            let ([y, ..], u, v) = magenta.apply_block([126; 4], 128, 128);
            [y, u, v]
        });
        assert!(g + 5.0 < r && g + 5.0 < b, "{r} {g} {b}");
    }

    #[test]
    fn no_look_clips_or_discolours_skin() {
        for look in LookId::ALL {
            let tables = ColourTables::build(&full(look));
            let monochrome = look.look().saturation == 0.0;
            for skin in SKIN {
                let [y, u, v] = rgb_to_codes(skin);
                let ([y, ..], u, v) = tables.apply_block([y; 4], u, v);
                let rgb = codes_to_rgb([y, u, v]);
                assert!(
                    rgb.iter().all(|channel| (-1.5..=256.5).contains(channel)),
                    "{look:?} clips {skin:?} to {rgb:?}"
                );
                let (hue, saturation) = hue_and_saturation(rgb);
                let (skin_hue, skin_saturation) = hue_and_saturation(skin.map(f32::from));
                if monochrome {
                    assert!(saturation < 0.08, "{look:?} leaves colour in {skin:?}");
                    continue;
                }
                // Still skin: orange, never pink-magenta, yellow-green or grey.
                assert!(
                    (5.0..=45.0).contains(&hue),
                    "{look:?} turns {skin:?} to hue {hue}"
                );
                assert!(
                    saturation > skin_saturation * 0.6 && saturation < skin_saturation * 1.45,
                    "{look:?} changes the saturation of {skin:?} from {skin_saturation} to \
                     {saturation}"
                );
                assert!(
                    (hue - skin_hue).abs() < 12.0,
                    "{look:?} {skin:?} {hue} {skin_hue}"
                );
            }
        }
    }

    #[test]
    fn skin_protection_keeps_faces_out_of_a_desaturating_look() {
        // Compared in the chroma map alone, apart from white balance and toning.
        let tables = ColourTables::build(&full(LookId::Nordic));
        let ratio = |rgb: [u8; 3]| {
            let [_, u, v] = rgb_to_codes(rgb);
            let mapped = tables.chroma[usize::from(u) << 8 | usize::from(v)];
            let before = (f32::from(u) - 128.0).hypot(f32::from(v) - 128.0);
            let after = ((mapped & 0xFF) as f32 - 128.0).hypot((mapped >> 8 & 0xFF) as f32 - 128.0);
            after / before
        };
        let (skin, sky) = (ratio([198, 134, 102]), ratio([104, 150, 196]));
        assert!(skin > sky + 0.1, "skin {skin}, sky {sky}");
    }

    #[test]
    fn intensity_blends_between_none_and_the_look() {
        let colour = [104, 150, 196];
        let distance = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| (i32::from(*a) - i32::from(b)).abs())
                .sum::<i32>()
        };
        for look in &LookId::ALL[1..] {
            let at = |intensity| {
                ColourTables::build(&ColourSettings {
                    look: *look,
                    intensity,
                    ..Default::default()
                })
                .apply_rgb(colour)
            };
            let (none, half, all) = (at(0.0), at(0.5), at(1.0));
            assert!(distance(none, half) <= distance(none, all) + 2, "{look:?}");
            assert!(distance(half, all) <= distance(none, all) + 2, "{look:?}");
        }
    }

    #[test]
    fn the_c_layer_grades_exactly_like_the_reference() {
        use crate::decoder::{Nv12PlanesMut, apply_colour};
        use crate::picture::PixelRect;

        let (width, height) = (64u32, 48u32);
        let luma_size = (width * height) as usize;
        let mut state = 0x9e37_79b9_u32;
        let original: Vec<u8> = (0..luma_size + luma_size / 2)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        for look in LookId::ALL {
            let tables = ColourTables::build(&ColourSettings {
                look,
                intensity: 0.8,
                temperature: 0.3,
                saturation: -0.2,
                ..Default::default()
            });
            let mut data = original.clone();
            let (y, uv) = data.split_at_mut(luma_size);
            let planes = Nv12PlanesMut {
                width,
                height,
                y,
                y_stride: width as usize,
                uv,
                uv_stride: width as usize,
            };
            // Leave a border of two pixels to check the area is respected.
            let area = PixelRect {
                x: 2,
                y: 2,
                width: width - 4,
                height: height - 4,
            };
            apply_colour(&tables, planes, area).unwrap();

            let w = width as usize;
            for block_y in 0..height as usize / 2 {
                for block_x in 0..w / 2 {
                    let (x, y) = (block_x * 2, block_y * 2);
                    let at = |data: &[u8], dx: usize, dy: usize| data[(y + dy) * w + x + dx];
                    let luma = [
                        at(&original, 0, 0),
                        at(&original, 1, 0),
                        at(&original, 0, 1),
                        at(&original, 1, 1),
                    ];
                    let chroma = luma_size + block_y * w + x;
                    let (u, v) = (original[chroma], original[chroma + 1]);
                    let inside = x >= 2 && y >= 2 && x < w - 2 && y < height as usize - 2;
                    let expected = if inside {
                        tables.apply_block(luma, u, v)
                    } else {
                        (luma, u, v)
                    };
                    let got = (
                        [
                            at(&data, 0, 0),
                            at(&data, 1, 0),
                            at(&data, 0, 1),
                            at(&data, 1, 1),
                        ],
                        data[chroma],
                        data[chroma + 1],
                    );
                    assert_eq!(got, expected, "{look:?} at block {block_x}, {block_y}");
                }
            }
        }
    }

    #[test]
    fn building_is_deterministic() {
        let settings = ColourSettings {
            look: LookId::Cinema,
            intensity: 0.7,
            exposure: 0.2,
            temperature: -0.3,
            ..Default::default()
        };
        let first = ColourTables::build(&settings);
        let second = ColourTables::build(&settings);
        assert_eq!(first.luma, second.luma);
        assert_eq!(first.gain, second.gain);
        assert_eq!(first.offset_u, second.offset_u);
        assert_eq!(first.chroma[..], second.chroma[..]);
    }

    #[test]
    fn settings_are_clamped_and_saved_by_name() {
        let wild = ColourSettings {
            look: LookId::Dusk,
            intensity: 3.0,
            exposure: f32::NAN,
            contrast: -4.0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(wild.intensity, 1.0);
        assert_eq!(wild.exposure, 0.0);
        assert_eq!(wild.contrast, -1.0);
        let json = serde_json::to_string(&wild).unwrap();
        assert!(json.contains("\"look\":\"dusk\""), "{json}");
        assert_eq!(serde_json::from_str::<ColourSettings>(&json).unwrap(), wild);
        let partial: ColourSettings = serde_json::from_str(r#"{"look":"silver"}"#).unwrap();
        assert_eq!(partial.look, LookId::Silver);
        assert_eq!(partial.intensity, 1.0);
    }

    #[test]
    fn swatches_show_the_look() {
        // No look: the reference colours, give or take rounding.
        for (graded, original) in swatches(LookId::None).iter().zip(SWATCH_COLOURS) {
            for (a, b) in graded.iter().zip(original) {
                assert!((i32::from(*a) - i32::from(b)).abs() <= 2);
            }
        }
        // Black and white: grey swatches.
        for swatch in swatches(LookId::Silver) {
            let (_, saturation) = hue_and_saturation(swatch.map(f32::from));
            assert!(saturation < 0.1, "{swatch:?}");
        }
        // Every look's strip differs from every other's.
        for (index, look) in LookId::ALL.iter().enumerate() {
            for other in &LookId::ALL[index + 1..] {
                assert_ne!(swatches(*look), swatches(*other), "{look:?} {other:?}");
            }
        }
    }
}
