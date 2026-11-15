# Compatibility with Equalizer APO

SonicForge reads the same configuration language as Equalizer APO 1.4.x so an
existing `config.txt` can be pointed at it unchanged. The table below records
what is implemented, what degrades, and what is deliberately unsupported — this
is the honest state of the compatibility layer, not a wish list.

## Commands

| Command | Status |
| --- | --- |
| `Preamp: <n> dB` | Supported |
| `Filter: ON/OFF <type> ...` | Supported for all types below |
| `Filter <n>: ...` | Supported (named filters, `Filter: ON` reuses none) |
| `GraphicEQ: <f> <db>; ...` | Supported, log-frequency interpolation, flat outside range |
| `Convolution: <file>` | Supported (WAV impulse responses, partitioned FFT) |
| `Copy: <src> <dst> [gain]` | Supported, including virtual channels and `+-6dB` forms |
| `Delay: <ms>` / `Delay: <idx> <ms>` | Supported, fractional delay interpolated |
| `Channel: <spec>` / `Channel: <spec> !` | Supported, with `!` inversion |
| `Device: <patterns>` | Supported (matched against the endpoint friendly name) |
| `Stage: pre-mix / post-mix` | Supported |
| `Include: <file>` | Supported |
| `If: <expr>` / `Else` / `EndIf` | Supported |
| `Eval: <var> = <expr>` | Supported |
| `Filter: ON IIR Order <n> Coefficients ... ` | Supported |
| `LoudnessCorrection` | **Not implemented** — compiled as an explicit warning, never silently ignored |
| `VSTPlugin:` | **Not implemented** — reported as an error; see below |

## Filter types

PK/PEQ/Modal, LP/LPQ, HP/HPQ, BP, LS/HS (6 dB and 12 dB, plus `LSC`/`HSC` with
a slope in dB/oct), NO (notch), AP (all-pass), and IIR with arbitrary
coefficients. Filter parameter parsing accepts the same unit suffixes
(`Hz`, `kHz`, `dB`, `Q`, `BW`, `S`) as the reference.

## Expression language

Arithmetic, comparisons, `&&`/`||`/`!`, ternary `?:`, `min`/`max`/`abs`/`sqrt`/
`log`/`log10`/`sin`/`cos`/`tan`/`pow`/`exp`, `random()`, string helpers and
regular expressions via `regex`. Variables are evaluated in file order, so a
`Eval:` before use behaves exactly like the original.

## Configuration locations

SonicForge uses the same file layout as Equalizer APO:

```
%ProgramFiles%\SonicForge\config\config.txt
%ProgramFiles%\SonicForge\config\*.txt
%ProgramFiles%\SonicForge\config\*.wav
```

`sonicforge config import <path>` copies a foreign configuration into place and
`sonicforge config export <path>` writes it back out.

## Known differences

* **No VST2 hosting.** The VST2 SDK is not redistributable and there is no
  Free/Open licensed host implementation to build on, so SonicForge refuses
  `VSTPlugin:` with a clear error instead of pretending. The rest of the chain
  still runs.
* **No LoudnessCorrection curves.** The reference ships measured correction
  curves whose licence we could not verify for redistribution. The command is
  parsed and reported as unsupported.
* **Performance defaults differ.** SonicForge uses one block of latency for
  convolution by default (512 samples at 48 kHz) and does not attempt the
  reference's zero-latency short filters.
* **Device selection** matches the endpoint friendly name. Configurations that
  rely on the reference's per-device file separation should use `Device:` lines
  instead.

## Verified on hardware

Endpoint enumeration, APO registration and the COM interface were exercised on
the development machine (Windows 11 x64, 14 render endpoints including
VB-Audio Voicemeeter and Realtek/NVIDIA HDMI). A full listening test matrix
across every device class has **not** been completed; see
`docs/INSTALL.md` for the manual checklist.
