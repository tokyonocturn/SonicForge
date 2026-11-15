//! `LoudnessCorrection:` behaviour through the real compiler and pipeline.
//!
//! The unit tests in `dsp::loudness` cover the gain model; these tests prove that
//! a configuration line turns into an audible low/high shelf pair, that the
//! gains scale with the listening volume, and that the reference level really is
//! neutral.

use sf_core::config::Document;
use sf_core::dsp::loudness::{correction, LoudnessSettings, HIGH_SHELF_HZ, LOW_SHELF_HZ};
use sf_core::engine::{compile_document, CompileOptions, CompiledChain};

fn chain_for(line: &str, volume_db: f64) -> CompiledChain {
    compile_document(
        &Document::parse(&format!("{line}\n")),
        &CompileOptions {
            volume_db,
            ..Default::default()
        },
    )
}

/// Response in dB at `freq`.
fn at(chain: &mut CompiledChain, freq: f64) -> f64 {
    chain.pipeline.frequency_response(&[freq])[0]
}

#[test]
fn the_documented_line_compiles_without_diagnostics() {
    let chain = chain_for(
        "LoudnessCorrection: State 1 ReferenceLevel -20 ReferenceOffset 0 Attenuation 1.0",
        -35.0,
    );
    assert!(!chain.has_errors(), "{:?}", chain.diagnostics);
    assert!(
        chain.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        chain.diagnostics
    );
}

#[test]
fn the_reference_volume_is_neutral() {
    let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", -20.0);
    for freq in [20.0, 75.0, 1000.0, 10000.0, 16000.0] {
        let gain = at(&mut chain, freq);
        assert!(gain.abs() < 1e-6, "{freq} Hz gained {gain} dB");
    }
}

#[test]
fn louder_than_the_reference_is_also_neutral() {
    let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", -6.0);
    assert!(at(&mut chain, 30.0).abs() < 1e-6);
}

#[test]
fn low_shelf_gain_matches_the_model_at_each_volume() {
    // Bass boost is half of the distance below the reference.
    for (volume, expected_low) in [(-25.0, 2.5), (-30.0, 5.0), (-40.0, 10.0), (-60.0, 20.0)] {
        let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", volume);
        let model = correction(
            &LoudnessSettings {
                state: true,
                reference_level: -20.0,
                reference_offset: 0.0,
                attenuation: 1.0,
            },
            volume,
        );
        assert!((model.low_shelf_db - expected_low).abs() < 1e-9);
        // 20 Hz is far below the 75 Hz corner, so the shelf is fully developed
        // there; 1 kHz is untouched, and the preamp applies to both.
        let difference = at(&mut chain, 20.0) - at(&mut chain, 1000.0);
        assert!(
            (difference - expected_low).abs() < 0.6,
            "volume {volume}: low shelf measured {difference} dB, expected {expected_low}"
        );
    }
}

#[test]
fn high_shelf_gain_matches_the_model_at_each_volume() {
    for (volume, expected_high) in [(-30.0, 2.5), (-40.0, 5.0), (-60.0, 10.0)] {
        let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", volume);
        let difference = at(&mut chain, 18000.0) - at(&mut chain, 1000.0);
        assert!(
            (difference - expected_high).abs() < 0.6,
            "volume {volume}: high shelf measured {difference} dB, expected {expected_high}"
        );
    }
}

#[test]
fn headroom_cut_keeps_the_overall_level_down() {
    let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", -40.0);
    // preamp = -(low + high) / 2 = -(10 + 5) / 2 = -7.5 dB at 1 kHz.
    let mid = at(&mut chain, 1000.0);
    assert!((mid + 7.5).abs() < 0.2, "mid-band gain was {mid} dB");
}

#[test]
fn attenuation_scales_the_whole_correction() {
    let mut full = chain_for(
        "LoudnessCorrection: State 1 ReferenceLevel -20 Attenuation 1.0",
        -40.0,
    );
    let mut half = chain_for(
        "LoudnessCorrection: State 1 ReferenceLevel -20 Attenuation 0.5",
        -40.0,
    );
    let full_low = at(&mut full, 20.0) - at(&mut full, 1000.0);
    let half_low = at(&mut half, 20.0) - at(&mut half, 1000.0);
    assert!(
        (half_low - full_low / 2.0).abs() < 0.4,
        "{half_low} vs {full_low}"
    );
}

#[test]
fn state_zero_switches_the_filter_off() {
    let mut chain = chain_for("LoudnessCorrection: State 0 ReferenceLevel -20", -50.0);
    assert!(chain.diagnostics.is_empty(), "{:?}", chain.diagnostics);
    assert!(at(&mut chain, 20.0).abs() < 1e-6);
}

#[test]
fn reference_offset_moves_the_neutral_point() {
    let mut chain = chain_for(
        "LoudnessCorrection: State 1 ReferenceLevel -20 ReferenceOffset -10",
        -10.0,
    );
    assert!(at(&mut chain, 20.0).abs() < 1e-6);
}

#[test]
fn malformed_lines_are_errors_not_silence() {
    for line in [
        "LoudnessCorrection: ReferenceLevel -20",
        "LoudnessCorrection: State 1 ReferenceLevl -20",
        "LoudnessCorrection: State 2 ReferenceLevel -20",
        "LoudnessCorrection: State 1 ReferenceLevel -20 Attenuation 5",
        "LoudnessCorrection:",
    ] {
        let chain = chain_for(line, -30.0);
        assert!(chain.has_errors(), "'{line}' was accepted");
        let message = chain.diagnostics[0].to_string();
        assert!(
            message.contains("LoudnessCorrection"),
            "'{line}' produced '{message}'"
        );
    }
}

#[test]
fn the_shelves_sit_at_the_documented_frequencies() {
    let mut chain = chain_for("LoudnessCorrection: State 1 ReferenceLevel -20", -40.0);
    // At the corner frequencies the shelves are half of their final value.
    let low_corner = at(&mut chain, LOW_SHELF_HZ) - at(&mut chain, 1000.0);
    let high_corner = at(&mut chain, HIGH_SHELF_HZ) - at(&mut chain, 1000.0);
    assert!((low_corner - 5.0).abs() < 1.0, "low corner {low_corner}");
    assert!((high_corner - 2.5).abs() < 1.0, "high corner {high_corner}");
}
