//! The looks: colour grades a person would choose for a call or a stream, each defined here as
//! data and nowhere else. [`crate::colour`] turns a look into the lookup tables the picture
//! thread applies.
//!
//! How to read the parameters. Tone works on display luma, 0 for black and 1 for white:
//!
//! - `exposure` brightens or darkens in stops, as a camera would (in linear light).
//! - `contrast` bends an S-curve through `pivot`: the slope there is `1 + contrast` and at black
//!   and white `1 - contrast`, so detail at the ends is softened rather than clipped. Negative
//!   values flatten the picture.
//! - `rolloff` (0 to 1) bends the top of the curve into a shoulder that starts lower and ends a
//!   little below white, so highlights on a forehead or a window fade out instead of clipping.
//! - `fade` lifts black to this level (a matte finish), easing back to the untouched curve by
//!   the lower midtones.
//!
//! Colour works on the chroma of each pixel:
//!
//! - `saturation` scales all colour; `vibrance` adds more to muted colours than to vivid ones.
//! - `skin_protection` (0 to 1) holds skin hues back from the saturation and hue changes, so
//!   faces stay natural while the rest of the picture moves.
//! - `shadow_saturation` and `highlight_saturation` scale colour by brightness.
//! - `warmth` and `tint` shift white balance on the same scale as the Temperature and Tint
//!   sliders: positive is warmer, and more magenta.
//! - `shadows` and `highlights` tint the dark and bright parts of the picture (split toning);
//!   `balance` moves where one hands over to the other.
//! - `hue_shifts` turn one band of hues toward another and change its saturation.
//! - `luma_mix` brightens colours of one hue and darkens the opposite hue, like a coloured
//!   filter on a black and white camera. Pointed at skin, it lifts faces a little.
//!
//! Hues are in degrees on the familiar colour wheel: 0 red, 25 skin, 40 amber, 60 yellow,
//! 120 green, 180 cyan, 210 azure, 240 blue, 300 magenta. Tint amounts are in colour-difference
//! units, where 0.01 is a just visible cast on grey and 0.04 a strong one.

use serde::{Deserialize, Serialize};

/// Which look is chosen. Stored by its key, so new looks never disturb saved settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum LookId {
    #[default]
    None,
    Natural,
    Crisp,
    Velvet,
    Golden,
    Punch,
    Nordic,
    Pastel,
    Cinema,
    Dusk,
    Vintage,
    Silver,
}

impl LookId {
    /// Every look, from the most neutral to the most stylised.
    pub const ALL: [Self; 12] = [
        Self::None,
        Self::Natural,
        Self::Crisp,
        Self::Velvet,
        Self::Golden,
        Self::Punch,
        Self::Nordic,
        Self::Pastel,
        Self::Cinema,
        Self::Dusk,
        Self::Vintage,
        Self::Silver,
    ];

    /// The stable name in settings files and translation catalogs.
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Natural => "natural",
            Self::Crisp => "crisp",
            Self::Velvet => "velvet",
            Self::Golden => "golden",
            Self::Punch => "punch",
            Self::Nordic => "nordic",
            Self::Pastel => "pastel",
            Self::Cinema => "cinema",
            Self::Dusk => "dusk",
            Self::Vintage => "vintage",
            Self::Silver => "silver",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|look| look.key() == key)
    }

    /// The look's parameters.
    pub fn look(self) -> &'static Look {
        match self {
            Self::None => &Look::NEUTRAL,
            Self::Natural => &NATURAL,
            Self::Crisp => &CRISP,
            Self::Velvet => &VELVET,
            Self::Golden => &GOLDEN,
            Self::Punch => &PUNCH,
            Self::Nordic => &NORDIC,
            Self::Pastel => &PASTEL,
            Self::Cinema => &CINEMA,
            Self::Dusk => &DUSK,
            Self::Vintage => &VINTAGE,
            Self::Silver => &SILVER,
        }
    }
}

/// An unknown key, from a newer or older version, falls back to no look.
impl From<String> for LookId {
    fn from(key: String) -> Self {
        Self::from_key(&key).unwrap_or_default()
    }
}

impl From<LookId> for String {
    fn from(look: LookId) -> Self {
        look.key().to_owned()
    }
}

/// A push toward one hue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    /// Degrees on the colour wheel.
    pub hue: f32,
    pub amount: f32,
}

impl Tint {
    pub const NONE: Self = Self {
        hue: 0.0,
        amount: 0.0,
    };
}

/// Turns the hues around `hue` toward another hue and changes their saturation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HueShift {
    /// The middle of the band, in degrees on the colour wheel.
    pub hue: f32,
    /// How far either side of `hue` the band reaches before it fades out, in degrees.
    pub width: f32,
    /// How far the band turns, in degrees; positive goes from red toward yellow and green.
    pub shift: f32,
    /// What the band's saturation is multiplied by.
    pub saturation: f32,
}

impl HueShift {
    pub const NONE: Self = Self {
        hue: 0.0,
        width: 30.0,
        shift: 0.0,
        saturation: 1.0,
    };
}

/// A look's parameters. See the module documentation for what each one does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub exposure: f32,
    pub contrast: f32,
    pub pivot: f32,
    pub rolloff: f32,
    pub fade: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub skin_protection: f32,
    pub shadow_saturation: f32,
    pub highlight_saturation: f32,
    pub warmth: f32,
    pub tint: f32,
    pub shadows: Tint,
    pub highlights: Tint,
    pub balance: f32,
    pub hue_shifts: [HueShift; 2],
    pub luma_mix: Tint,
}

impl Look {
    /// Changes nothing: the picture as the phone sent it.
    pub const NEUTRAL: Self = Self {
        exposure: 0.0,
        contrast: 0.0,
        pivot: 0.45,
        rolloff: 0.0,
        fade: 0.0,
        saturation: 1.0,
        vibrance: 0.0,
        skin_protection: 0.0,
        shadow_saturation: 1.0,
        highlight_saturation: 1.0,
        warmth: 0.0,
        tint: 0.0,
        shadows: Tint::NONE,
        highlights: Tint::NONE,
        balance: 0.0,
        hue_shifts: [HueShift::NONE, HueShift::NONE],
        luma_mix: Tint::NONE,
    };
}

/// Natural: what a good camera would have made of the scene. Phone pictures sent as a webcam
/// look flat and a touch cold; this adds the contrast and colour they lack and nothing else.
const NATURAL: Look = Look {
    // A gentle S-curve pivoting a little below middle grey, where faces sit, for depth.
    contrast: 0.16,
    pivot: 0.42,
    // A soft shoulder keeps the lit side of a face from going white.
    rolloff: 0.3,
    // Vibrance rather than saturation: dull walls and clothes come alive, vivid ones stay put,
    // and skin is mostly left alone.
    vibrance: 0.22,
    skin_protection: 0.6,
    // Just enough warmth to undo the blue of a screen-lit room.
    warmth: 0.08,
    // Lifts skin a touch against the background.
    luma_mix: Tint {
        hue: 25.0,
        amount: 0.05,
    },
    ..Look::NEUTRAL
};

/// Crisp: a cool, clean editorial look, like a magazine portrait on white.
const CRISP: Look = Look {
    // Firm contrast for snap, with a shoulder so the cleaner whites stay smooth.
    contrast: 0.26,
    pivot: 0.45,
    rolloff: 0.25,
    // Slightly restrained colour reads as precise rather than loud.
    saturation: 0.95,
    vibrance: 0.08,
    // Faces keep their warmth while everything around them cools. Any cooler and fair skin
    // starts to look chilled.
    skin_protection: 0.85,
    warmth: -0.1,
    // Cool, blue-steel shadows and almost neutral highlights.
    shadows: Tint {
        hue: 215.0,
        amount: 0.012,
    },
    highlights: Tint {
        hue: 200.0,
        amount: 0.004,
    },
    // Clean highlights: colour thins out toward white.
    highlight_saturation: 0.9,
    luma_mix: Tint {
        hue: 25.0,
        amount: 0.05,
    },
    ..Look::NEUTRAL
};

/// Velvet: warm, soft, low-contrast film. Flattering on any face: blemishes and harsh light are
/// softened, and highlights melt away.
const VELVET: Look = Look {
    // Lower contrast and a slight lift of black: the gentleness of print film.
    contrast: -0.12,
    pivot: 0.45,
    fade: 0.035,
    // The strongest shoulder of the set: highlights roll off slowly into cream.
    rolloff: 0.75,
    exposure: 0.05,
    saturation: 0.9,
    vibrance: 0.1,
    skin_protection: 0.5,
    // Warm overall, with warmer highlights than shadows, as daylight film looks.
    warmth: 0.18,
    shadows: Tint {
        hue: 30.0,
        amount: 0.006,
    },
    highlights: Tint {
        hue: 42.0,
        amount: 0.016,
    },
    shadow_saturation: 0.85,
    highlight_saturation: 0.85,
    ..Look::NEUTRAL
};

/// Golden: the hour before sunset. Rich and warm, with amber light on the face and greens turned
/// to olive.
const GOLDEN: Look = Look {
    exposure: 0.05,
    contrast: 0.14,
    pivot: 0.45,
    rolloff: 0.5,
    saturation: 1.06,
    vibrance: 0.14,
    skin_protection: 0.5,
    // Much warmer, mostly in the light: shadows only take a little. More than this turns a
    // white wall or shirt yellow rather than sunlit.
    warmth: 0.26,
    shadows: Tint {
        hue: 15.0,
        amount: 0.008,
    },
    highlights: Tint {
        hue: 38.0,
        amount: 0.02,
    },
    hue_shifts: [
        // Greens toward olive, the way leaves look against a low sun.
        HueShift {
            hue: 115.0,
            width: 30.0,
            shift: -18.0,
            saturation: 0.9,
        },
        // Blues lose their bite so the warmth dominates.
        HueShift {
            hue: 220.0,
            width: 30.0,
            shift: 0.0,
            saturation: 0.8,
        },
    ],
    luma_mix: Tint {
        hue: 35.0,
        amount: 0.06,
    },
    ..Look::NEUTRAL
};

/// Punch: bold and saturated for streaming, where a picture has to hold its own next to games
/// and graphics. Skin is held back so it does not turn orange.
const PUNCH: Look = Look {
    contrast: 0.3,
    pivot: 0.45,
    rolloff: 0.3,
    saturation: 1.12,
    vibrance: 0.3,
    // Medium skin went brick red at 0.75.
    skin_protection: 0.85,
    warmth: 0.03,
    // Deep shadows a little less saturated, so dark clothes do not turn to coloured mud.
    shadow_saturation: 0.92,
    luma_mix: Tint {
        hue: 25.0,
        amount: 0.05,
    },
    ..Look::NEUTRAL
};

/// Nordic: muted, cool and calm. Colour drains from everything but faces; greens drift toward
/// teal and shadows go slate.
const NORDIC: Look = Look {
    contrast: 0.05,
    pivot: 0.45,
    fade: 0.02,
    rolloff: 0.45,
    saturation: 0.62,
    // Skin protection also softens desaturation, so faces keep most of their colour; with
    // less, and a cooler balance, skin turned ashen.
    skin_protection: 0.7,
    warmth: -0.08,
    shadows: Tint {
        hue: 205.0,
        amount: 0.012,
    },
    highlights: Tint {
        hue: 190.0,
        amount: 0.004,
    },
    hue_shifts: [
        HueShift {
            hue: 120.0,
            width: 35.0,
            shift: 25.0,
            saturation: 0.75,
        },
        HueShift::NONE,
    ],
    ..Look::NEUTRAL
};

/// Pastel: light, airy and soft, with powdery colour, lavender shadows and blush highlights.
const PASTEL: Look = Look {
    exposure: 0.15,
    contrast: -0.2,
    pivot: 0.45,
    fade: 0.05,
    rolloff: 0.6,
    saturation: 0.78,
    vibrance: 0.18,
    skin_protection: 0.5,
    warmth: 0.04,
    tint: 0.05,
    shadows: Tint {
        hue: 255.0,
        amount: 0.012,
    },
    highlights: Tint {
        hue: 340.0,
        amount: 0.008,
    },
    shadow_saturation: 0.8,
    ..Look::NEUTRAL
};

/// Cinema: the teal and orange of a modern feature film, done with restraint. Shadows and cool
/// colours lean teal, light leans warm, and skin stays where it was.
const CINEMA: Look = Look {
    contrast: 0.24,
    pivot: 0.42,
    fade: 0.015,
    rolloff: 0.5,
    vibrance: 0.05,
    // The highest protection of the set: the grade happens around the face, not on it.
    skin_protection: 0.85,
    shadows: Tint {
        hue: 188.0,
        amount: 0.03,
    },
    highlights: Tint {
        hue: 32.0,
        amount: 0.018,
    },
    balance: 0.05,
    hue_shifts: [
        // Greens, yellow-greens included, toward teal, and quieter, so foliage reads as a
        // backdrop rather than turning a vivid aqua.
        HueShift {
            hue: 110.0,
            width: 40.0,
            shift: 38.0,
            saturation: 0.75,
        },
        // Blues a little toward teal too, so the whole cool half of the wheel agrees, but not
        // so far that a navy shirt turns petrol.
        HueShift {
            hue: 225.0,
            width: 30.0,
            shift: -15.0,
            saturation: 0.95,
        },
    ],
    // Warm colours a little brighter, cool ones a little darker: separation without colour.
    luma_mix: Tint {
        hue: 30.0,
        amount: 0.08,
    },
    ..Look::NEUTRAL
};

/// Dusk: moody and low-key. Darker, with lifted blue-black shadows and quiet colour; the face
/// is kept up so the mood never costs the person.
const DUSK: Look = Look {
    // A third of a stop down and firm contrast pushes the room into shadow...
    exposure: -0.35,
    contrast: 0.26,
    pivot: 0.5,
    // ...but the blacks are lifted and turned blue-grey, like evening light, never crushed.
    fade: 0.06,
    rolloff: 0.6,
    saturation: 0.76,
    // High, so the mood stays in the room: deep skin went ashen at 0.65.
    skin_protection: 0.8,
    warmth: -0.05,
    // Slate-teal rather than blue: blue shadows mixed with deep skin read as purple.
    shadows: Tint {
        hue: 200.0,
        amount: 0.016,
    },
    // A warm key on the face against the cool room.
    highlights: Tint {
        hue: 35.0,
        amount: 0.012,
    },
    // Quieter shadows, but not so quiet that deep skin, which lives down here, turns grey.
    shadow_saturation: 0.88,
    highlight_saturation: 0.8,
    // The face is lifted back out of the gloom.
    luma_mix: Tint {
        hue: 25.0,
        amount: 0.08,
    },
    ..Look::NEUTRAL
};

/// Vintage: a faded colour print from a family album. Milky blacks, creamy whites, cyan-green
/// shadows and gently yellowed light.
const VINTAGE: Look = Look {
    contrast: -0.05,
    pivot: 0.45,
    fade: 0.075,
    rolloff: 0.7,
    saturation: 0.75,
    skin_protection: 0.4,
    // The yellowing is in the highlights; overall warmth on top of the cyan shadows turned
    // dark hair olive.
    warmth: 0.08,
    // Cyan rather than green, and lighter than first tried: dark hair went olive.
    shadows: Tint {
        hue: 185.0,
        amount: 0.014,
    },
    highlights: Tint {
        hue: 48.0,
        amount: 0.026,
    },
    highlight_saturation: 0.8,
    hue_shifts: [
        // Reds drift to orange, as old dyes do.
        HueShift {
            hue: 0.0,
            width: 30.0,
            shift: 8.0,
            saturation: 0.9,
        },
        // Blues fade toward cyan.
        HueShift {
            hue: 230.0,
            width: 30.0,
            shift: -15.0,
            saturation: 0.75,
        },
    ],
    ..Look::NEUTRAL
};

/// Silver: contrasty black and white with the faint cool-to-warm tone of a selenium print.
const SILVER: Look = Look {
    // Strong contrast pivoting at middle grey, so well-lit skin stays a tone and does not
    // flare to paper white.
    contrast: 0.3,
    pivot: 0.5,
    fade: 0.01,
    rolloff: 0.4,
    // No colour, and nothing protected from losing it.
    saturation: 0.0,
    // A light orange filter: skin glows a little and blue skies and shirts go deeper, as on
    // film. Stronger filters bleached fair and medium skin.
    luma_mix: Tint {
        hue: 30.0,
        amount: 0.12,
    },
    // The tone: cool, slightly purple blacks and warm paper whites.
    shadows: Tint {
        hue: 270.0,
        amount: 0.007,
    },
    highlights: Tint {
        hue: 40.0,
        amount: 0.008,
    },
    ..Look::NEUTRAL
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_have_unique_keys_that_round_trip() {
        for (index, look) in LookId::ALL.iter().enumerate() {
            assert_eq!(LookId::from_key(look.key()), Some(*look));
            for other in &LookId::ALL[index + 1..] {
                assert_ne!(look.key(), other.key());
            }
        }
        assert_eq!(LookId::ALL[0], LookId::None);
        assert_eq!(*LookId::None.look(), Look::NEUTRAL);
    }

    #[test]
    fn looks_are_saved_by_key_and_unknown_keys_fall_back() {
        assert_eq!(
            serde_json::to_string(&LookId::Cinema).unwrap(),
            "\"cinema\""
        );
        assert_eq!(
            serde_json::from_str::<LookId>("\"velvet\"").unwrap(),
            LookId::Velvet
        );
        assert_eq!(
            serde_json::from_str::<LookId>("\"retired-look\"").unwrap(),
            LookId::None
        );
    }

    #[test]
    fn every_stylised_look_differs_from_the_others() {
        for (index, look) in LookId::ALL.iter().enumerate() {
            for other in &LookId::ALL[index + 1..] {
                assert_ne!(look.look(), other.look(), "{look:?} and {other:?}");
            }
        }
    }
}
