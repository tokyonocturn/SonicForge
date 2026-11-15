# Installing and verifying SonicForge

## Requirements

* Windows 10 or Windows 11, x64.
* Administrator rights for installation (COM registration and endpoint configuration
  live under `HKLM`).

## Install

From an elevated prompt, in the folder that contains the release binaries:

```powershell
.\sf-setup.exe install
```

This copies the binaries to `C:\Program Files\SonicForge`, registers the APO class,
writes the uninstall entry, creates the configuration layout and enables processing on
every active playback endpoint.

Useful flags:

| Flag | Effect |
| --- | --- |
| `--dir <path>` | install somewhere else |
| `--no-enable` | install without touching any endpoint |

After installation, restart playback (or the Windows Audio service) so the audio engine
loads the effect:

```powershell
Restart-Service audiosrv
```

## Verify

```powershell
sonicforge apo status          # which endpoints are processed
sonicforge apo self-test       # COM round trip against the installed DLL
sonicforge devices             # endpoints, with a * on the default device
```

`apo self-test` loads the DLL, obtains the class factory, creates the APO, queries the
registration properties and the configuration interface, and checks that the reference
count reaches zero. If it passes, the COM side is sound; it does not prove that audio is
flowing.

For a signal-path check that does not depend on a physical device, render a file:

```powershell
sonicforge render eq.txt --input test.wav --output out.wav
```

## Manual listening checklist

The automated tests cover the DSP and the COM plumbing, not the listening experience on
every device class. When validating a new machine, check:

1. Music plays with `Preamp: 0 dB` and no filters (pass-through is bit-identical).
2. A `Filter: ON LP Fc 500 Hz` audibly muffles the same track.
3. The editor's curve matches what you hear for a large `+12 dB` band.
4. Switching the default device keeps audio running (the APO reloads per stream).
5. `sonicforge apo disable --all` restores the previous effect chain and audio still
   plays.

## Uninstall / recovery

```powershell
# elevated
sf-setup.exe uninstall
```

or, to only detach the APO without removing files:

```powershell
sonicforge apo disable --all
sonicforge apo uninstall
```

Both restore the effect CLSID and `Disable_SysFx` value that were present before
SonicForge was enabled, so another vendor's APO keeps working. Your configuration and
presets in `%APPDATA%\SonicForge` are left untouched.

If audio breaks after a manual experiment, `sonicforge apo disable --all` followed by a
`Restart-Service audiosrv` is the supported recovery path.

## Logs and diagnostics

Logs are written to `%APPDATA%\SonicForge\logs`. For a bug report:

```powershell
sonicforge diagnose --bundle .\sf-report
```

This writes the configuration, endpoint state, engine diagnostics and recent log lines.
It contains no audio and no personal data.
