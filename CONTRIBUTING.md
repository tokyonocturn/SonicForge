# Contributing to SonicForge

Thanks for helping build a better open-source audio processor. This guide describes how to
get a working build, what the project expects from a change, and how to report the audio
problems that are hardest to reproduce.

## Getting set up

```powershell
rustup target add x86_64-pc-windows-msvc
cargo test            # platform independent engine tests
cargo build --release
```

You need the MSVC build tools and the Windows 10/11 SDK. The GUI needs a GPU with working
OpenGL/Direct3D drivers; on machines without one, use `sonicforge` and the text
configuration instead.

### Installing a development build safely

Never test a development APO on your only audio device without a fallback:

1. Keep a second output device (USB headset, HDMI) that SonicForge is **not** enabled on.
2. Use `sonicforge apo enable --device "<name>"` for a single endpoint while developing.
3. If audio stops working, `sonicforge apo disable --all` removes SonicForge from the
   audio stack. It also works from a terminal while audio is broken.

## Adding a filter or command

1. Implement the design in `crates/sf-core/src/dsp/filters.rs`.
2. Add a parser case in `crates/sf-core/src/config/filter.rs` if the configuration
   language needs one.
3. Wire it into `crates/sf-core/src/engine/compile.rs`.
4. Add tests that verify **behaviour**, not implementation details: measure the magnitude
   response at meaningful frequencies, check stability, and check parsing of both valid
   and malformed input.
5. Update `docs/CONFIGURATION.md` and, if the reference project behaves differently,
   `docs/COMPATIBILITY.md`.
6. Add a `CHANGELOG.md` entry.

Filter tests must be analytic where possible. For example a peaking filter with +6 dB gain
must measure 6 dB ± 0.02 dB at its centre frequency; do not assert on coefficient values.

## Code rules

* **No allocation, locking, logging or file IO in the audio thread.** The real-time path
  is `Pipeline::process`. Precompute in the compiler.
* **No panics on user input.** Configuration text is untrusted; every parse error becomes
  a `Diagnostic` with a line number.
* **Denormals:** filter state is sanitised after each block.
* **No silent failures.** An unsupported feature must produce an error or warning
  diagnostic. Never accept a command and do nothing.
* Formatting: `cargo fmt`, and `cargo clippy --all-targets -- -D warnings` must pass.
* Prefer small, focused commits. Each commit must build and pass tests on its own.

## Testing

```powershell
cargo test --workspace              # unit + integration tests
cargo test -p sf-core --release     # DSP tests in release mode
```

Please include a test with every bug fix. For DSP bugs, a test that reproduces the
measurable defect (frequency response, impulse response, sample values) is worth more than
a test that just calls the function.

## Reporting audio problems

Include:

* Windows version and build (`winver`).
* Device name, sample rate and channel count (`sonicforge devices`).
* The configuration file that triggers the problem.
* `sonicforge diagnose --bundle` output (it includes APO registration state, engine
  diagnostics and recent logs, with no personal data).
* Whether the problem happens with `Preamp: 0 dB` only, which isolates the audio path from
  the configuration.

## Licence

By contributing you agree that your contribution is licensed under GPL-2.0-or-later, the
licence of this project. Do not paste code from other projects unless its licence is
compatible and the attribution is added to `NOTICE`.
