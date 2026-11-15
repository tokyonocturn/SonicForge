# Compatibility with Equalizer APO

SonicForge reads the same configuration language as Equalizer APO 1.4.x so an
existing `config.txt` can be pointed at it unchanged. The table below records
what is implemented, what differs, and what is deliberately unsupported — this
is the honest state of the compatibility layer, not a wish list.

## Commands

| Command | Status |
| --- | --- |
| `Preamp: <n> dB` | Supported |
| `Filter: ON/OFF <type> ...` | Supported for all types below |
| `Filter <n>: ...` | Supported (named filters) |
| `GraphicEQ: <f> <db>; ...` | Supported, log-frequency interpolation, flat outside range |
| `Convolution: <file>` | Supported for WAVE, FLAC, Ogg/Vorbis, raw float32/float64 and text impulse responses |
| `Copy: <src> <dst> [gain]` | Supported, including virtual channels and `+-6dB` forms |
| `Delay: <ms>` / `Delay: <idx> <ms>` | Supported, fractional delay interpolated |
| `Channel: <spec>` / `Channel: <spec> !` | Supported, with `!` inversion |
| `Device: <patterns>` | Supported (matched against the endpoint friendly name) |
| `Stage: pre-mix / post-mix` | Supported |
| `Include: <file>` | Supported |
| `If: <expr>` / `ElseIf` / `Else` / `EndIf` | Supported |
| `Eval: <var> = <expr>` | Supported |
| `Filter: ON IIR Order <n> Coefficients ... ` | Supported |
| `LoudnessCorrection: State .. ReferenceLevel ..` | Supported, see below |
| `VSTPlugin:` | **Not implemented** — reported as an error; see below |

Every command the parser recognises is either applied or produces an error
diagnostic with its line number. Unknown command names produce a warning instead
of being dropped silently, and malformed parameters are errors.

## Filter types

PK/PEQ/Modal, LP/LPQ, HP/HPQ, BP, LS/HS (6 dB and 12 dB, plus `LSC`/`HSC` with
a slope in dB/oct), NO (notch), AP (all-pass), and IIR with arbitrary
coefficients. Filter parameter parsing accepts the same unit suffixes
(`Hz`, `kHz`, `dB`, `Q`, `BW`, `S`) as the reference.

## Impulse responses

`Convolution:` accepts:

| Format | Detection | Notes |
| --- | --- | --- |
| WAVE | `RIFF/WAVE` header | PCM 8/16/24/32, float 32/64, `WAVE_FORMAT_EXTENSIBLE` |
| FLAC | `fLaC` magic | any bit depth the decoder supports, converted to float |
| Ogg/Vorbis | `OggS` magic | decoded to 48 kHz-native float; lossy |
| raw float32 | `.f32`, `.raw`, `.pcm` extension | little endian, mono, assumed 48 kHz |
| raw float64 | `.f64` extension | little endian, mono, assumed 48 kHz |
| text | `.txt`, `.csv`, `.ir` extension | one sample per token, `#` comments, optional `sample_rate <hz>` line |

All formats go through `sf_core::ir`, which has no Windows dependencies. The
test suite stores the same synthetic impulse response in every format
(`crates/sf-core/tests/fixtures/ir`) and asserts that:

* WAVE, FLAC, raw float and text load sample-for-sample identically (worst
  difference < 1e-6);
* their measured magnitude responses agree within 1e-5 dB;
* the lossy Ogg/Vorbis copy agrees within 0.3 dB RMS in band and under 1 dB at
  every frequency within 40 dB of the peak (outside that band the impulse
  response has no energy and the codec noise floor dominates);
* convolving an impulse through a WAVE and a FLAC configuration produces the
  same output within 1e-5.

Raw float and text files carry no sample rate, so they default to 48 kHz. A text
file can declare its own rate with `sample_rate 96000`. An impulse response whose
rate differs from the device is rejected with a diagnostic rather than resampled.

## Loudness correction

Syntax, verified against the reference binary's own parameter regex
(`s*State\s+(0|1)`, `s*ReferenceLevel\s+([-+0-9]+)`):

```
LoudnessCorrection: State <0|1> ReferenceLevel <integer dB> [ReferenceOffset <integer dB>] [Attenuation <0..1>]
```

The implemented model, documented in `sf_core::dsp::loudness`:

* `below = clamp(ReferenceLevel - ReferenceOffset - volume, 0, 40)`;
* low shelf at **75 Hz** with `+0.5 * below` dB;
* high shelf at **10 kHz** with `+0.25 * below` dB;
* headroom preamp of `-0.5 * (low + high)` dB.

`Attenuation` scales the whole correction between 0 and 1. `State 0` bypasses it
without removing the line. The listening volume is read from the default playback
endpoint and re-sampled by the watcher whenever it moves by 1 dB or more, so turning
the Windows volume knob retunes the correction without editing the configuration. At or above the reference the correction is exactly
0 dB, so the effect is inaudible at normal listening levels and grows as the
volume drops. Tests assert the measured pipeline gain at 20 dB, 30 dB and 40 dB
below the reference, the headroom cut, the attenuation scaling, the neutral point,
and that malformed lines are errors.

The reference implementation does not publish its coefficients, so the exact
shelf gains may differ numerically from Equalizer APO even though the shape and
behaviour match. Nothing else about the command is approximated: if a parameter
is misspelled (`ReferenceLevl`), it is an error, not a silently neutral filter.

## Configuration locations

```
%APPDATA%\SonicForge\config\config.txt
%APPDATA%\SonicForge\config\*.txt   (Include: targets)
%APPDATA%\SonicForge\config\*.wav   (Convolution: impulse responses)
```

`sonicforge config import <path>` copies a foreign configuration into place and
`sonicforge config export <path>` writes it back out.

## Known differences

* **No VST2 hosting.** The VST2 SDK is not redistributable and there is no
  Free/Open licensed host implementation to build on, so SonicForge refuses
  `VSTPlugin:` with a clear error instead of pretending. The rest of the chain
  still runs.
* **`LoudnessCorrection` follows the documented model above**, not the private
  coefficients of the reference build.
* **Performance defaults differ.** SonicForge uses one block of latency for
  convolution by default (512 samples at 48 kHz) and does not attempt the
  reference's zero-latency short filters.
* **Device selection** matches the endpoint friendly name. Configurations that
  rely on the reference's per-device file separation should use `Device:` lines
  instead.
* **`T60` on modal filters** is parsed and ignored, matching the reference, and
  documented here so it is not mistaken for silent data loss.

## Verified on hardware

Endpoint enumeration, APO registration and the COM interface were exercised on
the development machine (Windows 11 x64, 14 render endpoints including
VB-Audio Voicemeeter and Realtek/NVIDIA HDMI). A full listening test matrix
across every device class has **not** been completed; see
`docs/INSTALL.md` for the manual checklist.
