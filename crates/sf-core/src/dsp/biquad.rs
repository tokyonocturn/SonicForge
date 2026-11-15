//! Second order sections (biquads) with analytic frequency response.

use super::fft::Complex64;
use std::f64::consts::PI;

/// A normalised biquad: `(b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Biquad {
    pub const IDENTITY: Biquad = Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// Normalises raw coefficients by `a0`.
    pub fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        debug_assert!(a[0] != 0.0, "a0 must not be zero");
        let inv = 1.0 / a[0];
        Self {
            b0: b[0] * inv,
            b1: b[1] * inv,
            b2: b[2] * inv,
            a1: a[1] * inv,
            a2: a[2] * inv,
        }
    }

    /// True when the section is a pure gain (no filtering).
    pub fn is_identity(&self) -> bool {
        self.b1 == 0.0
            && self.b2 == 0.0
            && self.a1 == 0.0
            && self.a2 == 0.0
            && (self.b0 - 1.0).abs() < 1e-15
    }

    /// Complex response at normalised frequency `w` (radians/sample).
    pub fn response(&self, w: f64) -> Complex64 {
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num = Complex64::new(self.b0 + self.b1 * c1 + self.b2 * c2, -(self.b1 * s1 + self.b2 * s2));
        let den = Complex64::new(1.0 + self.a1 * c1 + self.a2 * c2, -(self.a1 * s1 + self.a2 * s2));
        let d = den.re * den.re + den.im * den.im;
        if d < 1e-300 {
            return Complex64::new(f64::INFINITY, 0.0);
        }
        Complex64::new(
            (num.re * den.re + num.im * den.im) / d,
            (num.im * den.re - num.re * den.im) / d,
        )
    }

    /// Magnitude response in decibels at `freq` Hz for a `sample_rate`.
    pub fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let r = self.response(2.0 * PI * freq / sample_rate).norm();
        20.0 * r.max(1e-12).log10()
    }

    /// True when the poles are inside the unit circle.
    pub fn is_stable(&self) -> bool {
        // Jury stability test for a second order denominator.
        (self.a2.abs() < 1.0) && (self.a1.abs() < 1.0 + self.a2)
    }
}

/// Per-channel filter state (transposed direct form II).
#[derive(Clone, Copy, Debug, Default)]
pub struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    #[inline]
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, coeffs: &Biquad, x: f32) -> f32 {
        let y = coeffs.b0 as f32 * x + self.z1;
        self.z1 = coeffs.b1 as f32 * x - coeffs.a1 as f32 * y + self.z2;
        self.z2 = coeffs.b2 as f32 * x - coeffs.a2 as f32 * y;
        y
    }

    /// Processes a slice in place.
    #[inline]
    pub fn process_slice(&mut self, coeffs: &Biquad, buf: &mut [f32]) {
        let b0 = coeffs.b0 as f32;
        let b1 = coeffs.b1 as f32;
        let b2 = coeffs.b2 as f32;
        let a1 = coeffs.a1 as f32;
        let a2 = coeffs.a2 as f32;
        let mut z1 = self.z1;
        let mut z2 = self.z2;
        for x in buf.iter_mut() {
            let input = *x;
            let y = b0 * input + z1;
            z1 = b1 * input - a1 * y + z2;
            z2 = b2 * input - a2 * y;
            *x = y;
        }
        self.z1 = z1;
        self.z2 = z2;
    }

    /// Flushes denormals that would otherwise cost performance on x86.
    #[inline]
    pub fn sanitize(&mut self) {
        if self.z1.abs() < 1e-25 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-25 {
            self.z2 = 0.0;
        }
    }
}

/// Generic direct form II transposed IIR of arbitrary order.
#[derive(Clone, Debug)]
pub struct IirFilter {
    /// Numerator coefficients, normalised to `a0 == 1`.
    pub b: Vec<f64>,
    /// Denominator coefficients, normalised to `a0 == 1`.
    pub a: Vec<f64>,
}

impl IirFilter {
    /// Builds a filter from raw `b` and `a` coefficient vectors. `a` must have
    /// the same length as `b` and `a[0] != 0`.
    pub fn new(b: &[f64], a: &[f64]) -> Option<Self> {
        if b.len() != a.len() || b.is_empty() || a[0] == 0.0 {
            return None;
        }
        let inv = 1.0 / a[0];
        Some(Self {
            b: b.iter().map(|v| v * inv).collect(),
            a: a.iter().map(|v| v * inv).collect(),
        })
    }

    pub fn order(&self) -> usize {
        self.b.len() - 1
    }

    pub fn response(&self, w: f64) -> Complex64 {
        let mut num = Complex64::ZERO;
        let mut den = Complex64::ZERO;
        for (i, &c) in self.b.iter().enumerate() {
            num = num.add(Complex64::new((w * i as f64).cos(), -(w * i as f64).sin()).scale(c));
        }
        for (i, &c) in self.a.iter().enumerate() {
            den = den.add(Complex64::new((w * i as f64).cos(), -(w * i as f64).sin()).scale(c));
        }
        let d = den.re * den.re + den.im * den.im;
        if d < 1e-300 {
            return Complex64::new(f64::INFINITY, 0.0);
        }
        Complex64::new(
            (num.re * den.re + num.im * den.im) / d,
            (num.im * den.re - num.re * den.im) / d,
        )
    }

    pub fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        20.0 * self.response(2.0 * PI * freq / sample_rate).norm().max(1e-12).log10()
    }
}

/// Generic IIR state (direct form II transposed, `order` delay elements).
#[derive(Clone, Debug, Default)]
pub struct IirState {
    z: Vec<f64>,
}

impl IirState {
    pub fn with_order(order: usize) -> Self {
        Self { z: vec![0.0; order] }
    }

    pub fn reset(&mut self) {
        self.z.iter_mut().for_each(|v| *v = 0.0);
    }

    pub fn process_slice(&mut self, filter: &IirFilter, buf: &mut [f32]) {
        let order = filter.order();
        if self.z.len() < order {
            self.z.resize(order, 0.0);
        }
        for x in buf.iter_mut() {
            let input = *x as f64;
            let y = filter.b[0] * input + self.z[0];
            for i in 0..order {
                let next = if i + 1 < order { self.z[i + 1] } else { 0.0 };
                self.z[i] = filter.b[i + 1] * input - filter.a[i + 1] * y + next;
            }
            *x = y as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_biquad_does_not_change_signal() {
        let mut state = BiquadState::default();
        let mut buf = vec![0.0f32, 1.0, -0.5, 0.25, 0.0];
        state.process_slice(&Biquad::IDENTITY, &mut buf);
        assert_eq!(buf, vec![0.0, 1.0, -0.5, 0.25, 0.0]);
    }

    #[test]
    fn dc_gain_of_identity_is_zero_db() {
        assert!(Biquad::IDENTITY.magnitude_db(100.0, 48000.0).abs() < 1e-12);
    }

    #[test]
    fn iir_order_two_matches_biquad_response() {
        // A lowpass biquad expressed as a generic IIR must have identical response.
        let bq = Biquad::new([0.2, 0.4, 0.2], [1.0, -0.4, 0.1]);
        let iir = IirFilter::new(&[0.2, 0.4, 0.2], &[1.0, -0.4, 0.1]).unwrap();
        for f in [20.0, 100.0, 1000.0, 10000.0] {
            let a = bq.magnitude_db(f, 48000.0);
            let b = iir.magnitude_db(f, 48000.0);
            assert!((a - b).abs() < 1e-9, "{f} Hz: {a} vs {b}");
        }
    }

    #[test]
    fn iir_state_processes_samples_like_direct_form() {
        let filter = IirFilter::new(&[0.5, 0.5], &[1.0, -0.25]).unwrap();
        let mut state = IirState::with_order(1);
        let mut buf: Vec<f32> = (0..16).map(|i| (i as f32 * 0.3).sin()).collect();
        let input = buf.clone();
        state.process_slice(&filter, &mut buf);
        // Direct evaluation of y[n] = 0.5x[n] + 0.5x[n-1] + 0.25y[n-1].
        let mut prev_x = 0.0f64;
        let mut prev_y = 0.0f64;
        for (i, &x) in input.iter().enumerate() {
            let y = 0.5 * x as f64 + 0.5 * prev_x + 0.25 * prev_y;
            assert!((buf[i] as f64 - y).abs() < 1e-6);
            prev_x = x as f64;
            prev_y = y;
        }
    }
}
