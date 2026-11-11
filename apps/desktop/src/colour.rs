//! The inspector's Colour section: the looks as tiles that preview themselves, how strongly the
//! chosen look applies, and the basic adjustments that sit under any look.
//!
//! The grade itself lives in the engine (`paircam_engine::colour`), which applies it to the
//! virtual camera and the preview alike; these controls only read and change the settings,
//! which are saved with the other picture settings.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{Selectable as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, FontWeight, ParentElement as _, SharedString,
    Styled as _, Subscription, Window, div, px,
};
use paircam_engine::colour::{ColourSettings, swatches};
use paircam_engine::looks::LookId;
use paircam_ui::tokens::{font, space};
use paircam_ui::{color, palette, section_label, swatch_strip};

use crate::app::PairCamApp;
use crate::controls::valued_field;
use crate::text;

/// How many look tiles sit side by side.
const TILE_COLUMNS: u16 = 3;
/// The sliders run in whole percent.
const PERCENT: f32 = 100.0;

/// One of the basic adjustments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adjustment {
    Exposure,
    Contrast,
    Saturation,
    Temperature,
    Tint,
}

impl Adjustment {
    const ALL: [Self; 5] = [
        Self::Exposure,
        Self::Contrast,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
    ];

    fn label(self) -> SharedString {
        match self {
            Self::Exposure => text!("colour.exposure"),
            Self::Contrast => text!("colour.contrast"),
            Self::Saturation => text!("colour.saturation"),
            Self::Temperature => text!("colour.temperature"),
            Self::Tint => text!("colour.tint"),
        }
    }

    fn value(self, colour: &ColourSettings) -> f32 {
        match self {
            Self::Exposure => colour.exposure,
            Self::Contrast => colour.contrast,
            Self::Saturation => colour.saturation,
            Self::Temperature => colour.temperature,
            Self::Tint => colour.tint,
        }
    }

    fn set(self, colour: &mut ColourSettings, value: f32) {
        let slot = match self {
            Self::Exposure => &mut colour.exposure,
            Self::Contrast => &mut colour.contrast,
            Self::Saturation => &mut colour.saturation,
            Self::Temperature => &mut colour.temperature,
            Self::Tint => &mut colour.tint,
        };
        *slot = value;
    }
}

/// A look's name in the interface language.
pub fn look_name(look: LookId) -> SharedString {
    match look {
        LookId::None => text!("look.none"),
        LookId::Natural => text!("look.natural"),
        LookId::Crisp => text!("look.crisp"),
        LookId::Velvet => text!("look.velvet"),
        LookId::Golden => text!("look.golden"),
        LookId::Punch => text!("look.punch"),
        LookId::Nordic => text!("look.nordic"),
        LookId::Pastel => text!("look.pastel"),
        LookId::Cinema => text!("look.cinema"),
        LookId::Dusk => text!("look.dusk"),
        LookId::Vintage => text!("look.vintage"),
        LookId::Silver => text!("look.silver"),
    }
}

/// A look's swatch strip as token-style colours (`0xRRGGBBAA`), graded by its own tables.
pub fn look_swatches(look: LookId) -> Vec<u32> {
    swatches(look)
        .iter()
        .map(|[r, g, b]| u32::from(*r) << 24 | u32::from(*g) << 16 | u32::from(*b) << 8 | 0xFF)
        .collect()
}

/// A selectable tile for one look: its swatch strip above its name.
pub fn look_tile(
    id: impl Into<gpui_kit::ElementId>,
    look: LookId,
    strip: &[u32],
    selected: bool,
    cx: &App,
) -> Button {
    Button::new(id)
        .outline()
        .selected(selected)
        .h_auto()
        .w_full()
        .p(px(space::S_1))
        // The chosen look is ringed in the primary colour, readable at a glance.
        .when(selected, |tile| {
            tile.border_color(color(palette(cx).primary))
        })
        .accessibility_label(look_name(look))
        .child(
            div()
                .v_flex()
                .w_full()
                .min_w_0()
                .gap(px(space::S_1))
                .child(swatch_strip(strip.iter().copied(), cx))
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(font::size::XS))
                        .when(selected, |name| name.font_weight(FontWeight::MEDIUM))
                        .child(look_name(look)),
                ),
        )
}

/// The sliders, which keep state of their own between renders, and whether the adjustments
/// are showing.
pub struct ColourControls {
    intensity: Entity<SliderState>,
    adjustments: Vec<(Adjustment, Entity<SliderState>)>,
    /// Each look's swatch strip, in [`LookId::ALL`] order. Computed once: the looks are fixed.
    swatches: Vec<Vec<u32>>,
    adjust_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl ColourControls {
    pub fn new(colour: &ColourSettings, cx: &mut Context<PairCamApp>) -> Self {
        let intensity = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(PERCENT)
                .step(1.0)
                .default_value(colour.intensity * PERCENT)
        });
        let mut subscriptions =
            vec![
                cx.subscribe(&intensity, |this, _, event: &SliderEvent, cx| {
                    if let SliderEvent::Change(value) = event {
                        let intensity = value.start() / PERCENT;
                        this.change_picture(|settings| settings.colour.intensity = intensity, cx);
                    }
                }),
            ];
        let adjustments = Adjustment::ALL
            .iter()
            .map(|adjustment| {
                let adjustment = *adjustment;
                let slider = cx.new(|_| {
                    SliderState::new()
                        .min(-PERCENT)
                        .max(PERCENT)
                        .step(1.0)
                        .default_value(adjustment.value(colour) * PERCENT)
                });
                subscriptions.push(cx.subscribe(
                    &slider,
                    move |this, _, event: &SliderEvent, cx| {
                        if let SliderEvent::Change(value) = event {
                            let value = value.start() / PERCENT;
                            this.change_picture(
                                |settings| adjustment.set(&mut settings.colour, value),
                                cx,
                            );
                        }
                    },
                ));
                (adjustment, slider)
            })
            .collect();
        Self {
            intensity,
            adjustments,
            swatches: LookId::ALL
                .iter()
                .map(|look| look_swatches(*look))
                .collect(),
            // Open from the start when something is already adjusted, so it is never hidden.
            adjust_open: colour.is_adjusted(),
            _subscriptions: subscriptions,
        }
    }

    /// Moves the sliders to settings changed elsewhere: Reset, or another window.
    pub fn sync(&self, colour: &ColourSettings, window: &mut Window, cx: &mut App) {
        let mut sliders = vec![(&self.intensity, colour.intensity)];
        sliders.extend(
            self.adjustments
                .iter()
                .map(|(adjustment, slider)| (slider, adjustment.value(colour))),
        );
        for (slider, value) in sliders {
            let value = value * PERCENT;
            if (slider.read(cx).value().start() - value).abs() > 0.5 {
                slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
    }

    pub fn toggle_adjust(&mut self) {
        self.adjust_open = !self.adjust_open;
    }

    /// The Colour section.
    pub fn render(&self, colour: &ColourSettings, cx: &mut Context<PairCamApp>) -> Div {
        let changed = *colour != ColourSettings::default();
        let tiles =
            LookId::ALL
                .iter()
                .zip(&self.swatches)
                .enumerate()
                .map(|(index, (look, strip))| {
                    let look = *look;
                    look_tile(("colour-look", index), look, strip, colour.look == look, cx)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_picture(|settings| settings.colour.look = look, cx);
                        }))
                });

        let adjust = Button::new("colour-adjust")
            .ghost()
            .xsmall()
            .icon(if self.adjust_open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .label(text!("colour.adjust"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.colour_controls_mut().toggle_adjust();
                cx.notify();
            }));
        let sliders = div()
            .v_flex()
            .w_full()
            .gap_3()
            .pt_1()
            .children(self.adjustments.iter().map(|(adjustment, slider)| {
                valued_field(adjustment.label(), signed(adjustment.value(colour)), cx)
                    .child(Slider::new(slider))
            }));

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(text!("colour.title"), cx))
                    .when(changed, |header| {
                        header.child(
                            Button::new("colour-reset")
                                .ghost()
                                .xsmall()
                                .label(text!("colour.reset"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.change_picture(
                                        |settings| settings.colour = ColourSettings::default(),
                                        cx,
                                    );
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(TILE_COLUMNS)
                    .gap(px(space::S_1_5))
                    .children(tiles),
            )
            // With no look there is nothing to blend, so the slider only appears with one.
            .when(colour.look != LookId::None, |section| {
                section.child(
                    valued_field(
                        text!("colour.intensity"),
                        format!("{:.0}%", colour.intensity * PERCENT),
                        cx,
                    )
                    .child(Slider::new(&self.intensity)),
                )
            })
            .child(
                Collapsible::new()
                    .motion_id("colour-adjustments")
                    .w_full()
                    .open(self.adjust_open)
                    .child(div().h_flex().child(adjust))
                    .content(sliders),
            )
    }
}

/// An adjustment as a whole signed percentage: "+12", "-30", "0".
fn signed(value: f32) -> String {
    let percent = (value * PERCENT).round();
    if percent == 0.0 {
        "0".to_owned()
    } else {
        format!("{percent:+.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjustments_read_back_what_they_set() {
        let mut colour = ColourSettings::default();
        for (index, adjustment) in Adjustment::ALL.iter().enumerate() {
            adjustment.set(&mut colour, index as f32 / 10.0 - 0.2);
        }
        for (index, adjustment) in Adjustment::ALL.iter().enumerate() {
            assert_eq!(adjustment.value(&colour), index as f32 / 10.0 - 0.2);
        }
    }

    #[test]
    fn signed_values_read_naturally() {
        assert_eq!(signed(0.0), "0");
        assert_eq!(signed(-0.001), "0");
        assert_eq!(signed(0.12), "+12");
        assert_eq!(signed(-0.3), "-30");
    }

    #[test]
    fn every_look_has_a_strip_of_opaque_colours() {
        for look in LookId::ALL {
            let strip = look_swatches(look);
            assert_eq!(strip.len(), 6);
            assert!(strip.iter().all(|colour| colour & 0xFF == 0xFF));
        }
    }
}
