//! Graphic equaliser design.
//!
//! The configuration format describes a graphic EQ as a list of
//! `frequency gain` control points whose gains are interpolated linearly on a
//! logarithmic frequency axis. SonicForge realises the curve as a cascade of
//! peaking filters, one per control point, and then refines the individual band
//! gains so that the *summed* response hits the requested curve. Without the
//! refinement pass neighbouring bands would pile up and overshoot the target by
//! several dB for closely spaced bands.

use super::biquad::Biquad;
use super::filters::{design, Bandwidth, FilterKind, FilterSpec};
use std::f64::consts::PI;

/// Maximum number of control points accepted for a single graphic EQ.
pub const MAX_POINTS: usize = 512;
/// Maximum deviation from the requested curve targeted by the refinement pass.
const MAX_GAIN_DB: f64 = 36.0;
/// Number of refinement iterations.
const REFINE_ITERATIONS: usize = 6;

/// Designs a biquad cascade approximating `points` (`(frequency_hz, gain_db)`).
///
/// Points must be sorted by frequency; duplicates and points outside
/// `(0, nyquist)` are dropped. The returned cascade is empty when no point is
/// usable.
pub fn design_graphic_eq(points: &[(f64, f64)], sample_rate: f64) -> Vec<Biquad> {
    let mut pts: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|(f, g)| f.is_finite() && f.is_finite() && *f > 0.0 && *f < sample_rate / 2.0 && g.is_finite())
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
    if pts.len() > MAX_POINTS {
        pts.truncate(MAX_POINTS);
    }
    if pts.is_empty() {
        return Vec::new();
    }
    if pts.len() == 1 {
        // A single control point is a broadband gain: implement it as a very
        // wide peak filter so the whole spectrum follows the request.
        let mut spec = FilterSpec::new(FilterKind::Peak, pts[0].0.clamp(10.0, sample_rate * 0.45));
        spec.gain_db = pts[0].1;
        spec.bandwidth = Bandwidth::Q(0.1);
        return design(&spec, sample_rate).into_iter().collect();
    }

    // Band edges are the geometric midpoints between adjacent control points.
    let n = pts.len();
    let mut qs = vec![std::f64::consts::FRAC_1_SQRT_2; n];
    for i in 0..n {
        let lo = if i == 0 { pts[0].0 / 2f64.powf(1.0 / 6.0) } else { (pts[i - 1].0 * pts[i].0).sqrt() };
        let hi = if i == n - 1 {
            (pts[n - 1].0 * 2f64.powf(1.0 / 6.0)).min(sample_rate * 0.499)
        } else {
            (pts[i].0 * pts[i + 1].0).sqrt()
        };
        let width = (hi - lo).max(1e-6);
        qs[i] = (pts[i].0 / width).clamp(0.3, 30.0);
    }

    let mut gains: Vec<f64> = pts.iter().map(|p| p.1.clamp(-MAX_GAIN_DB, MAX_GAIN_DB)).collect();
    let mut cascade: Vec<Biquad> = Vec::with_capacity(n);

    for _ in 0..REFINE_ITERATIONS {
        cascade.clear();
        for i in 0..n {
            let mut spec = FilterSpec::new(FilterKind::Peak, pts[i].0);
            spec.gain_db = gains[i];
            spec.bandwidth = Bandwidth::Q(qs[i]);
            match design(&spec, sample_rate) {
                Some(bq) => cascade.push(bq),
                None => cascade.push(Biquad::IDENTITY),
            }
        }
        // Measure the summed response at every control point and correct the
        // individual band gains by the residual.
        let mut worst = 0.0f64;
        for i in 0..n {
            let w = 2.0 * PI * pts[i].0 / sample_rate;
            let mut mag = 1.0f64;
            for bq in &cascade {
                mag *= bq.response(w).norm();
            }
            let actual_db = 20.0 * mag.max(1e-12).log10();
            let error = pts[i].1 - actual_db;
            worst = worst.max(error.abs());
            gains[i] = (gains[i] + error).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        }
        if worst < 0.01 {
            break;
        }
    }

    // Final cascade with the refined gains.
    cascade.clear();
    for i in 0..n {
        let mut spec = FilterSpec::new(FilterKind::Peak, pts[i].0);
        spec.gain_db = gains[i];
        spec.bandwidth = Bandwidth::Q(qs[i]);
        if let Some(bq) = design(&spec, sample_rate) {
            cascade.push(bq);
        }
    }
    cascade
}

/// Evaluates the interpolated target curve of a graphic EQ at `freq`.
///
/// This mirrors what a user sees in an editor: linear interpolation on a
/// logarithmic frequency axis, flat outside the control points.
pub fn target_curve_db(points: &[(f64, f64)], freq: f64) -> f64 {
    let mut pts: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|(f, _)| f.is_finite() && *f > 0.0)
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if pts.is_empty() {
        return 0.0;
    }
    if freq <= pts[0].0 {
        return 0.0;
    }
    if freq >= pts[pts.len() - 1].0 {
        return 0.0;
    }
    for pair in pts.windows(2) {
        let (f0, g0) = pair[0];
        let (f1, g1) = pair[1];
        if freq >= f0 && freq <= f1 {
            let t = (freq.ln() - f0.ln()) / (f1.ln() - f0.ln());
            return g0 + (g1 - g0) * t;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    fn cascade_db(cascade: &[Biquad], freq: f64) -> f64 {
        let w = 2.0 * PI * freq / FS;
        cascade.iter().map(|b| b.response(w).norm()).product::<f64>().max(1e-12).log10() * 20.0
    }

    #[test]
    fn iso_graphic_eq_hits_all_control_points() {
        let points = [
            (25.0, 6.0),
            (40.0, 4.5),
            (63.0, 3.0),
            (100.0, 1.5),
            (160.0, 0.0),
            (250.0, 0.0),
            (400.0, 0.0),
            (630.0, 0.0),
            (1000.0, 0.0),
            (1600.0, 0.0),
            (2500.0, 0.0),
            (4000.0, 0.0),
            (6300.0, 1.5),
            (10000.0, 3.0),
            (16000.0, 3.0),
        ];
        let cascade = design_graphic_eq(&points, FS);
        assert_eq!(cascade.len(), points.len());
        for &(f, g) in &points {
            let measured = cascade_db(&cascade, f);
            assert!((measured - g).abs() < 0.5, "{f} Hz: measured {measured:.2} dB, want {g} dB");
        }
    }

    #[test]
    fn dense_custom_curve_is_tracked_within_one_db() {
        let points: Vec<(f64, f64)> = vec![
            (20.0, 0.0),
            (25.0, -1.75),
            (30.0, -3.2),
            (35.0, -4.15),
            (40.0, -4.9),
            (45.0, -5.55),
            (50.0, -6.1),
            (60.0, -6.9),
            (70.0, -7.4),
            (80.0, -7.8),
            (90.0, -8.1),
            (100.0, -8.3),
        ];
        let cascade = design_graphic_eq(&points, FS);
        for &(f, g) in &points {
            let measured = cascade_db(&cascade, f);
            assert!((measured - g).abs() < 1.0, "{f} Hz: measured {measured:.2} dB, want {g} dB");
        }
    }

    #[test]
    fn flat_graphic_eq_is_flat() {
        let points: Vec<(f64, f64)> = [25.0, 100.0, 1000.0, 10000.0].iter().map(|f| (*f, 0.0)).collect();
        let cascade = design_graphic_eq(&points, FS);
        for f in [50.0, 500.0, 2000.0, 15000.0] {
            assert!(cascade_db(&cascade, f).abs() < 1e-9);
        }
    }

    #[test]
    fn out_of_range_points_are_dropped() {
        let points = [(0.0, 3.0), (24000.0, 3.0), (1000.0, 3.0)];
        let cascade = design_graphic_eq(&points, FS);
        assert_eq!(cascade.len(), 1);
    }

    #[test]
    fn target_curve_interpolates_logarithmically() {
        let pts = [(100.0, 0.0), (1000.0, 10.0)];
        // Geometric mean frequency should sit exactly half way.
        let mid = target_curve_db(&pts, 316.227_766_016_837_9);
        assert!((mid - 5.0).abs() < 1e-6, "mid = {mid}");
        assert_eq!(target_curve_db(&pts, 50.0), 0.0);
        assert_eq!(target_curve_db(&pts, 5000.0), 0.0);
    }
}
