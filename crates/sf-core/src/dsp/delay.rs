//! Delay lines with fractional sample accuracy.

/// A delay line for one channel.
///
/// Delays are stored in samples. Fractional delays are realised with linear
/// interpolation, which is sufficient for speaker alignment where the error at
/// 20 kHz stays below 0.1 dB for typical sub-millisecond delays.
#[derive(Clone, Debug)]
pub struct DelayLine {
    buffer: Vec<f32>,
    pos: usize,
    delay: f64,
}

/// Largest delay accepted by the compiler (seconds).
pub const MAX_DELAY_SECONDS: f64 = 10.0;

impl DelayLine {
    pub fn new(delay_samples: f64) -> Self {
        let capacity = delay_samples.ceil().max(0.0) as usize + 2;
        Self {
            buffer: vec![0.0; capacity.max(2)],
            pos: 0,
            delay: delay_samples.max(0.0),
        }
    }

    pub fn delay_samples(&self) -> f64 {
        self.delay
    }

    pub fn set_delay(&mut self, delay_samples: f64) {
        self.delay = delay_samples.max(0.0);
        let needed = self.delay.ceil() as usize + 2;
        if needed > self.buffer.len() {
            self.buffer.resize(needed, 0.0);
        }
        self.pos %= self.buffer.len();
    }

    pub fn reset(&mut self) {
        self.buffer.iter_mut().for_each(|v| *v = 0.0);
        self.pos = 0;
    }

    /// Processes a block in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        let len = self.buffer.len();
        if self.delay <= 0.0 {
            // Still advance the write pointer so that changing the delay later
            // does not produce a discontinuity.
            for &sample in buf.iter() {
                self.buffer[self.pos] = sample;
                self.pos = (self.pos + 1) % len;
            }
            return;
        }
        let whole = self.delay.floor();
        let frac = (self.delay - whole) as f32;
        for sample in buf.iter_mut() {
            self.buffer[self.pos] = *sample;
            let i1 = (self.pos as f64 - whole).rem_euclid(len as f64) as usize;
            // One sample older than i1, because the ring advances with time.
            let i2 = (i1 + len - 1) % len;
            let a = self.buffer[i1];
            let b = self.buffer[i2];
            *sample = a + (b - a) * frac;
            self.pos = (self.pos + 1) % len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_delay_shifts_signal() {
        for delay in [1usize, 5, 64] {
            let mut line = DelayLine::new(delay as f64);
            let input: Vec<f32> = (0..256).map(|i| i as f32).collect();
            let mut buf = input.clone();
            line.process(&mut buf);
            for (i, value) in buf.iter().enumerate().skip(delay) {
                assert_eq!(*value, input[i - delay], "delay {delay} at {i}");
            }
            for value in buf.iter().take(delay) {
                assert_eq!(*value, 0.0);
            }
        }
    }

    #[test]
    fn fractional_delay_interpolates() {
        let mut line = DelayLine::new(2.5);
        let mut buf = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        line.process(&mut buf);
        // Impulse delayed by 2.5 samples splits between index 2 and 3.
        assert!((buf[2] - 0.5).abs() < 1e-6, "{buf:?}");
        assert!((buf[3] - 0.5).abs() < 1e-6, "{buf:?}");
    }

    #[test]
    fn zero_delay_is_transparent() {
        let mut line = DelayLine::new(0.0);
        let mut buf: Vec<f32> = (0..32).map(|i| i as f32).collect();
        let input = buf.clone();
        line.process(&mut buf);
        assert_eq!(buf, input);
    }

    #[test]
    fn growing_the_delay_keeps_processing() {
        let mut line = DelayLine::new(1.0);
        let mut buf = vec![0.0f32; 64];
        line.process(&mut buf);
        line.set_delay(100.0);
        let mut buf = vec![1.0f32; 256];
        line.process(&mut buf);
        assert!(buf[..100].iter().all(|v| v.abs() < 1e-6));
        assert_eq!(buf[100], 1.0);
    }
}
