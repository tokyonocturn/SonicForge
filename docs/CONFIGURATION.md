# Configuring SonicForge

SonicForge reads Equalizer APO style text configuration. This document is the
authoritative grammar reference for the implementation in `sf-core`.

The active file is `%APPDATA%\SonicForge\config\config.txt`. `sonicforge config path`
prints it, `sonicforge config edit` opens it in Notepad, and the editor writes it with a
`.bak` backup. Changes are picked up automatically within about a second; no restart is
needed.

## Basics

```
# Comments start with a hash and run to the end of the line.
Preamp: -6 dB
Filter: ON PK Fc 105 Hz Gain 4.5 dB Q 0.7
```

Commands are case sensitive. A `Filter:` line must start with `ON` or `OFF`. Numbers may
carry a unit suffix; `kHz` is accepted anywhere `Hz` is.

## Filters

| Type | Meaning | Parameters |
| --- | --- | --- |
| `PK`, `PEQ`, `Modal` | peaking bell | `Fc`, `Gain`, `Q` (or `BW`/`S`) |
| `LP`, `LPQ` | low pass | `Fc`, `Q` |
| `HP`, `HPQ` | high pass | `Fc`, `Q` |
| `BP` | band pass | `Fc`, `Q` |
| `LS`, `HS` | shelving, 12 dB/oct | `Fc`, `Gain`, `Q` |
| `LS 6dB`, `HS 6dB` | shelving, 6 dB/oct | `Fc`, `Gain`, `Q` |
| `LSC`, `HSC` | shelving with slope | `Fc`, `Gain`, `Q`, `Slope` in dB/oct |
| `NO` | notch | `Fc`, `Q` |
| `AP` | all pass | `Fc`, `Q` |
| `IIR` | arbitrary coefficients | `Order <n> Coefficients a0 a1 ... b0 b1 ...` |

Named filters are supported: `Filter 3: ON PK Fc 60 Hz Gain 3 dB Q 1` and later
`Filter: ON PK Fc ...` lines are independent; `Filter: ON` without a number applies to no
named filter.

## Graphic EQ

```
GraphicEQ: 25 0; 40 2; 63 1; 100 0; 160 -1; 250 0; 400 1; 1000 0; 4000 2; 16000 0
```

Points are joined by straight lines in log-frequency space, the curve is flat outside the
first and last point, and the resulting response is realised by a bank of peaking filters
whose gains are refined iteratively until they match the target curve.

## Routing and channels

```
Channel: L R            # process only left and right
Channel: L R !          # invert them
Copy: L L2              # create a virtual channel
Copy: L R2 +-6dB        # copy with a gain offset in dB
```

Virtual channels are allocated on demand beyond the device channel count and are dropped
at the output; use them together with `Copy:` to build crossfeeds and bass management.

## Delay, convolution, preamp

```
Delay: 12.5 ms          # all channels
Delay: 1 3.0 ms         # channel index 1 only
Convolution: room.wav   # WAVE impulse response at the device sample rate
Preamp: -3 dB
```

Convolution supports PCM 8/16/24/32-bit and float 32/64-bit WAVE files. An impulse
response whose sample rate differs from the device is rejected with a diagnostic rather
than silently resampled. The partition size is 512 samples, so latency is one block
(about 10.7 ms at 48 kHz) and is reported by `sonicforge diagnose` and in the editor.

## Device selection, stages and includes

```
Device: Speakers (Realtek(R) Audio)
Stage: post-mix
Include: harman.txt
```

`Device:` takes a list of patterns matched against the endpoint friendly name; a
configuration without `Device:` lines applies to every endpoint that has the APO enabled.
`Include:` accepts a path relative to the configuration directory. `Stage: pre-mix`
applies per application stream, `Stage: post-mix` (default) per device.

## Variables and conditionals

```
Eval: boost = 4.5
Eval: f = 105
Filter: ON PK Fc `f` Hz Gain `boost` dB Q 0.7

If: boost > 3
    Preamp: -3 dB
ElseIf: boost > 0
    Preamp: -1 dB
Else
    Preamp: 0 dB
EndIf
```

Backtick expressions are evaluated inline. `Eval:` variables are evaluated in file order,
so a variable must be defined before it is used, exactly as in Equalizer APO.

Expressions support `+ - * / %`, comparisons, `&& || !`, `?:`, parentheses,
`min/max/abs/sqrt/log/log10/exp/pow/sin/cos/tan`, string concatenation with `+`, and
regular expressions in `If:` conditions.

## Validation

```
sonicforge validate config.txt
```

prints every diagnostic with its line number. The editor shows the same diagnostics live,
and the APO skips only the commands that failed — a broken line never silences audio.
