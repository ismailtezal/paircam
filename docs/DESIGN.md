# Design direction

PairCam should feel like Apple and Vercel made it together: Vercel's precision and restraint,
Apple's clarity, depth and motion. It is built on shadcn-style components, but it must never look
like default shadcn.

## Principles

1. **The picture is the product.** The camera preview is the largest, brightest thing on every
   screen. Controls sit around it or over it and get out of the way.
2. **Quiet until needed.** One primary action per screen. Secondary controls are low-contrast
   until hovered, focused or relevant.
3. **Precise, not decorative.** No gradients, glows or illustrations for their own sake. Quality
   shows in spacing, alignment, type and motion.
4. **State is always visible.** Connected or not, resolution, frame rate and delay are readable at
   a glance, in numbers, without opening a panel.
5. **Fast feels better than fancy.** Every interaction responds within a frame; animation never
   makes the user wait.

## From Vercel

- **Near-monochrome palette.** True black and near-white surfaces, a ramp of neutral greys, and a
  single accent used sparingly. Colour otherwise means status: live, warning, error.
- **Hairline borders instead of shadows** to separate surfaces. Shadows only on floating layers.
- **Geist and Geist Mono** on the desktop. Tight tracking on headings; tabular mono numerals for
  every live number (fps, Mbps, ms).
- **A strict spacing grid** (4 px base) and a short type scale. Small, dense, well-aligned
  controls.
- **High contrast text** on calm surfaces; muted text for anything secondary.

## From Apple

- **Deference to content.** Chrome is minimal and recedes behind the preview.
- **Depth through material.** Controls that float over the preview use a translucent, blurred
  surface so the picture stays visible beneath them.
- **Spring motion** that can be interrupted and reversed mid-flight. Things move from where the
  user acted. Durations are short (roughly 150 to 350 ms) and nothing uses linear easing.
- **Continuous corners** and one radius family, with inner radii derived from outer ones.
- **Platform-native feel on the phone:** the system font (SF Pro), SF-style icon weight, haptics
  on meaningful actions, large touch targets, safe areas respected, reduced-motion honoured.

## Tokens

All values live in `design/tokens` and are generated into both apps. Components reference
semantic names, never raw values.

| Group | Examples |
| --- | --- |
| Colour | `background`, `surface`, `surface-raised`, `border`, `text`, `text-muted`, `accent`, `live`, `warning`, `danger` |
| Type | family, a 6-step size scale, weights, tracking, mono numerals |
| Space | a 4 px scale |
| Radius | `sm`, `md`, `lg`, `full`, and the rule for nested radii |
| Elevation | hairline, raised, floating, material (blur and tint) |
| Motion | spring presets (`snappy`, `smooth`, `gentle`), durations, the standard ease-out curve |

Dark is the default theme; light is a full peer, not an afterthought.

## Components

- **Desktop:** GPUI Kit components, restyled through the generated PairCam theme and wrapped
  where the default behaviour or look is not right. Wrappers live in the shared UI crate.
- **iPhone:** React Native Reusables as the shadcn-style base, restyled with the same tokens.
- Before any screen is built, a gallery of every component in every state (default, hover,
  pressed, focused, disabled, loading) in both themes is reviewed and approved.

## Review checklist for a screen

- Is the preview the hero, and is there exactly one primary action?
- Does every colour, size, radius and duration come from a token?
- Do hover, focus, pressed, disabled, loading, empty and error states all exist?
- Is it fully usable with the keyboard on desktop and with one hand on the phone?
- Does every animation start from the user's action, use a spring or ease-out, and stay
  interruptible?
- Would it still look right placed next to a screen from Vercel's dashboard or Apple's Camera app?
