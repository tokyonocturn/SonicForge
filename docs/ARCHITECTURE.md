# Architecture

SonicForge is a Windows audio processing object (APO) written in Rust, plus the
tools around it. It is an independent implementation: no Equalizer APO source
code is used, but the configuration language follows the same grammar so
existing files keep working.

```
                       +---------------------------+
   application audio   |  audiodg.exe              |
   -------- stream --->|  +---------------------+  |
                       |  | SonicForgeAPO.dll   |  |
                       |  |  IAudioProcessing-  |  |
                       |  |  ObjectRT::Process  |  |
                       |  +----------+----------+  |
                       +-------------|-------------+
                                     | shared memory control block
                                     |
                       +-------------|-------------+
                       |  sonicforge-editor        |
                       |  sonicforge-tray          |
                       |  sonicforge (CLI)         |
                       +---------------------------+
```

## Crates

| Crate | Responsibility |
| --- | --- |
| `sf-core` | Config parser/compiler and DSP. No Windows dependency, fully unit tested, also builds on Linux so CI can run the DSP suite anywhere. |
| `sf-win` | Windows plumbing: WASAPI endpoint enumeration, APO registration with backup/restore, paths, rolling log. |
| `sf-apo` | The COM server loaded into `audiodg.exe`: three interfaces, a lock-free real-time path, and a watcher that rebuilds the chain. |
| `sf-cli` (`sonicforge`) | Device listing, validation, offline rendering, APO install/enable/disable/status/self-test, preset and diagnostic commands. |
| `sf-gui` (`sonicforge-editor`) | egui editor: response graph, draggable bands, presets, raw config editing, diagnostics. |
| `sf-tray` (`sonicforge-tray`) | Tray icon: open editor, bypass, reload, log folder, exit. |
| `sf-setup` (`sf-setup`) | Installer/uninstaller: file copy, COM registration, uninstall entry, endpoint enablement. |

## Configuration pipeline

1. **Parse.** `config::Document` keeps every line verbatim together with its
   origin, so error messages can point at the offending line.
2. **Compile.** `engine::compile_document` walks the document in file order and
   builds a `Pipeline`. Commands are interpreted where they appear, which is
   what makes `Eval:` variables, `Include:`, `If:`/`Else:`/`EndIf:` and inline
   backtick expressions behave like the reference implementation.
3. **Run.** The compiled pipeline is an ordered list of `Op`s. Each op owns its
   per-channel state, so a rebuild swaps the whole pipeline atomically instead
   of mutating state in place.

Layout and channel handling:

* Channels are planar `Vec<f32>`; a `u64` mask records which channels exist.
* `Channel:` selects a subset; `Copy:` can create virtual channels (`L2`, `R2`)
  beyond the device channel count, which are dropped at the output.
* Everything is compiled for a specific sample rate and channel count, and is
  recompiled when the device format changes.

## Real-time safety

The APO's `Process` callback runs on the audio thread and must never block:

* The active pipeline lives behind a `Mutex` that the audio thread only ever
  touches with `try_lock`. If a rebuild holds it, that block passes through
  untouched instead of glitching or stalling the audio engine.
* No allocation, no logging, no file IO and no COM calls happen in `Process`.
* A watcher thread (started on `Initialize`) polls the configuration file's
  modification time and the control block every 750 ms and rebuilds off-thread.
* Filters are denormal-safe and use `f32` throughout; convolution uses a
  uniformly partitioned FFT with one block of latency.

## Control block

`sf-core::control` maps a small shared-memory page (`Local\SonicForgeControl`)
containing the bypass flag, a global preamp trim, the configuration generation
counter and the active chain description. The editor and tray write it; the APO
reads it once per block. It is an optimisation channel, not the source of truth:
the configuration file remains authoritative.

## Failure handling

* A missing or broken configuration never disables audio: compile errors are
  reported as diagnostics, the affected commands are skipped, and the rest of
  the chain still runs.
* APO registration backs up the previous effect CLSID and `Disable_SysFx` value
  per endpoint and restores them on disable or uninstall.
* The editor writes `config.txt.bak` before overwriting the configuration.
