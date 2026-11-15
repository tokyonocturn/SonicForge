//! The processing pipeline: an ordered list of operations applied to planar
//! channel data.

use super::biquad::{Biquad, BiquadState, IirFilter, IirState};
use super::convolver::Convolver;
use super::delay::DelayLine;
use super::mixer::{apply_mix, MixAssignment};
use std::f64::consts::PI;

/// Bitset of channel indices (up to 64 channels, far beyond any Windows device).
pub type ChannelMask = u64;

/// Builds a mask from an iterator of channel indices.
pub fn mask_from(indices: impl IntoIterator<Item = usize>) -> ChannelMask {
    let mut mask = 0u64;
    for idx in indices {
        if idx < 64 {
            mask |= 1 << idx;
        }
    }
    mask
}

/// Iterates the channel indices selected by `mask` that exist in `channels`.
fn selected(mask: ChannelMask, channels: usize) -> impl Iterator<Item = usize> {
    (0..channels.min(64)).filter(move |c| mask & (1 << c) != 0)
}

/// A single processing stage.
#[derive(Clone, Debug)]
pub enum Op {
    /// Static gain in linear units.
    Gain { mask: ChannelMask, gain: f32 },
    /// A cascade of biquads (a single `Filter:` or a whole `GraphicEQ:`).
    Biquad {
        mask: ChannelMask,
        cascade: Vec<Biquad>,
        states: Vec<BiquadState>,
    },
    /// A generic IIR section from `Filter: ON IIR`.
    Iir {
        mask: ChannelMask,
        filter: IirFilter,
        states: Vec<IirState>,
    },
    /// FFT convolution, one convolver per selected channel.
    Convolver {
        mask: ChannelMask,
        convolvers: Vec<Convolver>,
        scratch: Vec<f32>,
    },
    /// Channel delay, one line per selected channel.
    Delay {
        mask: ChannelMask,
        lines: Vec<DelayLine>,
    },
    /// Channel copy / mix matrix.
    Mix { assignments: Vec<MixAssignment> },
}

impl Op {
    /// Number of biquad states the op needs for `channels` channels.
    fn prepare(&mut self, channels: usize) {
        match self {
            Op::Biquad { mask, cascade, states } => {
                let count = selected(*mask, channels).count() * cascade.len();
                states.resize(count, BiquadState::default());
            }
            Op::Iir { mask, filter, states } => {
                let count = selected(*mask, channels).count();
                let order = filter.order();
                states.resize_with(count, || IirState::with_order(order));
            }
            Op::Convolver { mask, convolvers, scratch } => {
                let count = selected(*mask, channels).count();
                if convolvers.len() == 1 && count > 1 {
                    let template = convolvers[0].clone();
                    convolvers.resize(count, template);
                }
                convolvers.truncate(count.max(1));
                if let Some(first) = convolvers.first() {
                    scratch.resize(first.block_size().max(1) * 8, 0.0);
                }
            }
            Op::Delay { mask, lines } => {
                let count = selected(*mask, channels).count();
                if lines.len() == 1 && count > 1 {
                    let template = lines[0].clone();
                    lines.resize(count, template);
                }
                lines.truncate(count.max(1));
            }
            Op::Gain { .. } | Op::Mix { .. } => {}
        }
    }

    fn reset(&mut self) {
        match self {
            Op::Biquad { states, .. } => states.iter_mut().for_each(|s| s.reset()),
            Op::Iir { states, .. } => states.iter_mut().for_each(|s| s.reset()),
            Op::Convolver { convolvers, .. } => convolvers.iter_mut().for_each(|c| c.reset()),
            Op::Delay { lines, .. } => lines.iter_mut().for_each(|l| l.reset()),
            Op::Gain { .. } | Op::Mix { .. } => {}
        }
    }
}

/// A complete processing chain for one sample rate.
#[derive(Clone, Debug)]
pub struct Pipeline {
    sample_rate: u32,
    channels: usize,
    ops: Vec<Op>,
}

impl Pipeline {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self { sample_rate, channels, ops: Vec::new() }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Appends an operation and prepares its per-channel state.
    pub fn push(&mut self, op: Op) {
        let mut op = op;
        op.prepare(self.channels);
        self.ops.push(op);
    }

    /// Re-prepares every op for `channels` channels. Extra channels start with
    /// cleared state.
    pub fn set_channels(&mut self, channels: usize) {
        self.channels = channels;
        for op in &mut self.ops {
            op.prepare(channels);
        }
    }

    /// Total latency added by the chain, in samples.
    pub fn latency_samples(&self) -> usize {
        self.ops
            .iter()
            .map(|op| match op {
                Op::Convolver { convolvers, .. } => {
                    convolvers.first().map(|c| c.latency_samples()).unwrap_or(0)
                }
                _ => 0,
            })
            .sum()
    }

    /// Clears all filter and convolver state. Call this when the stream is
    /// (re)started so that stale audio never leaks into a new stream.
    pub fn reset(&mut self) {
        for op in &mut self.ops {
            op.reset();
        }
    }

    /// Processes planar audio in place. All channels must have equal length.
    pub fn process(&mut self, buf: &mut [Vec<f32>]) {
        if buf.is_empty() {
            return;
        }
        // Guard against channel count changes between prepare and process.
        if buf.len() != self.channels {
            self.set_channels(buf.len());
        }
        for op in &mut self.ops {
            match op {
                Op::Gain { mask, gain } => {
                    for c in selected(*mask, buf.len()) {
                        for sample in buf[c].iter_mut() {
                            *sample *= *gain;
                        }
                    }
                }
                Op::Biquad { mask, cascade, states } => {
                    for (slot, c) in selected(*mask, buf.len()).enumerate() {
                        let base = slot * cascade.len();
                        if base + cascade.len() > states.len() {
                            continue;
                        }
                        for (f, bq) in cascade.iter().enumerate() {
                            states[base + f].process_slice(bq, &mut buf[c]);
                            states[base + f].sanitize();
                        }
                    }
                }
                Op::Iir { mask, filter, states } => {
                    for (slot, c) in selected(*mask, buf.len()).enumerate() {
                        if let Some(state) = states.get_mut(slot) {
                            state.process_slice(filter, &mut buf[c]);
                        }
                    }
                }
                Op::Convolver { mask, convolvers, scratch } => {
                    for (slot, c) in selected(*mask, buf.len()).enumerate() {
                        if slot >= convolvers.len() {
                            break;
                        }
                        let frames = buf[c].len();
                        if scratch.len() < frames {
                            scratch.resize(frames, 0.0);
                        }
                        scratch[..frames].copy_from_slice(&buf[c][..frames]);
                        convolvers[slot].process(&scratch[..frames], &mut buf[c][..frames]);
                    }
                }
                Op::Delay { mask, lines } => {
                    for (slot, c) in selected(*mask, buf.len()).enumerate() {
                        if let Some(line) = lines.get_mut(slot) {
                            line.process(&mut buf[c]);
                        }
                    }
                }
                Op::Mix { assignments } => apply_mix(buf, assignments),
            }
        }
    }

    /// Magnitude response of the chain in dB, ignoring the flat parts
    /// (gain, delay and mix which cannot change the shape of the spectrum).
    pub fn frequency_response(&mut self, freqs: &[f64]) -> Vec<f64> {
        let fs = self.sample_rate as f64;
        let mut out = vec![0.0f64; freqs.len()];
        for (i, &f) in freqs.iter().enumerate() {
            let w = 2.0 * PI * f / fs;
            let mut mag = 1.0f64;
            for op in self.ops.iter_mut() {
                match op {
                    Op::Biquad { cascade, .. } => {
                        for bq in cascade {
                            mag *= bq.response(w).norm();
                        }
                    }
                    Op::Iir { filter, .. } => {
                        mag *= filter.clone().response(w).norm();
                    }
                    Op::Convolver { convolvers, .. } => {
                        if let Some(c) = convolvers.first_mut() {
                            let db = c.response_db(f, fs);
                            mag *= 10f64.powf(db / 20.0);
                        }
                    }
                    Op::Gain { gain, .. } => mag *= *gain as f64,
                    _ => {}
                }
            }
            out[i] = 20.0 * mag.max(1e-12).log10();
        }
        out
    }

    /// Total static gain of the chain (useful for headroom calculations).
    pub fn static_gain(&self) -> f64 {
        self.ops
            .iter()
            .map(|op| match op {
                Op::Gain { gain, .. } => *gain as f64,
                _ => 1.0,
            })
            .product()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::filters::{design, Bandwidth, FilterKind, FilterSpec};

    fn peak(freq: f64, gain: f64) -> Biquad {
        let mut spec = FilterSpec::new(FilterKind::Peak, freq);
        spec.gain_db = gain;
        spec.bandwidth = Bandwidth::Q(1.0);
        design(&spec, 48000.0).unwrap()
    }

    #[test]
    fn gain_op_scales_selected_channels_only() {
        let mut p = Pipeline::new(48000, 2);
        p.push(Op::Gain { mask: 0b01, gain: 0.5 });
        let mut buf = vec![vec![1.0f32; 8], vec![1.0f32; 8]];
        p.process(&mut buf);
        assert!(buf[0].iter().all(|v| (v - 0.5).abs() < 1e-6));
        assert!(buf[1].iter().all(|v| (v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn biquad_states_are_independent_per_channel() {
        let mut p = Pipeline::new(48000, 2);
        p.push(Op::Biquad {
            mask: 0b11,
            cascade: vec![peak(1000.0, 12.0)],
            states: Vec::new(),
        });
        // Only the left channel carries signal; the right channel must stay
        // silent, proving that state is not shared.
        let mut buf = vec![
            (0..256).map(|i| (i as f32 * 0.2).sin()).collect::<Vec<_>>(),
            vec![0.0f32; 256],
        ];
        p.process(&mut buf);
        assert!(buf[1].iter().all(|v| v.abs() < 1e-9));
    }

    #[test]
    fn empty_pipeline_is_transparent() {
        let mut p = Pipeline::new(48000, 2);
        let mut buf = vec![vec![0.3f32; 16], vec![-0.2f32; 16]];
        let original = buf.clone();
        p.process(&mut buf);
        assert_eq!(buf, original);
        assert_eq!(p.latency_samples(), 0);
    }

    #[test]
    fn frequency_response_matches_filter_design() {
        let mut p = Pipeline::new(48000, 1);
        let bq = peak(1000.0, 6.0);
        p.push(Op::Biquad { mask: 0b1, cascade: vec![bq], states: Vec::new() });
        let resp = p.frequency_response(&[1000.0, 100.0]);
        assert!((resp[0] - bq.magnitude_db(1000.0, 48000.0)).abs() < 1e-6);
        assert!((resp[1] - bq.magnitude_db(100.0, 48000.0)).abs() < 1e-6);
    }

    #[test]
    fn mix_changes_channel_count_safely() {
        let mut p = Pipeline::new(48000, 2);
        p.push(Op::Mix {
            assignments: vec![MixAssignment {
                target: 0,
                terms: vec![
                    crate::dsp::mixer::MixTerm { gain: 0.5, source: Some(0), constant: 0.0 },
                    crate::dsp::mixer::MixTerm { gain: 0.5, source: Some(1), constant: 0.0 },
                ],
            }],
        });
        let mut buf = vec![vec![1.0f32; 4], vec![0.0f32; 4]];
        p.process(&mut buf);
        assert!(buf[0].iter().all(|v| (v - 0.5).abs() < 1e-6));
    }
}
