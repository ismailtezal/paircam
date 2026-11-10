# Kadraj

Phone-as-webcam: a phone app streams its camera to a desktop app, which exposes it to Windows as a
real webcam. Open source (MIT); the GitHub repository is private until the first public release.
Read `docs/ARCHITECTURE.md`, `docs/PROTOCOL.md` and `docs/ROADMAP.md` before changing anything.

## The bar

The goal is to beat DroidCam, Camo and Iriun on quality, delay and polish, with nothing paid.
A feature is done when it is better than the best existing app's version of it, not when it works.
Before building a feature, look at how the best app does it and follow that interaction model.

## How we work

- **Foundation first.** Build shared pieces (design tokens, components, protocol, diagnostics)
  before screens and features, then build features on top of them.
- **Commits:** one logical change per commit, clear messages, so any good state can be restored.
- **Never `git push` unless the owner asks in that message.**
- **Ship when done.** Keep the cheap gates (type-check, lint, build, unit tests); skip open-ended
  polishing and long test sweeps unless asked.
- **Never print secrets** (signing keys, tokens, `.env` values) in output or commit them.

## Design system

- One token source in `packages/tokens` generates the styles for the desktop app (CSS variables
  for Tailwind) and the phone app (a typed theme object). Never hard-code a colour, radius,
  spacing or duration in a component.
- Desktop UI is built on shadcn/ui and Radix. Do not hand-roll a component that exists there.
- Reuse the app's own components; a new one-off widget needs a reason.
- The look: calm, dark by default, built around the camera preview, with carefully made small
  elements. Not plain shadcn and not a copy of another app.
- Motion follows Emil Kowalski's rules: use the `animate` / `emil-design-eng` skills on web and
  `animate-expo` on the phone.

## Languages

Kadraj is a global product. English is the source and fallback language, and the apps follow the
device language. Every user-visible string lives in a translation catalog so languages can be
added without touching components (Turkish is the first additional one); no literals in
components. Code, comments, commit messages and docs are in English.

## Code

- TypeScript is strict everywhere. Write formatted, multi-line code that looks like its
  neighbours.
- The phone-to-desktop protocol is versioned and specified in `docs/PROTOCOL.md`. Change the spec
  and the shared test vectors in the same commit as the code.
- Camera capture and encoding are native code (Swift, Kotlin). React Native never touches frames.
- **Licences:** copy code only from MIT, BSD or Apache projects, and keep their notices. GPL
  projects (Mobcam, the DroidCam plugins, obs-ios-camera-source, Webcamoid, Android Webcam
  Project) may be read for understanding but never copied. See `docs/PRIOR-ART.md`.
