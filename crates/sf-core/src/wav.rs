//! Minimal RIFF/WAVE reader and writer used for impulse responses.
//!
//! Supported input: PCM 8/16/24/32-bit, IEEE float 32/64-bit, and
//! `WAVE_FORMAT_EXTENSIBLE` wrappers around those formats. Other container
//! formats supported by the reference implementation (FLAC, Ogg, ...) are not
//! supported yet and produce a clear diagnostic instead of silent silence.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

/// An audio file decoded into planar float channels.
#[derive(Clone, Debug)]
pub struct WaveAudio {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl WaveAudio {
    pub fn frames(&self) -> usize {
        self.channels.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Normalised mono downmix, used for display and diagnostics.
    pub fn mono(&self) -> Vec<f32> {
        let n = self.frames();
        if self.channels.is_empty() || n == 0 {
            return Vec::new();
        }
        let gain = 1.0 / self.channels.len() as f32;
        let mut out = vec![0.0f32; n];
        for ch in &self.channels {
            for (o, &v) in out.iter_mut().zip(ch.iter()) {
                *o += v * gain;
            }
        }
        out
    }
}

/// Errors produced while decoding audio files.
#[derive(Debug)]
pub enum WavError {
    Io(std::io::Error),
    Invalid(String),
    Unsupported(String),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavError::Io(e) => write!(f, "I/O error: {e}"),
            WavError::Invalid(m) => write!(f, "invalid WAVE file: {m}"),
            WavError::Unsupported(m) => write!(f, "unsupported audio format: {m}"),
        }
    }
}

impl std::error::Error for WavError {}

impl From<std::io::Error> for WavError {
    fn from(e: std::io::Error) -> Self {
        WavError::Io(e)
    }
}

fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads a WAVE file from disk.
pub fn read_wave(path: &Path) -> Result<WaveAudio, WavError> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    parse_wave(&data)
}

/// Parses an in-memory WAVE file.
pub fn parse_wave(data: &[u8]) -> Result<WaveAudio, WavError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::Invalid("missing RIFF/WAVE header".into()));
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    let mut payload: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = le_u32(data, pos + 4) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(data.len());
        let body = &data[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(WavError::Invalid("fmt chunk too short".into()));
                }
                let mut tag = le_u16(body, 0);
                let channels = le_u16(body, 2);
                let rate = le_u32(body, 4);
                let bits = le_u16(body, 14);
                if tag == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: the real format is in the GUID.
                    if body.len() < 40 {
                        return Err(WavError::Invalid("extensible fmt chunk too short".into()));
                    }
                    tag = le_u16(body, 24);
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => payload = Some(body),
            _ => {}
        }
        pos = body_start + size + (size & 1);
    }

    let (tag, channels, rate, bits) = fmt.ok_or_else(|| WavError::Invalid("missing fmt chunk".into()))?;
    let payload = payload.ok_or_else(|| WavError::Invalid("missing data chunk".into()))?;
    if channels == 0 {
        return Err(WavError::Invalid("zero channels".into()));
    }
    if rate == 0 {
        return Err(WavError::Invalid("zero sample rate".into()));
    }
    let channels = channels as usize;
    let bytes_per_sample = (bits as usize + 7) / 8;
    if bytes_per_sample == 0 {
        return Err(WavError::Invalid("zero bit depth".into()));
    }
    let frame_bytes = bytes_per_sample * channels;
    if frame_bytes == 0 {
        return Err(WavError::Invalid("invalid block alignment".into()));
    }
    let frames = payload.len() / frame_bytes;
    let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];

    let decode = |chunk: &[u8]| -> f32 {
        match (tag, bits) {
            (1, 8) => (chunk[0] as f32 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0,
            (1, 24) => {
                let v = ((chunk[2] as i32) << 24 | (chunk[1] as i32) << 16 | (chunk[0] as i32) << 8) >> 8;
                v as f32 / 8388608.0
            }
            (1, 32) => i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f32 / 2147483648.0,
            (3, 32) => f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            (3, 64) => f64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]) as f32,
            (1, b) => {
                // Any other integer depth is not valid PCM.
                let _ = b;
                0.0
            }
            (t, b) => {
                let _ = (t, b);
                0.0
            }
        }
    };

    let supported = matches!((tag, bits), (1, 8) | (1, 16) | (1, 24) | (1, 32) | (3, 32) | (3, 64));
    if !supported {
        return Err(WavError::Unsupported(format!(
            "format tag {tag} with {bits} bits per sample"
        )));
    }

    for frame in 0..frames {
        let base = frame * frame_bytes;
        for ch in 0..channels {
            let start = base + ch * bytes_per_sample;
            out[ch].push(decode(&payload[start..start + bytes_per_sample]));
        }
    }

    Ok(WaveAudio { sample_rate: rate, channels: out })
}

/// Writes 32-bit float WAVE data. Used by tests and the diagnostics exporter.
pub fn write_wave_f32(path: &Path, sample_rate: u32, channels: &[Vec<f32>]) -> std::io::Result<()> {
    let channel_count = channels.len().max(1);
    let frames = channels.first().map(|c| c.len()).unwrap_or(0);
    let bits = 32u16;
    let block_align = channel_count as u16 * (bits / 8);
    let data_bytes = frames * channel_count * 4;
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);
    w.write_all(b"RIFF")?;
    w.write_all(&((36 + data_bytes) as u32).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?;
    w.write_all(&(channel_count as u16).to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * block_align as u32).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&(data_bytes as u32).to_le_bytes())?;
    for frame in 0..frames {
        for ch in channels.iter() {
            let v = ch.get(frame).copied().unwrap_or(0.0);
            w.write_all(&v.to_le_bytes())?;
        }
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_float_wave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ir.wav");
        let left: Vec<f32> = (0..100).map(|i| (i as f32 * 0.1).sin()).collect();
        let right: Vec<f32> = (0..100).map(|i| (i as f32 * 0.2).cos()).collect();
        write_wave_f32(&path, 48000, &[left.clone(), right.clone()]).unwrap();
        let audio = read_wave(&path).unwrap();
        assert_eq!(audio.sample_rate, 48000);
        assert_eq!(audio.channel_count(), 2);
        assert_eq!(audio.frames(), 100);
        for i in 0..100 {
            assert!((audio.channels[0][i] - left[i]).abs() < 1e-7);
            assert!((audio.channels[1][i] - right[i]).abs() < 1e-7);
        }
    }

    #[test]
    fn rejects_non_wave_data() {
        let err = parse_wave(b"not a wave file at all").unwrap_err();
        assert!(matches!(err, WavError::Invalid(_)));
    }

    #[test]
    fn decodes_sixteen_bit_pcm() {
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&40u32.to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes()); // PCM
        data.extend_from_slice(&1u16.to_le_bytes()); // mono
        data.extend_from_slice(&44100u32.to_le_bytes());
        data.extend_from_slice(&88200u32.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&16384i16.to_le_bytes());
        data.extend_from_slice(&(-16384i16).to_le_bytes());
        let audio = parse_wave(&data).unwrap();
        assert_eq!(audio.sample_rate, 44100);
        assert_eq!(audio.channels[0].len(), 2);
        assert!((audio.channels[0][0] - 0.5).abs() < 1e-4);
        assert!((audio.channels[0][1] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn mono_downmix_averages_channels() {
        let audio = WaveAudio {
            sample_rate: 48000,
            channels: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        };
        let mono = audio.mono();
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
    }
}
