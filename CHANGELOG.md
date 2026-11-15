# Changelog

## 0.1.0 - unreleased

### Added

* Core engine: biquads, 6/12 dB and slope-configurable shelves, arbitrary IIR, graphic EQ
  with iterative gain refinement, uniformly partitioned FFT convolution, fractional delay,
  channel routing with virtual channels, preamp and gain stages.
* Equalizer APO configuration language: all filter types, `GraphicEQ`, `Convolution`,
  `Copy`, `Channel`, `Delay`, `Device`, `Stage`, `Include`, `If`/`ElseIf`/`Else`/`EndIf`,
  `Eval`, inline backtick expressions, a full expression language with regex support.
* Windows APO: in-proc COM server with explicit vtables, real-time safe processing,
  off-thread reload watcher, shared-memory control block for bypass and master trim.
* Platform layer: WASAPI endpoint enumeration, per-endpoint APO registration with backup
  and restore of the previous effect chain, rolling logs, configuration layout.
* Editor: response graph computed by the real engine, draggable bands, double-click to
  add, band table, presets, raw configuration editing, live diagnostics, device list,
  bypass and master preamp.
* Tray application: open editor, bypass, reload, log folder, exit.
* Installer: `sf-setup install/uninstall/status` plus an Inno Setup script.
* CLI: devices, validate, render, apo install/enable/disable/status/self-test, config
  path/show/import/export, presets, diagnose with bundle output.
* CI: build and test on Windows and Linux, clippy on the core crate, release packaging.

### Impulse responses

* `Convolution:` loads WAVE, FLAC, Ogg/Vorbis, raw float32/float64 and text
  impulse responses through `sf-core::ir`, which stays free of Windows APIs.
  Cross-format tests store one synthetic IR in every container and prove the
  measured frequency response matches the WAVE original: sample exact for
  lossless formats, under 1 dB in band and 0.3 dB RMS for Vorbis.
* Raw float and text files can declare their sample rate; an IR whose rate does
  not match the device is still rejected with a diagnostic.

### Loudness correction

* `LoudnessCorrection: State <0|1> ReferenceLevel <dB> [ReferenceOffset <dB>]
  [Attenuation <0..1>]` is implemented instead of warned about. It applies a
  75 Hz low shelf, a 10 kHz high shelf and a matching headroom cut derived from
  the distance below the reference volume, scaled by `Attenuation`.
* The listening volume comes from the default playback endpoint (read off the
  audio thread) and travels to the compiler in `CompileOptions::volume_db`.
  Tests cover the gain at each volume, the neutral point, offsets, attenuation
  and malformed input.

### Not implemented

* VST2 plugin hosting (the configuration command is rejected with a diagnostic).


All notable changes to SonicForge are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Workspace scaffolding: `sf-core`, `sf-win`, `sf-apo`, `sf-gui`, `sf-tray`, `sf-cli`,
  `sf-setup`.

### Core engine (`sf-core`)
- Biquad implementation (transposed direct form II) with analytic magnitude response and
  stability checking.
- Audio EQ Cookbook filter designs: peaking, notch, low/high pass, band pass, all pass,
  low/high shelf with Q or slope, and fixed 6 dB/oct and 12 dB/oct shelves.
- Bandwidth-in-octaves to Q conversion.
- Generic arbitrary-order IIR filter with its own state.
- Graphic EQ design: one peaking band per control point with iterative gain refinement so
  the summed response tracks the interpolated target curve.
- Radix-2 FFT, real-input FFT with correct DC/Nyquist packing, verified against a naive
  DFT.
- Uniformly partitioned FFT convolution with per-partition delayed spectra, impulse
  responses longer than one partition, arbitrary host block sizes and a documented
  one-block latency.
- Fractional delay lines, channel copy/mix matrix with simultaneous-read semantics.
- Ordered processing pipeline with per-channel filter state, mask based channel
  selection, reset, latency reporting and frequency response calculation.
- RIFF/WAVE decoder (PCM 8/16/24/32-bit, IEEE float 32/64-bit, `WAVE_FORMAT_EXTENSIBLE`)
  and a float WAVE writer.
- Equalizer APO configuration parser and compiler: `Preamp`, `Filter` (all types plus
  `IIR`), `GraphicEQ`, `Convolution`, `Delay`, `Copy`, `Channel`, `Device`, `Stage`,
  `Include` (with cycle detection), `If`/`ElseIf`/`Else`/`EndIf`, `Eval` and inline
  backtick expressions.
- Expression evaluator with numbers, strings, booleans, arrays, variables, assignment,
  arithmetic/comparison/bitwise operators, ternaries, regex helpers and the documented
  function set.
- Virtual channel allocation for `Copy:` targets such as `L2`, as used by the reference
  project's selective-delay example.
- Editable preset model and six built-in presets, with configuration text export.
- Shared memory control block for live bypass, master preamp and configuration
  generation, with an in-process fallback for tests.
- 120 unit tests covering filters, FFT, convolution, delay, mixing, parsing, compilation
  and diagnostics.

### Fixed
- Real FFT inverse transform now pairs the DC bin with the separate Nyquist bin instead
  of bin 0, restoring exact round trips.
- Partitioned convolution now multiplies each impulse-response partition with the input
  spectrum from the correct earlier block; previously every partition contributed at the
  wrong time offset for impulse responses longer than one block.
- Fractional delay interpolation now reads towards older samples; delays were shifted by
  one sample.
- Generic `IIR` filters are compiled instead of being skipped as "no filter type".
- `Copy:` factors written in dB (`R+-6dB*C`) are converted as dB values, not as negative
  linear gains.

## [0.1.0] - TBD

First public release. Not yet published.
