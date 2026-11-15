//! Small, dependency free FFT implementation.
//!
//! The convolver needs forward/inverse transforms of real signals with a
//! predictable, allocation free inner loop, so a compact radix-2 implementation
//! is shipped here rather than pulling in a heavy dependency.

use std::f64::consts::PI;

/// Double precision complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex64 {
    pub re: f64,
    pub im: f64,
}

impl Complex64 {
    pub const ZERO: Complex64 = Complex64 { re: 0.0, im: 0.0 };

    #[inline]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    #[inline]
    pub fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    #[inline]
    pub fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    #[inline]
    pub fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }

    #[inline]
    pub fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }

    #[inline]
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    #[inline]
    pub fn norm(self) -> f64 {
        (self.re * self.re + self.im * self.im).sqrt()
    }
}

/// In-place iterative radix-2 complex FFT.
#[derive(Clone, Debug)]
pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    twiddles: Vec<Complex64>,
}

impl Fft {
    /// Creates a transform for `n` samples. `n` must be a power of two.
    pub fn new(n: usize) -> Self {
        assert!(
            n.is_power_of_two(),
            "FFT size must be a power of two, got {n}"
        );
        let bits = n.trailing_zeros();
        let mut rev = vec![0u32; n];
        for (i, slot) in rev.iter_mut().enumerate() {
            *slot = (i as u32).reverse_bits() >> (32 - bits.max(1));
        }
        let mut twiddles = Vec::with_capacity(n / 2);
        for k in 0..n / 2 {
            let angle = -2.0 * PI * k as f64 / n as f64;
            twiddles.push(Complex64::new(angle.cos(), angle.sin()));
        }
        Self { n, rev, twiddles }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.n
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Forward transform (unnormalised).
    pub fn forward(&self, buf: &mut [Complex64]) {
        assert_eq!(buf.len(), self.n);
        self.transform(buf, false);
    }

    /// Inverse transform (includes the `1/n` normalisation).
    pub fn inverse(&self, buf: &mut [Complex64]) {
        assert_eq!(buf.len(), self.n);
        self.transform(buf, true);
        let scale = 1.0 / self.n as f64;
        for v in buf.iter_mut() {
            *v = v.scale(scale);
        }
    }

    fn transform(&self, buf: &mut [Complex64], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if i < j {
                buf.swap(i, j);
            }
        }
        let mut span = 2usize;
        while span <= n {
            let half = span / 2;
            let step = n / span;
            for start in (0..n).step_by(span) {
                for k in 0..half {
                    let mut w = self.twiddles[k * step];
                    if inverse {
                        w = w.conj();
                    }
                    let a = buf[start + k];
                    let b = buf[start + k + half].mul(w);
                    buf[start + k] = a.add(b);
                    buf[start + k + half] = a.sub(b);
                }
            }
            span <<= 1;
        }
    }
}

/// Real-input FFT built on top of a half-size complex transform.
#[derive(Clone, Debug)]
pub struct RealFft {
    n: usize,
    half: Fft,
    /// Twiddles `exp(-2*pi*i*k/n)` for `k` in `0..n/2`.
    twiddles: Vec<Complex64>,
}

impl RealFft {
    pub fn new(n: usize) -> Self {
        assert!(
            n.is_power_of_two() && n >= 4,
            "real FFT size must be a power of two >= 4"
        );
        let half = Fft::new(n / 2);
        let mut twiddles = Vec::with_capacity(n / 2);
        for k in 0..n / 2 {
            let angle = -2.0 * PI * k as f64 / n as f64;
            twiddles.push(Complex64::new(angle.cos(), angle.sin()));
        }
        Self { n, half, twiddles }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.n
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Number of complex bins produced by [`Self::forward`].
    #[inline]
    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Transforms `input` (length `n`) into `out` (length `n/2 + 1`).
    pub fn forward(&self, input: &[f64], out: &mut [Complex64]) {
        assert_eq!(input.len(), self.n);
        assert_eq!(out.len(), self.bins());
        let m = self.n / 2;
        let mut z: Vec<Complex64> = input
            .chunks_exact(2)
            .map(|pair| Complex64::new(pair[0], pair[1]))
            .collect();
        self.half.forward(&mut z);

        // X[k] = E[k] + W[k] * O[k] with
        // E[k] = (Z[k] + conj(Z[M-k])) / 2, O[k] = (Z[k] - conj(Z[M-k])) / (2i).
        for k in 0..m {
            let zk = z[k];
            let zmk = z[(m - k) % m].conj();
            let even = zk.add(zmk).scale(0.5);
            let odd = zk.sub(zmk).mul(Complex64::new(0.0, -0.5));
            out[k] = even.add(self.twiddles[k].mul(odd));
        }
        // Nyquist bin: E[0] - O[0].
        let z0 = z[0];
        out[m] = Complex64::new(z0.re - z0.im, 0.0);
    }

    /// Inverse transform of a conjugate-symmetric spectrum (length `n/2 + 1`).
    pub fn inverse(&self, spec: &[Complex64], out: &mut [f64]) {
        assert_eq!(spec.len(), self.bins());
        assert_eq!(out.len(), self.n);
        let m = self.n / 2;
        let mut z = vec![Complex64::ZERO; m];
        // k = 0 pairs the DC bin with the (separate) Nyquist bin.
        let nyquist = spec[m].re;
        z[0] = Complex64::new((spec[0].re + nyquist) * 0.5, (spec[0].re - nyquist) * 0.5);
        for k in 1..m {
            let xk = spec[k];
            let xmk = spec[m - k].conj();
            let even = xk.add(xmk).scale(0.5);
            let odd = xk.sub(even).mul(self.twiddles[k].conj());
            // z = E + i*O
            z[k] = even.add(Complex64::new(-odd.im, odd.re));
        }
        self.half.inverse(&mut z);
        for (i, v) in z.iter().enumerate() {
            out[2 * i] = v.re;
            if 2 * i + 1 < self.n {
                out[2 * i + 1] = v.im;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Naive O(n^2) DFT used as the reference implementation in tests.
    fn naive_dft(input: &[f64]) -> Vec<Complex64> {
        let n = input.len();
        (0..=n / 2)
            .map(|k| {
                let mut acc = Complex64::ZERO;
                for (i, &x) in input.iter().enumerate() {
                    let angle = -2.0 * PI * (k * i) as f64 / n as f64;
                    acc = acc.add(Complex64::new(angle.cos(), angle.sin()).scale(x));
                }
                acc
            })
            .collect()
    }

    #[test]
    fn complex_fft_roundtrip() {
        let n = 64;
        let fft = Fft::new(n);
        let mut buf: Vec<Complex64> = (0..n)
            .map(|i| Complex64::new((i as f64 * 0.1).sin(), (i as f64 * 0.05).cos()))
            .collect();
        let original = buf.clone();
        fft.forward(&mut buf);
        fft.inverse(&mut buf);
        for (a, b) in buf.iter().zip(original.iter()) {
            assert!((a.re - b.re).abs() < 1e-12, "{a:?} != {b:?}");
            assert!((a.im - b.im).abs() < 1e-12, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn real_fft_matches_naive_dft() {
        for &n in &[16usize, 64, 256, 1024] {
            let rfft = RealFft::new(n);
            let input: Vec<f64> = (0..n)
                .map(|i| (i as f64 * 0.37).sin() * 0.8 + (i as f64 * 1.7).cos() * 0.2)
                .collect();
            let mut out = vec![Complex64::ZERO; rfft.bins()];
            rfft.forward(&input, &mut out);
            let reference = naive_dft(&input);
            for (k, (a, b)) in out.iter().zip(reference.iter()).enumerate() {
                assert!((a.re - b.re).abs() < 1e-9, "n={n} k={k} re {a:?} vs {b:?}");
                assert!((a.im - b.im).abs() < 1e-9, "n={n} k={k} im {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn real_fft_roundtrip_is_exact() {
        for &n in &[16usize, 128, 1024] {
            let rfft = RealFft::new(n);
            let input: Vec<f64> = (0..n)
                .map(|i| ((i * 7919) % 101) as f64 / 101.0 - 0.5)
                .collect();
            let mut spec = vec![Complex64::ZERO; rfft.bins()];
            rfft.forward(&input, &mut spec);
            let mut back = vec![0.0; n];
            rfft.inverse(&spec, &mut back);
            for (a, b) in back.iter().zip(input.iter()) {
                assert!((a - b).abs() < 1e-12, "n={n}: {a} != {b}");
            }
        }
    }

    #[test]
    fn dc_and_nyquist_bins_are_correct() {
        let n = 8;
        let rfft = RealFft::new(n);
        let input = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
        let mut spec = vec![Complex64::ZERO; rfft.bins()];
        rfft.forward(&input, &mut spec);
        // Nyquist rate signal: all energy in the last bin.
        assert!(spec[0].norm() < 1e-12);
        assert!((spec[4].norm() - 8.0).abs() < 1e-12);
    }
}
