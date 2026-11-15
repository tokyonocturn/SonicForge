# SonicForge

**A modern, fully open-source system-wide audio processor for Windows 10 and 11 (x64).**

SonicForge is an independent reimplementation of the ideas behind Equalizer APO, written
from scratch in Rust. It installs an Audio Processing Object (APO) into the Windows audio
stack so that **every** application's audio is processed in real time, and it ships a
modern editor with draggable EQ points, a live frequency-response graph, presets,
convolution and per-device configurations.

SonicForge is free. There is no subscription, no trial, no licence key and no artificial
feature limit.

---

## Status

SonicForge is under active development. This table is the honest state of the project; it
is updated with every release.

| Area | State |
| --- | --- |
| DSP core (biquads, shelves, IIR, graphic EQ, convolution, delay, channel mix) | **Working**, 145 unit tests |
| Equalizer APO configuration parser + compiler | **Working** (`Preamp`, `Filter`, `GraphicEQ`, `Convolution`, `Delay`, `Copy`, `Channel`, `Device`, `Stage`, `Include`, `If`/`ElseIf`/`Else`/`EndIf`, `Eval`, inline expressions) |
| Expression language | **Working** (arithmetic, comparison, ternary, arrays, string ops, regex, registry reads) |
| WAVE impulse-response loading | **Working** (PCM 8/16/24/32, float 32/64, extensible) |
| System-wide APO | **Implemented** (`IAudioProcessingObject` in-proc COM server, per-endpoint registration) |
| GUI editor | **Built and launchable** (device list, live graph, draggable bands, presets, text editor) |
| Command line tools | **Working** (devices, validate, render, install, diagnose) |
| Installer | **Working** (`sf-setup`, plus `packaging/installer.iss` for an Inno Setup build) |
| Tray application | **Working** (bypass, reload, open editor, exit) |
| VST2 plugin hosting | **Not implemented** — the configuration command is rejected with an explicit diagnostic instead of silently doing nothing |
| `LoudnessCorrection` | **Working** (volume-dependent 75 Hz / 10 kHz shelves plus headroom, tested at each reference volume) |
| Impulse responses | **Working** for WAVE, FLAC, Ogg/Vorbis, raw float32/64 and text, all through `sf-core::ir` with cross-format response tests |

Unsupported features never fail silently: the compiler reports a diagnostic with the file
and line number, and the editor shows it.

---

## How it works

```
 application audio
        │
        ▼
 ┌──────────────────────────────┐
 │ Windows audio engine (audiodg)│
 │  ┌────────────────────────┐  │
 │  │ SonicForge APO (Rust)  │  │   pre-mix stage  → per application stream
 │  │  · config compiler     │  │
 │  │  · DSP pipeline        │  │   post-mix stage → per device stream
 │  │  · convolution engine  │  │
 │  └────────────────────────┘  │
 └──────────────────────────────┘
        │
        ▼
    audio device
```

* `crates/sf-core` — platform independent engine: DSP, configuration language,
  compiler, WAVE IO, shared control block. Contains no Windows API calls in the signal
  path, which is why it is fully unit tested on any host.
* `crates/sf-win` — Windows platform layer: WASAPI endpoint enumeration, APO
  registration, configuration paths, registry helpers.
* `crates/sf-apo` — the in-process COM APO that Windows loads into `audiodg.exe`.
* `crates/sf-gui` — the editor (egui/eframe).
* `crates/sf-tray` — tray icon and quick actions.
* `crates/sf-cli` — `sonicforge` command line tool: devices, validate, render, diagnose.
* `crates/sf-setup` — installer/uninstaller.

### Audio path

The APO runs in the real-time audio thread, so it never allocates, blocks, reads files or
logs while processing. It owns one prebuilt [`Pipeline`] per stream and swaps in a newly
compiled pipeline between blocks when the configuration generation changes. Live
adjustments (bypass, master preamp) travel through a memory-mapped control block that the
audio thread only reads with volatile loads.

Convolution is uniformly partitioned FFT convolution with a 512-sample partition
(10.7 ms at 48 kHz). Latency is reported by the engine and shown in the editor. Filters
alone add zero latency.

---

## Building

Requirements: Rust 1.80+, the MSVC toolchain (Visual Studio Build Tools with the Windows
SDK), and Windows 10/11 x64.

```powershell
cargo build --release
cargo test
cargo clippy --workspace --all-targets -- -D warnings
```

`cargo clippy` with `-D warnings` covers every crate in the workspace; CI runs the same
command on Windows and Linux.

Artefacts:

| File | Purpose |
| --- | --- |
| `target/release/sonicforge_apo.dll` | the audio processing object, installed and registered as `SonicForgeAPO.dll` |
| `target/release/sonicforge.exe` | command line tools |
| `target/release/sonicforge-editor.exe` | the GUI editor |
| `target/release/sonicforge-tray.exe` | tray application |
| `target/release/sf-setup.exe` | installer |

## Installing

```powershell
# from an elevated prompt
.\sf-setup.exe install
```

Or manually, for development:

```powershell
.\sonicforge.exe apo install --dll target\release\sonicforge_apo.dll
.\sonicforge.exe apo enable --all
```

`sonicforge apo status` prints which endpoints are currently processed.
`sonicforge apo disable --all` removes SonicForge from every endpoint and restores the
previous effect chain, which is the supported "unbreak my audio" path.

> **Safety:** SonicForge never deletes another product's APO registration. Enabling
> stores the previous value of the endpoint effect CLSID and disabling restores it.

---

## Configuration

SonicForge reads Equalizer APO style configuration files. The default locations are:

```
%APPDATA%\SonicForge\config\config.txt
%APPDATA%\SonicForge\config\*.txt   (Include: targets)
%APPDATA%\SonicForge\config\*.wav   (Convolution: impulse responses)
```

Independent per-device chains are expressed with `Device:` lines inside the configuration,
matched against the endpoint friendly name.

Example:

```
Preamp: -6 dB
Include: harman.txt

Channel: L R
Filter: ON PK Fc 105 Hz Gain 4.5 dB Q 0.7
Filter: ON PK Fc 3200 Hz Gain -2.0 dB Q 1.4
GraphicEQ: 25 0; 40 2; 63 1; 100 0; 160 0; 250 0; 400 0
Convolution: room.wav
```

### Compatibility with Equalizer APO files

The supported grammar is documented in [docs/CONFIGURATION.md](docs/CONFIGURATION.md).
Known differences are listed in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md). Existing
`config.txt` files can be imported from the editor or with
`sonicforge config import <path>`.

---

## Licence and attribution

SonicForge is licensed under the **GNU General Public License v2.0 or later** — see
[LICENSE](LICENSE).

SonicForge is an independent implementation. It does not contain source code from
Equalizer APO; the configuration format is reimplemented from the publicly documented
behaviour so that existing user configurations keep working. Equalizer APO is a separate
project by Jonas Thedering, licensed under GPL-2.0, and is credited here as the
specification this project follows. See [NOTICE](NOTICE) for details.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports about audio dropouts should include
`sonicforge diagnose --bundle` output.
