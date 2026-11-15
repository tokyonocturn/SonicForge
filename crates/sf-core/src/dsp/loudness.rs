//! Loudness correction.
//!
//! Equalizer APO's `LoudnessCorrection` command compensates for the change in
//! perceived frequency balance as the listening level drops: at low volume the
//! ear is less sensitive to bass and treble. The command tracks the system
//! volume relative to a calibrated reference and applies a low shelf, a high
//! shelf and a matching headroom cut.
//!
//! Documented syntax (verified against the reference binary's own parser):
//!
//! ```text
//! LoudnessCorrection: State <0|1> ReferenceLevel <integer dB> [ReferenceOffset <integer dB>] [Attenuation <0..1>]
//! ```
//!
//! The model implemented here is deliberately simple and fully documented:
//!
//! * `below = (ReferenceLevel - ReferenceOffset) - volume`, clamped to `0..40`.
//! * low shelf at 75 Hz: `+0.5 * below` dB, scaled by `Attenuation`.
//! * high shelf at 10 kHz: `+0.25 * below` dB, scaled by `Attenuation`.
//! * headroom preamp: `-0.5 * (low + high)` so the bass boost cannot clip.
//!
//! At or above the reference the correction is exactly neutral, which is what
//! makes the effect inaudible for normal listening and only active when the
//! user turns the volume down.

/// Corner frequency of the low shelf, Hz.
pub const LOW_SHELF_HZ: f64 = 75.0;
/// Corner frequency of the high shelf, Hz.
pub const HIGH_SHELF_HZ: f64 = 10000.0;
/// Largest amount of correction applied, in dB below reference.
pub const MAX_CORRECTION_DB: f64 = 40.0;

/// Parsed `LoudnessCorrection:` parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessSettings {
    /// `State 0` switches the filter off without removing the line.
    pub state: bool,
    /// System volume, in dB, that produces the reference level in the room.
    pub reference_level: f64,
    /// Moves the effective reference without repeating the calibration.
    pub reference_offset: f64,
    /// Scales the whole correction from 0 (none) to 1 (full).
    pub attenuation: f64,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            state: true,
            reference_level: -20.0,
            reference_offset: 0.0,
            attenuation: 1.0,
        }
    }
}

impl LoudnessSettings {
    /// The reference that the current volume is compared against.
    pub fn effective_reference(&self) -> f64 {
        self.reference_level - self.reference_offset
    }
}

/// The correction to apply for one listening volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessCorrection {
    /// Low shelf gain at [`LOW_SHELF_HZ`], dB.
    pub low_shelf_db: f64,
    /// High shelf gain at [`HIGH_SHELF_HZ`], dB.
    pub high_shelf_db: f64,
    /// Headroom preamp, dB (never positive).
    pub preamp_db: f64,
}

impl LoudnessCorrection {
    pub fn is_neutral(&self) -> bool {
        self.low_shelf_db == 0.0 && self.high_shelf_db == 0.0 && self.preamp_db == 0.0
    }
}

/// Computes the correction for the given system volume, in dB relative to full
/// scale (`0.0` is maximum volume).
pub fn correction(settings: &LoudnessSettings, volume_db: f64) -> LoudnessCorrection {
    if !settings.state {
        return LoudnessCorrection {
            low_shelf_db: 0.0,
            high_shelf_db: 0.0,
            preamp_db: 0.0,
        };
    }
    let attenuation = settings.attenuation.clamp(0.0, 1.0);
    let below = (settings.effective_reference() - volume_db).clamp(0.0, MAX_CORRECTION_DB);
    let low_shelf_db = 0.5 * below * attenuation;
    let high_shelf_db = 0.25 * below * attenuation;
    let preamp_db = -0.5 * (low_shelf_db + high_shelf_db);
    LoudnessCorrection {
        low_shelf_db,
        high_shelf_db,
        preamp_db,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> LoudnessSettings {
        LoudnessSettings {
            state: true,
            reference_level: -20.0,
            reference_offset: 0.0,
            attenuation: 1.0,
        }
    }

    #[test]
    fn neutral_at_and_above_the_reference() {
        let s = settings();
        for volume in [-20.0, -10.0, 0.0] {
            let c = correction(&s, volume);
            assert!(c.is_neutral(), "volume {volume}: {c:?}");
        }
    }

    #[test]
    fn correction_grows_as_the_volume_drops() {
        let s = settings();
        let at_minus_30 = correction(&s, -30.0);
        assert!((at_minus_30.low_shelf_db - 5.0).abs() < 1e-12);
        assert!((at_minus_30.high_shelf_db - 2.5).abs() < 1e-12);
        assert!((at_minus_30.preamp_db + 3.75).abs() < 1e-12);

        let at_minus_40 = correction(&s, -40.0);
        assert!((at_minus_40.low_shelf_db - 10.0).abs() < 1e-12);
        assert!((at_minus_40.high_shelf_db - 5.0).abs() < 1e-12);
    }

    #[test]
    fn correction_is_clamped_at_the_documented_limit() {
        let s = settings();
        let deep = correction(&s, -120.0);
        assert!((deep.low_shelf_db - MAX_CORRECTION_DB * 0.5).abs() < 1e-12);
        assert!((deep.high_shelf_db - MAX_CORRECTION_DB * 0.25).abs() < 1e-12);
    }

    #[test]
    fn attenuation_scales_the_correction() {
        let mut s = settings();
        s.attenuation = 0.5;
        let c = correction(&s, -40.0);
        assert!((c.low_shelf_db - 5.0).abs() < 1e-12);
        assert!((c.high_shelf_db - 2.5).abs() < 1e-12);
        s.attenuation = 0.0;
        assert!(correction(&s, -40.0).is_neutral());
    }

    #[test]
    fn reference_offset_moves_the_reference() {
        let mut s = settings();
        s.reference_offset = -10.0;
        assert_eq!(s.effective_reference(), -10.0);
        assert!(correction(&s, -10.0).is_neutral());
        assert!((correction(&s, -20.0).low_shelf_db - 5.0).abs() < 1e-12);
    }

    #[test]
    fn state_zero_bypasses_everything() {
        let mut s = settings();
        s.state = false;
        assert!(correction(&s, -60.0).is_neutral());
    }

    #[test]
    fn headroom_always_offsets_the_boost() {
        let s = settings();
        for volume in [-20.0, -25.0, -30.0, -40.0, -60.0] {
            let c = correction(&s, volume);
            assert!(c.preamp_db <= 0.0);
            // The preamp is exactly half of the combined boost.
            let expected = -0.5 * (c.low_shelf_db + c.high_shelf_db);
            assert!((c.preamp_db - expected).abs() < 1e-12);
        }
    }
}
