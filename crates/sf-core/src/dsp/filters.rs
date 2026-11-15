//! Filter designs used by the `Filter:` configuration command.
//!
//! All designs follow the Audio EQ Cookbook by Robert Bristow-Johnson so that
//! the rendered response matches the reference implementation for the filter
//! types shared by both products.

use super::biquad::Biquad;
use std::f64::consts::PI;

/// Which parameter characterises the filter bandwidth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bandwidth {
    /// Quality factor.
    Q(f64),
    /// Bandwidth in octaves (converted to Q at design time).
    Octaves(f64),
}

impl Default for Bandwidth {
    fn default() -> Self {
        Bandwidth::Q(std::f64::consts::FRAC_1_SQRT_2)
    }
}

impl Bandwidth {
    /// Resolves to a Q value for the given centre frequency and sample rate.
    pub fn to_q(self, freq: f64, sample_rate: f64) -> f64 {
        match self {
            Bandwidth::Q(q) => q.max(1e-4),
            Bandwidth::Octaves(bw) => {
                let w0 = 2.0 * PI * freq / sample_rate;
                let sn = w0.sin();
                if sn.abs() < 1e-12 || bw.abs() < 1e-12 {
                    std::f64::consts::FRAC_1_SQRT_2
                } else {
                    let inv_q = 2.0 * ((2f64.ln() / 2.0) * bw * w0 / sn).sinh();
                    if inv_q.abs() < 1e-12 {
                        std::f64::consts::FRAC_1_SQRT_2
                    } else {
                        (1.0 / inv_q).abs()
                    }
                }
            }
        }
    }
}

/// Filter types supported by the SonicForge configuration language.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterKind {
    /// Peaking EQ (`PK`, `PEQ`, `Modal`).
    Peak,
    /// Notch filter (`NO`).
    Notch,
    /// Low pass (`LP`, `LPQ`).
    LowPass,
    /// High pass (`HP`, `HPQ`).
    HighPass,
    /// Band pass with 0 dB peak gain (`BP`).
    BandPass,
    /// All pass (`AP`).
    AllPass,
    /// Low shelf with a Q parameter (`LSC`).
    LowShelf,
    /// High shelf with a Q parameter (`HSC`).
    HighShelf,
    /// Low shelf with a slope in dB/octave (`LSC n dB`).
    LowShelfSlope,
    /// High shelf with a slope in dB/octave (`HSC n dB`).
    HighShelfSlope,
    /// Fixed 6 dB/octave low shelf (`LS 6dB`).
    LowShelf6,
    /// Fixed 6 dB/octave high shelf (`HS 6dB`).
    HighShelf6,
    /// Fixed 12 dB/octave low shelf (`LS`, `LS 12dB`).
    LowShelf12,
    /// Fixed 12 dB/octave high shelf (`HS`, `HS 12dB`).
    HighShelf12,
    /// Bypass.
    None,
}

impl FilterKind {
    /// Whether the type uses the gain parameter.
    pub fn uses_gain(self) -> bool {
        matches!(
            self,
            FilterKind::Peak
                | FilterKind::LowShelf
                | FilterKind::HighShelf
                | FilterKind::LowShelfSlope
                | FilterKind::HighShelfSlope
                | FilterKind::LowShelf6
                | FilterKind::HighShelf6
                | FilterKind::LowShelf12
                | FilterKind::HighShelf12
        )
    }

    /// Whether the type uses the Q/bandwidth parameter.
    pub fn uses_q(self) -> bool {
        matches!(
            self,
            FilterKind::Peak
                | FilterKind::Notch
                | FilterKind::LowPass
                | FilterKind::HighPass
                | FilterKind::BandPass
                | FilterKind::AllPass
                | FilterKind::LowShelf
                | FilterKind::HighShelf
        )
    }

    /// Human readable name used by the serializer.
    pub fn name(self) -> &'static str {
        match self {
            FilterKind::Peak => "PK",
            FilterKind::Notch => "NO",
            FilterKind::LowPass => "LP",
            FilterKind::HighPass => "HP",
            FilterKind::BandPass => "BP",
            FilterKind::AllPass => "AP",
            FilterKind::LowShelf => "LSC",
            FilterKind::HighShelf => "HSC",
            FilterKind::LowShelfSlope => "LSC",
            FilterKind::HighShelfSlope => "HSC",
            FilterKind::LowShelf6 => "LS 6dB",
            FilterKind::HighShelf6 => "HS 6dB",
            FilterKind::LowShelf12 => "LS 12dB",
            FilterKind::HighShelf12 => "HS 12dB",
            FilterKind::None => "None",
        }
    }
}

/// Complete description of a single filter.
#[derive(Clone, Copy, Debug)]
pub struct FilterSpec {
    pub kind: FilterKind,
    pub freq: f64,
    pub gain_db: f64,
    pub bandwidth: Bandwidth,
    /// Slope in dB/octave for the `LSC n dB` / `HSC n dB` variants.
    pub slope: f64,
}

impl FilterSpec {
    pub fn new(kind: FilterKind, freq: f64) -> Self {
        Self {
            kind,
            freq,
            gain_db: 0.0,
            bandwidth: Bandwidth::default(),
            slope: 12.0,
        }
    }
}

#[inline]
fn db_to_amp(db: f64) -> f64 {
    10f64.powf(db / 40.0)
}

/// Designs the biquad for `spec` at `sample_rate`. Returns `None` when the
/// filter degenerates (bypass, invalid frequency, or unstable poles).
pub fn design(spec: &FilterSpec, sample_rate: f64) -> Option<Biquad> {
    if sample_rate <= 0.0 || !sample_rate.is_finite() {
        return None;
    }
    // Clamp the centre frequency into the representable range. Equalizer APO
    // silently ignores filters that cannot be represented at the current rate.
    let nyquist = sample_rate / 2.0;
    if !spec.freq.is_finite() || spec.freq <= 0.0 || spec.freq >= nyquist {
        return None;
    }
    let freq = spec.freq;
    let w0 = 2.0 * PI * freq / sample_rate;
    let (sin_w0, cos_w0) = w0.sin_cos();
    let q = spec.bandwidth.to_q(freq, sample_rate);
    let alpha = if q > 0.0 && q.is_finite() { sin_w0 / (2.0 * q) } else { sin_w0 / 2.0 };
    let a = db_to_amp(spec.gain_db);

    let bq = match spec.kind {
        FilterKind::None => return None,
        FilterKind::Peak => Biquad::new(
            [1.0 + alpha * a, -2.0 * cos_w0, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos_w0, 1.0 - alpha / a],
        ),
        FilterKind::Notch => Biquad::new(
            [1.0, -2.0 * cos_w0, 1.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        ),
        FilterKind::LowPass => Biquad::new(
            [(1.0 - cos_w0) / 2.0, 1.0 - cos_w0, (1.0 - cos_w0) / 2.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        ),
        FilterKind::HighPass => Biquad::new(
            [(1.0 + cos_w0) / 2.0, -(1.0 + cos_w0), (1.0 + cos_w0) / 2.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        ),
        FilterKind::BandPass => Biquad::new(
            [alpha, 0.0, -alpha],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        ),
        FilterKind::AllPass => Biquad::new(
            [1.0 - alpha, -2.0 * cos_w0, 1.0 + alpha],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        ),
        FilterKind::LowShelf => {
            let sqrt_a = a.sqrt();
            let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
            Biquad::new(
                [
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                ],
                [
                    (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                ],
            )
        }
        FilterKind::HighShelf => {
            let sqrt_a = a.sqrt();
            let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
            Biquad::new(
                [
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                ],
                [
                    (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                ],
            )
        }
        FilterKind::LowShelfSlope | FilterKind::HighShelfSlope => {
            // RBJ shelf where the slope S is given in dB/octave. A slope of
            // 12 dB/oct corresponds to S = 1, the maximum ripple free slope.
            let s = (spec.slope / 12.0).clamp(0.05, 1.0);
            let alpha_s = sin_w0 / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.0).sqrt();
            let sqrt_a = a.sqrt();
            let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha_s;
            if spec.kind == FilterKind::LowShelfSlope {
                Biquad::new(
                    [
                        a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                        2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                        a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                    ],
                    [
                        (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                        -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                        (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                    ],
                )
            } else {
                Biquad::new(
                    [
                        a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                        -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                        a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                    ],
                    [
                        (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                        2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                        (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                    ],
                )
            }
        }
        FilterKind::LowShelf12 => {
            // Second order Butterworth shelf (S = 1).
            let alpha_s = sin_w0 / 2.0 * 2f64.sqrt();
            let sqrt_a = a.sqrt();
            let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha_s;
            Biquad::new(
                [
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                ],
                [
                    (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                ],
            )
        }
        FilterKind::HighShelf12 => {
            let alpha_s = sin_w0 / 2.0 * 2f64.sqrt();
            let sqrt_a = a.sqrt();
            let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha_s;
            Biquad::new(
                [
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha),
                ],
                [
                    (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha,
                ],
            )
        }
        FilterKind::LowShelf6 => first_order_low_shelf(freq, spec.gain_db, sample_rate),
        FilterKind::HighShelf6 => first_order_high_shelf(freq, spec.gain_db, sample_rate),
    };

    if bq.is_stable() {
        Some(bq)
    } else {
        None
    }
}

/// First order low shelf: `H(s) = (s + w0*A) / (s + w0/A)` with `A = sqrt(V0)`,
/// bilinear transformed with frequency pre-warping.
fn first_order_low_shelf(freq: f64, gain_db: f64, sample_rate: f64) -> Biquad {
    let v0 = 10f64.powf(gain_db / 20.0);
    let a = v0.sqrt();
    let w = 2.0 * sample_rate * (PI * freq / sample_rate).tan();
    let d = 2.0 * sample_rate;
    let a0 = d + w / a;
    Biquad::new([d + w * a, w * a - d, 0.0], [a0, w / a - d, 0.0])
}

/// First order high shelf: `H(s) = (A*s + w0) / (s/A + w0)`.
fn first_order_high_shelf(freq: f64, gain_db: f64, sample_rate: f64) -> Biquad {
    let v0 = 10f64.powf(gain_db / 20.0);
    let a = v0.sqrt();
    let w = 2.0 * sample_rate * (PI * freq / sample_rate).tan();
    let d = 2.0 * sample_rate;
    let a0 = d / a + w;
    Biquad::new([a * d + w, w - a * d, 0.0], [a0, w - d / a, 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    fn response_at(spec: &FilterSpec, freq: f64) -> f64 {
        design(spec, FS).expect("filter should design").magnitude_db(freq, FS)
    }

    #[test]
    fn peak_filter_hits_requested_gain_at_centre() {
        for gain in [-12.0, -6.0, 3.0, 9.0] {
            let mut spec = FilterSpec::new(FilterKind::Peak, 1000.0);
            spec.gain_db = gain;
            spec.bandwidth = Bandwidth::Q(2.0);
            let measured = response_at(&spec, 1000.0);
            assert!((measured - gain).abs() < 0.02, "gain {gain} -> {measured}");
        }
    }

    #[test]
    fn peak_filter_is_flat_far_from_centre() {
        let mut spec = FilterSpec::new(FilterKind::Peak, 1000.0);
        spec.gain_db = 12.0;
        spec.bandwidth = Bandwidth::Q(1.0);
        assert!(response_at(&spec, 20.0).abs() < 0.2);
        assert!(response_at(&spec, 20000.0).abs() < 0.2);
    }

    #[test]
    fn lowpass_is_minus_three_db_at_cutoff() {
        let spec = FilterSpec::new(FilterKind::LowPass, 1000.0);
        let at_cutoff = response_at(&spec, 1000.0);
        assert!((at_cutoff + 3.0103).abs() < 0.05, "LP at Fc = {at_cutoff} dB");
    }

    #[test]
    fn lowpass_rolls_off_at_twelve_db_per_octave() {
        // Measure away from Nyquist: a digital biquad has a zero at fs/2, so
        // the slope steepens there beyond the analog prototype.
        let spec = FilterSpec::new(FilterKind::LowPass, 1000.0);
        let a = response_at(&spec, 2000.0);
        let b = response_at(&spec, 4000.0);
        assert!((a - b - 12.0).abs() < 0.5, "rolloff {} dB/oct", a - b);
    }

    #[test]
    fn highpass_is_minus_three_db_at_cutoff() {
        let spec = FilterSpec::new(FilterKind::HighPass, 1000.0);
        let at_cutoff = response_at(&spec, 1000.0);
        assert!((at_cutoff + 3.0103).abs() < 0.05, "HP at Fc = {at_cutoff} dB");
    }

    #[test]
    fn shelf_reaches_requested_plateau() {
        let mut low = FilterSpec::new(FilterKind::LowShelf12, 200.0);
        low.gain_db = 6.0;
        let dc = response_at(&low, 10.0);
        assert!((dc - 6.0).abs() < 0.2, "low shelf DC gain {dc}");

        let mut high = FilterSpec::new(FilterKind::HighShelf12, 4000.0);
        high.gain_db = -4.0;
        let treble = response_at(&high, 20000.0);
        assert!((treble + 4.0).abs() < 0.4, "high shelf HF gain {treble}");
    }

    #[test]
    fn six_db_shelf_spans_its_gain_across_the_corner() {
        let mut low = FilterSpec::new(FilterKind::LowShelf6, 100.0);
        low.gain_db = 12.0;
        let dc = response_at(&low, 5.0);
        let corner = response_at(&low, 100.0);
        let high = response_at(&low, 20000.0);
        assert!((dc - 12.0).abs() < 0.5, "DC gain {dc}");
        assert!((high).abs() < 0.2, "HF gain {high}");
        // A first order shelf passes through half of its gain at the corner.
        assert!((corner - 6.0).abs() < 1.5, "corner gain {corner}");
    }

    #[test]
    fn notch_rejects_centre_frequency() {
        let mut spec = FilterSpec::new(FilterKind::Notch, 1000.0);
        spec.bandwidth = Bandwidth::Q(4.0);
        let centre = response_at(&spec, 1000.0);
        assert!(centre < -40.0, "notch depth {centre}");
    }

    #[test]
    fn bandpass_has_unity_peak_gain() {
        let mut spec = FilterSpec::new(FilterKind::BandPass, 1000.0);
        spec.bandwidth = Bandwidth::Q(2.0);
        let centre = response_at(&spec, 1000.0);
        assert!(centre.abs() < 0.05, "bandpass peak {centre}");
    }

    #[test]
    fn allpass_has_flat_magnitude() {
        let mut spec = FilterSpec::new(FilterKind::AllPass, 900.0);
        spec.bandwidth = Bandwidth::Q(0.707);
        for f in [50.0, 200.0, 900.0, 5000.0, 15000.0] {
            assert!(response_at(&spec, f).abs() < 1e-6);
        }
    }

    #[test]
    fn bandwidth_in_octaves_matches_q_conversion() {
        // 1 octave bandwidth == Q of about 1.414 at mid frequencies.
        let q = Bandwidth::Octaves(1.0).to_q(1000.0, FS);
        assert!((q - 1.4142).abs() < 0.02, "Q = {q}");
    }

    #[test]
    fn filters_outside_nyquist_are_rejected() {
        let spec = FilterSpec::new(FilterKind::Peak, 30000.0);
        assert!(design(&spec, FS).is_none());
        let spec = FilterSpec::new(FilterKind::Peak, 0.0);
        assert!(design(&spec, FS).is_none());
    }

    #[test]
    fn all_designed_filters_are_stable() {
        for kind in [
            FilterKind::Peak,
            FilterKind::Notch,
            FilterKind::LowPass,
            FilterKind::HighPass,
            FilterKind::BandPass,
            FilterKind::AllPass,
            FilterKind::LowShelf,
            FilterKind::HighShelf,
            FilterKind::LowShelfSlope,
            FilterKind::HighShelfSlope,
            FilterKind::LowShelf6,
            FilterKind::HighShelf6,
            FilterKind::LowShelf12,
            FilterKind::HighShelf12,
        ] {
            for &f in &[20.0, 100.0, 1000.0, 10000.0, 20000.0] {
                for &g in &[-24.0, -6.0, 0.0, 6.0, 24.0] {
                    let mut spec = FilterSpec::new(kind, f);
                    spec.gain_db = g;
                    spec.bandwidth = Bandwidth::Q(0.7);
                    if let Some(bq) = design(&spec, FS) {
                        assert!(bq.is_stable(), "{kind:?} at {f} Hz {g} dB unstable");
                    }
                }
            }
        }
    }
}
