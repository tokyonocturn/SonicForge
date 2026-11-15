//! Uniformly partitioned FFT convolution.
//!
//! The impulse response is split into `block` sized partitions. Every input
//! block is transformed once, multiplied with each partition spectrum and
//! accumulated in the frequency domain before a single inverse transform. This
//! keeps CPU usage proportional to `IR length / block` rather than to the IR
//! length directly, which is what makes long room-correction filters practical.
//!
//! Latency is exactly one block: the convolver must collect a complete input
//! block before it can produce the corresponding output block.

use super::fft::{Complex64, RealFft};

/// Default partition size. 512 samples is 10.7 ms at 48 kHz.
pub const DEFAULT_BLOCK: usize = 512;

/// Block based convolution with a fixed impulse response.
#[derive(Clone, Debug)]
pub struct Convolver {
    block: usize,
    fft: RealFft,
    ir: Vec<f32>,
    partitions: Vec<Vec<Complex64>>,
    /// `[previous block | current block]` used by overlap-save.
    history: Vec<f64>,
    /// Samples accumulated towards the next processing block.
    input: Vec<f32>,
    /// Samples produced but not yet handed to the caller.
    pending: Vec<f32>,
    pending_head: usize,
    /// Ring of the last `partitions.len()` input spectra. Partition `p` must be
    /// multiplied with the spectrum of the block `p` blocks ago, otherwise
    /// every partition would contribute at the wrong time offset.
    spectra: Vec<Vec<Complex64>>,
    spectrum_index: usize,
    spectra_filled: usize,
    spectrum: Vec<Complex64>,
    accumulator: Vec<Complex64>,
    time_out: Vec<f64>,
    response: Vec<f64>,
}

impl Convolver {
    /// Creates a convolver. `block` is rounded up to a power of two and clamped
    /// to `[64, 8192]`. An empty impulse response produces silence.
    pub fn new(impulse_response: &[f32], block: usize) -> Self {
        let block = block.clamp(64, 8192).next_power_of_two();
        let fft = RealFft::new(block * 2);
        let ir = impulse_response.to_vec();
        let partition_count = ir.len().div_ceil(block).max(1);
        let mut partitions = Vec::with_capacity(partition_count);
        for p in 0..partition_count {
            let start = p * block;
            let end = (start + block).min(ir.len());
            let mut buf = vec![0.0f64; block * 2];
            if start < end {
                for (i, &v) in ir[start..end].iter().enumerate() {
                    buf[i] = v as f64;
                }
            }
            let mut spec = vec![Complex64::ZERO; fft.bins()];
            fft.forward(&buf, &mut spec);
            partitions.push(spec);
        }
        Self {
            block,
            fft,
            ir,
            partitions,
            history: vec![0.0; block * 2],
            input: Vec::with_capacity(block),
            pending: vec![0.0; block],
            pending_head: 0,
            spectra: vec![vec![Complex64::ZERO; block + 1]; partition_count],
            spectrum_index: 0,
            spectra_filled: 0,
            spectrum: vec![Complex64::ZERO; block + 1],
            accumulator: vec![Complex64::ZERO; block + 1],
            time_out: vec![0.0; block * 2],
            response: Vec::new(),
        }
    }

    pub fn block_size(&self) -> usize {
        self.block
    }

    /// Latency introduced by block processing, in samples.
    pub fn latency_samples(&self) -> usize {
        self.block
    }

    pub fn impulse_len(&self) -> usize {
        self.ir.len()
    }

    pub fn impulse(&self) -> &[f32] {
        &self.ir
    }

    /// Absolute magnitude response of the impulse response in dB.
    pub fn response_db(&mut self, freq: f64, sample_rate: f64) -> f64 {
        if self.response.is_empty() {
            let size = (self.ir.len() * 2).next_power_of_two().clamp(4096, 65536);
            let mut padded = vec![0.0f64; size];
            for (i, &v) in self.ir.iter().enumerate().take(size) {
                padded[i] = v as f64;
            }
            let fft = RealFft::new(size);
            let mut spec = vec![Complex64::ZERO; fft.bins()];
            fft.forward(&padded, &mut spec);
            self.response = spec.iter().map(|c| c.norm()).collect();
            // Remember the resolution used for interpolation.
            self.response.insert(0, size as f64);
        }
        let size = self.response[0];
        let bin = freq / sample_rate * size;
        let idx = bin.round().max(0.0) as usize;
        if idx + 1 >= self.response.len() {
            return -120.0;
        }
        let mag = self.response[idx + 1];
        20.0 * mag.max(1e-12).log10()
    }

    /// Processes `input` into `output`. Both slices must have the same length.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (i, &sample) in input.iter().enumerate() {
            self.input.push(sample);
            if self.input.len() == self.block {
                self.process_block();
                self.input.clear();
            }
            output[i] = if self.pending_head < self.pending.len() {
                let v = self.pending[self.pending_head];
                self.pending_head += 1;
                v
            } else {
                0.0
            };
        }
        if self.pending_head > 0 {
            self.pending.drain(..self.pending_head);
            self.pending_head = 0;
        }
    }

    /// Clears all internal state. The steady state latency is preserved.
    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|v| *v = 0.0);
        self.spectra.iter_mut().for_each(|s| s.iter_mut().for_each(|v| *v = Complex64::ZERO));
        self.spectrum_index = 0;
        self.spectra_filled = 0;
        self.input.clear();
        self.pending.clear();
        self.pending.resize(self.block, 0.0);
        self.pending_head = 0;
    }

    fn process_block(&mut self) {
        let block = self.block;
        self.history.copy_within(block..block * 2, 0);
        for (i, &v) in self.input.iter().enumerate() {
            self.history[block + i] = v as f64;
        }
        self.fft.forward(&self.history, &mut self.spectrum);
        // Store the current spectrum in the ring.
        let slots = self.spectra.len();
        self.spectra[self.spectrum_index].copy_from_slice(&self.spectrum);
        let newest = self.spectrum_index;
        self.spectrum_index = (self.spectrum_index + 1) % slots;
        self.spectra_filled = (self.spectra_filled + 1).min(slots);

        self.accumulator.iter_mut().for_each(|v| *v = Complex64::ZERO);
        for (p, partition) in self.partitions.iter().enumerate() {
            if p >= self.spectra_filled {
                break;
            }
            let slot = (newest + slots - p) % slots;
            for (acc, (x, h)) in self
                .accumulator
                .iter_mut()
                .zip(self.spectra[slot].iter().zip(partition.iter()))
            {
                *acc = acc.add(x.mul(*h));
            }
        }
        self.fft.inverse(&self.accumulator, &mut self.time_out);
        // Overlap-save: the second half of the inverse transform is valid.
        self.pending
            .extend(self.time_out[block..block * 2].iter().map(|&v| v as f32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference implementation: direct convolution.
    fn direct(input: &[f32], ir: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; input.len()];
        for n in 0..input.len() {
            let mut acc = 0.0f64;
            for (k, &h) in ir.iter().enumerate() {
                if n >= k {
                    acc += input[n - k] as f64 * h as f64;
                }
            }
            out[n] = acc as f32;
        }
        out
    }

    #[test]
    fn delta_impulse_is_a_delayed_passthrough() {
        let mut ir = vec![0.0f32; 64];
        ir[0] = 1.0;
        let mut conv = Convolver::new(&ir, 64);
        let input: Vec<f32> = (0..1024).map(|i| ((i as f32) * 0.05).sin()).collect();
        let mut output = vec![0.0f32; input.len()];
        conv.process(&input, &mut output);
        let latency = conv.latency_samples();
        for i in latency..input.len() {
            assert!((output[i] - input[i - latency]).abs() < 1e-4, "sample {i}");
        }
    }

    #[test]
    fn matches_direct_convolution_for_short_ir() {
        let ir: Vec<f32> = vec![0.3, -0.2, 0.5, 0.1, -0.05, 0.02];
        let mut conv = Convolver::new(&ir, 64);
        let input: Vec<f32> = (0..512).map(|i| ((i as f32) * 0.11).sin() * 0.5).collect();
        let mut output = vec![0.0f32; input.len()];
        conv.process(&input, &mut output);
        let reference = direct(&input, &ir);
        let latency = conv.latency_samples();
        let mut worst = 0.0f32;
        for i in latency..input.len() {
            worst = worst.max((output[i] - reference[i - latency]).abs());
        }
        assert!(worst < 1e-4, "worst error {worst}");
    }

    #[test]
    fn handles_ir_longer_than_one_partition() {
        // 3000 tap IR spans several 256 sample partitions.
        let mut ir = vec![0.0f32; 3000];
        for (i, v) in ir.iter_mut().enumerate() {
            *v = (-(i as f32) / 400.0).exp() * 0.01;
        }
        let mut conv = Convolver::new(&ir, 256);
        let input: Vec<f32> = (0..8192).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let mut output = vec![0.0f32; input.len()];
        conv.process(&input, &mut output);
        let latency = conv.latency_samples();
        for k in 0..ir.len() {
            assert!((output[k + latency] - ir[k]).abs() < 1e-4, "tap {k}");
        }
    }

    #[test]
    fn arbitrary_buffer_sizes_preserve_the_signal() {
        let mut ir = vec![0.0f32; 32];
        ir[0] = 0.5;
        ir[7] = 0.25;
        let mut conv = Convolver::new(&ir, 128);
        // Feed the same signal in irregular chunks.
        let total = 1000usize;
        let input: Vec<f32> = (0..total).map(|i| ((i as f32) * 0.03).sin()).collect();
        let mut output = vec![0.0f32; total];
        let mut pos = 0;
        for chunk in [13usize, 200, 7, 300, 480] {
            let end = (pos + chunk).min(total);
            if pos >= end {
                break;
            }
            conv.process(&input[pos..end], &mut output[pos..end]);
            pos = end;
        }
        if pos < total {
            conv.process(&input[pos..], &mut output[pos..]);
        }
        let reference = direct(&input, &ir);
        let latency = 128;
        for i in latency..total {
            assert!((output[i] - reference[i - latency]).abs() < 1e-3, "sample {i}");
        }
    }

    #[test]
    fn response_of_delta_is_flat() {
        let mut ir = vec![0.0f32; 16];
        ir[0] = 1.0;
        let mut conv = Convolver::new(&ir, 64);
        for f in [100.0, 1000.0, 10000.0] {
            let db = conv.response_db(f, 48000.0);
            assert!(db.abs() < 0.01, "{f} Hz -> {db} dB");
        }
    }

    #[test]
    fn reset_clears_tail_without_changing_latency() {
        let mut ir = vec![0.0f32; 16];
        ir[0] = 1.0;
        let mut conv = Convolver::new(&ir, 64);
        let input = vec![1.0f32; 512];
        let mut output = vec![0.0f32; 512];
        conv.process(&input, &mut output);
        conv.reset();
        conv.process(&vec![0.0f32; 512], &mut output);
        assert!(output.iter().all(|v| v.abs() < 1e-6));
        assert_eq!(conv.latency_samples(), 64);
    }
}
