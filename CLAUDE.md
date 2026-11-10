# PairCam

Phone-as-webcam: an iPhone app streams its camera to a desktop app, which exposes it to Windows as
a real webcam. Open source (MIT); the GitHub repository is private until the first public release.
Read `docs/ARCHITECTURE.md`, `docs/DESIGN.md`, `docs/PROTOCOL.md` and `docs/ROADMAP.md` before
changing anything.

## The bar

The goal is to beat DroidCam, Camo and Iriun on quality, delay and polish, with nothing paid.
A feature is done when it is better than the best existing app's version of it, not when it works.
Before building a feature, look at how the best app does it and follow that interaction model.

## Scope

- **Now:** the iPhone app and the Windows 11 desktop app.
- **Later:** Android. Do not add Android code, dependencies or abstractions for it yet; just avoid
  choices in the React Native layer that would block it.

## How we work

- **Foundation first.** Build shared pieces (design tokens, components, protocol, diagnostics)
  before screens and features, then build features on top of them.
- **Commits:** one logical change per commit, clear messages, so any good state can be restored.
- **Never `git push` unless the owner asks in that message.**
- **Ship when done.** Keep the cheap gates (type-check, lint, build, unit tests); skip open-ended
  polishing and long test sweeps unless asked.
- **Never print secrets** (signing keys, tokens, `.env` values) in output or commit them.

## Stack

- **Desktop:** Rust and GPUI. Not Tauri, not Electron, no web view. Components come from GPUI Kit
  (`gpui-kit`, the shadcn-based component system for GPUI) under the PairCam theme.
- **iPhone:** React Native (Expo, development builds) for the interface, with components in the
  shadcn style (React Native Reusables as the base). The camera engine is a native Swift module.
- **Virtual camera:** C++ Media Foundation source for Windows 11.

## Design system

The full direction is in `docs/DESIGN.md`. In short: it should feel like Apple and Vercel made it
together. Customised shadcn, never default shadcn.

- One token source in `design/tokens` generates the desktop theme (a GPUI Kit theme) and the
  phone theme (a typed TypeScript object). Never hard-code a colour, radius, spacing, font size or
  duration in a component.
- Use the component library first; do not hand-roll a component it already has. A new one-off
  widget needs a reason, and then it lives in the shared component crate or folder.
- The camera preview is the hero of every screen; everything else stays quiet around it.
- Motion follows Emil Kowalski's and Apple's rules: springs, interruptible, fast. Use the
  `emil-design-eng` and `apple-design` skills for decisions, and `animate-expo` on the phone.
- Every new screen is checked against `docs/DESIGN.md` before it is called done.

## Languages

PairCam is a global product. English is the source and fallback language, and the apps follow the
device language. Every user-visible string lives in a translation catalog so languages can be
added without touching components (Turkish is the first additional one); no literals in
components. Code, comments, commit messages and docs are in English.

## Code

- Rust: `cargo fmt` and `cargo clippy` clean. TypeScript: strict. Write formatted, multi-line
  code that looks like its neighbours.
- The phone-to-desktop protocol is versioned and specified in `docs/PROTOCOL.md`. Change the spec
  and the shared test vectors in the same commit as the code.
- Camera capture and encoding are native Swift. React Native never touches frames.
- **Licences:** copy code only from MIT, BSD or Apache projects, and keep their notices. GPL
  projects (Mobcam, the DroidCam plugins, obs-ios-camera-source, Webcamoid, Android Webcam
  Project) may be read for understanding but never copied. See `docs/PRIOR-ART.md`.
