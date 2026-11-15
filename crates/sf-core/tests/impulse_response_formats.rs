//! Cross-format proof for `Convolution:`.
//!
//! Every fixture in `tests/fixtures/ir` is the same synthetic impulse response
//! (damped 200/800/2000 Hz modes) stored in a different container. The tests
//! load each one through [`sf_core::ir`] and compare it with the WAVE original:
//! lossless formats must match sample for sample, and even the lossy Vorbis file
//! must produce the same measured magnitude response.

use sf_core::ir::{load_impulse_response, ImpulseFormat, ImpulseResponse};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ir")
        .join(name)
}

fn load(name: &str) -> ImpulseResponse {
    load_impulse_response(&fixture(name)).unwrap_or_else(|e| panic!("cannot load {name}: {e}"))
}

/// Magnitude response in dB at `freq` for a mono impulse response.
fn magnitude_db(ir: &ImpulseResponse, freq: f64) -> f64 {
    let samples = &ir.channels[0];
    let rate = ir.sample_rate as f64;
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    for (n, &sample) in samples.iter().enumerate() {
        let phase = -2.0 * std::f64::consts::PI * freq * n as f64 / rate;
        re += sample as f64 * phase.cos();
        im += sample as f64 * phase.sin();
    }
    20.0 * (re.hypot(im)).max(1e-12).log10()
}

fn response(ir: &ImpulseResponse) -> Vec<f64> {
    let freqs: Vec<f64> = (0..48)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / 47.0))
        .collect();
    freqs.iter().map(|f| magnitude_db(ir, *f)).collect()
}

fn max_deviation(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f64, f64::max)
}

fn rms_deviation(a: &[f64], b: &[f64]) -> f64 {
    let sum: f64 = a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum();
    (sum / a.len() as f64).sqrt()
}

#[test]
fn every_supported_format_is_detected() {
    let cases = [
        ("ir.wav", ImpulseFormat::Wave),
        ("ir.flac", ImpulseFormat::Flac),
        ("ir.ogg", ImpulseFormat::OggVorbis),
        ("ir.f32", ImpulseFormat::RawF32),
        ("ir.txt", ImpulseFormat::Text),
    ];
    for (name, expected) in cases {
        let data = std::fs::read(fixture(name)).unwrap();
        assert_eq!(
            ImpulseFormat::detect(&data, Some(&fixture(name))),
            Some(expected),
            "{name}"
        );
    }
}

#[test]
fn all_formats_agree_on_sample_rate_and_length() {
    let reference = load("ir.wav");
    assert_eq!(reference.sample_rate, 48000);
    assert_eq!(reference.channel_count(), 1);
    assert!(reference.frames() > 1000);

    for name in ["ir.flac", "ir.f32", "ir.txt"] {
        let ir = load(name);
        assert_eq!(ir.sample_rate, reference.sample_rate, "{name}");
        assert_eq!(ir.frames(), reference.frames(), "{name}");
    }

    // Vorbis is lossy and adds encoder priming, so only the rate is guaranteed.
    let ogg = load("ir.ogg");
    assert_eq!(ogg.sample_rate, reference.sample_rate);
    assert!(ogg.frames() >= reference.frames(), "ogg lost samples");
}

#[test]
fn lossless_formats_are_sample_exact() {
    let reference = load("ir.wav");
    for name in ["ir.flac", "ir.f32", "ir.txt"] {
        let ir = load(name);
        let worst = reference.channels[0]
            .iter()
            .zip(ir.channels[0].iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{name} differs from the WAVE IR by {worst}");
    }
}

#[test]
fn flac_matches_the_wave_frequency_response() {
    let wave = response(&load("ir.wav"));
    let flac = response(&load("ir.flac"));
    let deviation = max_deviation(&wave, &flac);
    assert!(deviation < 1e-5, "FLAC response deviates by {deviation} dB");
}

#[test]
fn raw_and_text_match_the_wave_frequency_response() {
    let wave = response(&load("ir.wav"));
    for name in ["ir.f32", "ir.txt"] {
        let other = response(&load(name));
        let deviation = max_deviation(&wave, &other);
        assert!(deviation < 1e-4, "{name} deviates by {deviation} dB");
    }
}

#[test]
fn vorbis_matches_the_wave_frequency_response_within_codec_tolerance() {
    let wave_ir = load("ir.wav");
    let ogg_ir = load("ir.ogg");
    let freqs: Vec<f64> = (0..48)
        .map(|i| 20.0 * 1000f64.powf(i as f64 / 47.0))
        .collect();
    let wave: Vec<f64> = freqs.iter().map(|f| magnitude_db(&wave_ir, *f)).collect();
    let ogg: Vec<f64> = freqs.iter().map(|f| magnitude_db(&ogg_ir, *f)).collect();

    let rms = rms_deviation(&wave, &ogg);
    assert!(rms < 0.3, "Ogg/Vorbis response deviates by {rms:.3} dB RMS");

    // Above the pass band the impulse response has no energy left and the
    // codec's noise floor dominates the measurement, so the tight bound is
    // applied where the signal actually is: within 40 dB of the peak.
    let peak = wave.iter().copied().fold(f64::MIN, f64::max);
    let mut worst = 0.0f64;
    for ((freq, wave_db), ogg_db) in freqs.iter().zip(wave.iter()).zip(ogg.iter()) {
        if *wave_db < peak - 40.0 {
            continue;
        }
        let difference = (wave_db - ogg_db).abs();
        worst = worst.max(difference);
        assert!(
            difference < 1.0,
            "{freq:.0} Hz: WAVE {wave_db:.3} dB vs Ogg {ogg_db:.3} dB"
        );
    }
    assert!(worst > 0.0, "nothing was compared");
}

#[test]
fn the_measured_response_has_the_expected_shape() {
    // The fixture is a sum of damped modes at 200/800/2000 Hz, so those three
    // frequencies must be prominent compared with the gaps between them.
    let ir = load("ir.wav");
    let mode_200 = magnitude_db(&ir, 200.0);
    let mode_800 = magnitude_db(&ir, 800.0);
    let mode_2000 = magnitude_db(&ir, 2000.0);
    let gap_400 = magnitude_db(&ir, 400.0);
    let gap_1400 = magnitude_db(&ir, 1400.0);
    assert!(mode_200 > gap_400 + 3.0, "{mode_200} vs {gap_400}");
    assert!(mode_800 > gap_400 + 3.0, "{mode_800} vs {gap_400}");
    assert!(mode_2000 > gap_1400 + 3.0, "{mode_2000} vs {gap_1400}");
}

#[test]
fn convolution_uses_the_loader_for_every_format() {
    // End to end through the compiler: the same config with a WAVE and a FLAC
    // impulse response must produce the same output for the same input.
    use sf_core::config::Document;
    use sf_core::engine::{compile_document, CompileOptions};

    let mut outputs = Vec::new();
    for name in ["ir.wav", "ir.flac"] {
        let config = format!("Convolution: {}\n", fixture(name).display());
        let mut chain = compile_document(
            &Document::parse(&config),
            &CompileOptions {
                channels: 1,
                ..Default::default()
            },
        );
        assert!(!chain.has_errors(), "{name}: {:?}", chain.diagnostics);
        let mut buffer = vec![vec![0.0f32; 4096]];
        buffer[0][0] = 1.0;
        chain.pipeline.process(&mut buffer);
        outputs.push(buffer[0].clone());
    }
    let worst = outputs[0]
        .iter()
        .zip(outputs[1].iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "WAVE and FLAC convolution differ by {worst}");
}

#[test]
fn a_bad_impulse_response_reports_the_file_and_reason() {
    use sf_core::config::Document;
    use sf_core::engine::{compile_document, CompileOptions};

    let missing = fixture("does-not-exist.wav");
    let config = format!("Convolution: {}\n", missing.display());
    let chain = compile_document(&Document::parse(&config), &CompileOptions::default());
    assert!(chain.has_errors());
    let message = chain.diagnostics[0].to_string();
    assert!(message.contains("cannot load"), "{message}");
}
