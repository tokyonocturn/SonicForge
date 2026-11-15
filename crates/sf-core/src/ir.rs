//! Impulse response loading.
//!
//! `Convolution:` accepts more than RIFF/WAVE: FLAC, Ogg/Vorbis, raw little
//! endian float dumps and plain text are all supported. Everything goes through
//! this module so the compiler has one code path, and so the decoders can be
//! tested without a sound card or a Windows API.

use crate::wav::{read_wave, WavError};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Sample rate assumed for raw float and text impulse responses that do not
/// declare one. Raw formats have no header, so this is the documented default.
pub const DEFAULT_RAW_SAMPLE_RATE: u32 = 48000;

/// An impulse response decoded into planar float channels.
#[derive(Clone, Debug)]
pub struct ImpulseResponse {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl ImpulseResponse {
    pub fn frames(&self) -> usize {
        self.channels.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

/// File formats the loader understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImpulseFormat {
    Wave,
    Flac,
    OggVorbis,
    RawF32,
    RawF64,
    Text,
}

impl ImpulseFormat {
    /// All formats accepted by `Convolution:`.
    pub const ALL: [ImpulseFormat; 6] = [
        ImpulseFormat::Wave,
        ImpulseFormat::Flac,
        ImpulseFormat::OggVorbis,
        ImpulseFormat::RawF32,
        ImpulseFormat::RawF64,
        ImpulseFormat::Text,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ImpulseFormat::Wave => "WAVE",
            ImpulseFormat::Flac => "FLAC",
            ImpulseFormat::OggVorbis => "Ogg/Vorbis",
            ImpulseFormat::RawF32 => "raw float32",
            ImpulseFormat::RawF64 => "raw float64",
            ImpulseFormat::Text => "text",
        }
    }

    /// Detects the format from the file's magic bytes, falling back to the
    /// extension for headerless formats.
    pub fn detect(data: &[u8], path: Option<&Path>) -> Option<ImpulseFormat> {
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            return Some(ImpulseFormat::Wave);
        }
        if data.len() >= 4 && &data[0..4] == b"fLaC" {
            return Some(ImpulseFormat::Flac);
        }
        if data.len() >= 4 && &data[0..4] == b"OggS" {
            return Some(ImpulseFormat::OggVorbis);
        }
        let extension = path
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        match extension.as_deref() {
            Some("f32") | Some("raw") | Some("pcm") => Some(ImpulseFormat::RawF32),
            Some("f64") => Some(ImpulseFormat::RawF64),
            Some("txt") | Some("csv") | Some("ir") => Some(ImpulseFormat::Text),
            _ => None,
        }
    }
}

/// Loads an impulse response from any supported format.
pub fn load_impulse_response(path: &Path) -> Result<ImpulseResponse, WavError> {
    let mut data = Vec::new();
    File::open(path)?.read_to_end(&mut data)?;
    decode_impulse_response(&data, Some(path))
}

/// Decodes an in-memory impulse response, used by the tests and the loader.
pub fn decode_impulse_response(
    data: &[u8],
    path: Option<&Path>,
) -> Result<ImpulseResponse, WavError> {
    let format = ImpulseFormat::detect(data, path).ok_or_else(|| {
        WavError::Unsupported(format!(
            "cannot tell the impulse response format of '{}'; supported formats are {}",
            path.map(|p| p.display().to_string())
                .unwrap_or_else(|| "<memory>".into()),
            ImpulseFormat::ALL
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    match format {
        ImpulseFormat::Wave => {
            let audio = crate::wav::parse_wave(data)?;
            Ok(ImpulseResponse {
                sample_rate: audio.sample_rate,
                channels: audio.channels,
            })
        }
        ImpulseFormat::Flac => decode_flac(data),
        ImpulseFormat::OggVorbis => decode_vorbis(data),
        ImpulseFormat::RawF32 => Ok(ImpulseResponse {
            sample_rate: DEFAULT_RAW_SAMPLE_RATE,
            channels: vec![decode_raw_f32(data)?],
        }),
        ImpulseFormat::RawF64 => Ok(ImpulseResponse {
            sample_rate: DEFAULT_RAW_SAMPLE_RATE,
            channels: vec![decode_raw_f64(data)?],
        }),
        ImpulseFormat::Text => decode_text(data),
    }
}

fn decode_raw_f32(data: &[u8]) -> Result<Vec<f32>, WavError> {
    if data.len() % 4 != 0 {
        return Err(WavError::Invalid(format!(
            "raw float32 data has {} bytes, which is not a multiple of 4",
            data.len()
        )));
    }
    Ok(data
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn decode_raw_f64(data: &[u8]) -> Result<Vec<f32>, WavError> {
    if data.len() % 8 != 0 {
        return Err(WavError::Invalid(format!(
            "raw float64 data has {} bytes, which is not a multiple of 8",
            data.len()
        )));
    }
    Ok(data
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32)
        .collect())
}

/// Parses a text impulse response: one sample per line or whitespace separated
/// numbers, `#` comments, and an optional `sample_rate <hz>` header.
fn decode_text(data: &[u8]) -> Result<ImpulseResponse, WavError> {
    let text = std::str::from_utf8(data)
        .map_err(|_| WavError::Invalid("text impulse response is not valid UTF-8".into()))?;
    let mut samples = Vec::new();
    let mut sample_rate = DEFAULT_RAW_SAMPLE_RATE;
    let mut declared_rate = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("sample_rate") {
            let rest = rest.trim_start_matches([':', '=', ' ']);
            if declared_rate {
                return Err(WavError::Invalid(
                    "text impulse response declares its sample rate twice".into(),
                ));
            }
            let rate: u32 = rest
                .trim()
                .parse()
                .map_err(|_| WavError::Invalid(format!("bad sample_rate '{rest}'")))?;
            if rate == 0 {
                return Err(WavError::Invalid("sample_rate must not be zero".into()));
            }
            sample_rate = rate;
            declared_rate = true;
            continue;
        }
        for token in line.split([' ', '\t', ',', ';']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let value: f64 = token
                .parse()
                .map_err(|_| WavError::Invalid(format!("'{token}' is not a number")))?;
            if !value.is_finite() {
                return Err(WavError::Invalid(format!(
                    "'{token}' is not a finite sample value"
                )));
            }
            samples.push(value as f32);
        }
    }
    if samples.is_empty() {
        return Err(WavError::Invalid("text impulse response is empty".into()));
    }
    Ok(ImpulseResponse {
        sample_rate,
        channels: vec![samples],
    })
}

fn decode_flac(data: &[u8]) -> Result<ImpulseResponse, WavError> {
    let cursor = std::io::Cursor::new(data);
    let mut reader = claxon::FlacReader::new(BufReader::new(cursor))
        .map_err(|e| WavError::Codec(format!("FLAC: {e}")))?;
    let info = reader.streaminfo();
    let channels = info.channels.max(1) as usize;
    let scale = 1.0f32 / (1i64 << (info.bits_per_sample.saturating_sub(1)).min(30)) as f32;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut index = 0usize;
    let mut samples = reader.samples();
    for sample in samples.by_ref() {
        let value = sample.map_err(|e| WavError::Codec(format!("FLAC: {e}")))?;
        out[index % channels].push(value as f32 * scale);
        index += 1;
    }
    if index == 0 {
        return Err(WavError::Invalid("FLAC impulse response is empty".into()));
    }
    Ok(ImpulseResponse {
        sample_rate: info.sample_rate,
        channels: out,
    })
}

fn decode_vorbis(data: &[u8]) -> Result<ImpulseResponse, WavError> {
    let cursor = std::io::Cursor::new(data);
    let mut reader = lewton::inside_ogg::OggStreamReader::new(BufReader::new(cursor))
        .map_err(|e| WavError::Codec(format!("Ogg/Vorbis: {e}")))?;
    let channels = reader.ident_hdr.audio_channels.max(1) as usize;
    let sample_rate = reader.ident_hdr.audio_sample_rate;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut index = 0usize;
    while let Some(packet) = reader
        .read_dec_packet_itl()
        .map_err(|e| WavError::Codec(format!("Ogg/Vorbis: {e}")))?
    {
        for sample in packet {
            out[index % channels].push(sample as f32 / 32768.0);
            index += 1;
        }
    }
    if index == 0 {
        return Err(WavError::Invalid(
            "Ogg/Vorbis impulse response is empty".into(),
        ));
    }
    Ok(ImpulseResponse {
        sample_rate,
        channels: out,
    })
}

/// Convenience wrapper matching the old `read_wave` call sites.
pub fn load_any(path: &Path) -> Result<ImpulseResponse, WavError> {
    load_impulse_response(path)
}

/// Reads a `.wav` impulse response, kept for callers that require WAVE.
pub fn load_wave(path: &Path) -> Result<ImpulseResponse, WavError> {
    let audio = read_wave(path)?;
    Ok(ImpulseResponse {
        sample_rate: audio.sample_rate,
        channels: audio.channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats_from_magic_bytes() {
        assert_eq!(
            ImpulseFormat::detect(b"RIFF\0\0\0\0WAVEfmt ", None),
            Some(ImpulseFormat::Wave)
        );
        assert_eq!(
            ImpulseFormat::detect(b"fLaC\0\0\0\x22", None),
            Some(ImpulseFormat::Flac)
        );
        assert_eq!(
            ImpulseFormat::detect(b"OggS\0\x02\0\0", None),
            Some(ImpulseFormat::OggVorbis)
        );
        assert_eq!(
            ImpulseFormat::detect(b"\x01\x02\x03\x04", Some(Path::new("ir.f32"))),
            Some(ImpulseFormat::RawF32)
        );
    }

    #[test]
    fn unknown_blobs_are_rejected_with_a_helpful_message() {
        let error = decode_impulse_response(b"not audio at all", None).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("cannot tell the impulse response format"),
            "{text}"
        );
        assert!(text.contains("FLAC"), "{text}");
    }

    #[test]
    fn raw_float32_round_trips() {
        let samples = [0.0f32, 0.5, -0.25, 1.0];
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let ir = decode_impulse_response(&bytes, Some(Path::new("ir.f32"))).unwrap();
        assert_eq!(ir.sample_rate, DEFAULT_RAW_SAMPLE_RATE);
        assert_eq!(ir.channels[0], samples);
    }

    #[test]
    fn raw_float64_round_trips() {
        let samples = [0.0f64, 0.25, -0.5];
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let ir = decode_impulse_response(&bytes, Some(Path::new("ir.f64"))).unwrap();
        assert_eq!(ir.channels[0], vec![0.0, 0.25, -0.5]);
    }

    #[test]
    fn misaligned_raw_data_is_rejected() {
        assert!(decode_impulse_response(b"\0\0\0", Some(Path::new("ir.f32"))).is_err());
    }

    #[test]
    fn text_accepts_headers_comments_and_separators() {
        let text = "# synthetic IR\nsample_rate 96000\n0.0, 1.0, 0.5\n-0.25  0.125\n";
        let ir = decode_impulse_response(text.as_bytes(), Some(Path::new("ir.txt"))).unwrap();
        assert_eq!(ir.sample_rate, 96000);
        assert_eq!(ir.channels[0], vec![0.0, 1.0, 0.5, -0.25, 0.125]);
    }

    #[test]
    fn text_without_header_uses_the_default_rate() {
        let ir = decode_impulse_response(b"1 0 0", Some(Path::new("ir.txt"))).unwrap();
        assert_eq!(ir.sample_rate, DEFAULT_RAW_SAMPLE_RATE);
    }

    #[test]
    fn text_rejects_garbage() {
        let error = decode_impulse_response(b"1.0 banana", Some(Path::new("ir.txt"))).unwrap_err();
        assert!(error.to_string().contains("not a number"), "{error}");
    }

    #[test]
    fn empty_text_is_rejected() {
        assert!(decode_impulse_response(b"# nothing here", Some(Path::new("ir.txt"))).is_err());
    }
}
