//! Editable preset model used by the GUI and the preset library.
//!
//! Presets are a deliberately small subset of the configuration language:
//! a preamp plus a list of parametric bands. Anything more complex is edited as
//! text, which keeps the model honest instead of pretending to round-trip
//! features it cannot represent.

use super::filter::parse_filter;
use super::parser::{Command, Document};
use crate::dsp::filters::FilterKind;

/// One parametric band.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetBand {
    pub enabled: bool,
    pub kind: FilterKind,
    /// Centre or corner frequency in Hz.
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl PresetBand {
    pub fn peak(freq: f64, gain_db: f64, q: f64) -> Self {
        Self { enabled: true, kind: FilterKind::Peak, freq, gain_db, q }
    }

    pub fn low_shelf(freq: f64, gain_db: f64, q: f64) -> Self {
        Self { enabled: true, kind: FilterKind::LowShelf12, freq, gain_db, q }
    }

    pub fn high_shelf(freq: f64, gain_db: f64, q: f64) -> Self {
        Self { enabled: true, kind: FilterKind::HighShelf12, freq, gain_db, q }
    }

    /// Renders the band as a `Filter:` line.
    pub fn to_line(&self, index: usize) -> String {
        let state = if self.enabled { "ON " } else { "OFF" };
        let type_name = match self.kind {
            FilterKind::Peak => "PK ".to_string(),
            FilterKind::LowShelf12 => "LS 12dB ".to_string(),
            FilterKind::HighShelf12 => "HS 12dB ".to_string(),
            FilterKind::LowShelf6 => "LS 6dB ".to_string(),
            FilterKind::HighShelf6 => "HS 6dB ".to_string(),
            FilterKind::LowShelf => "LSC ".to_string(),
            FilterKind::HighShelf => "HSC ".to_string(),
            FilterKind::Notch => "NO ".to_string(),
            other => format!("{} ", other.name()),
        };
        let mut line = format!("Filter {:>2}: {state} {type_name}Fc {:>8.1} Hz", index, self.freq);
        if self.kind.uses_gain() || self.gain_db != 0.0 {
            line.push_str(&format!(" Gain {:>6.1} dB", self.gain_db));
        }
        if self.kind.uses_q() {
            line.push_str(&format!(" Q {:>6.3}", self.q));
        }
        line
    }
}

/// A named collection of bands and a preamp.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub preamp_db: f64,
    pub bands: Vec<PresetBand>,
}

impl Default for Preset {
    fn default() -> Self {
        Self { name: "Untitled".into(), description: String::new(), preamp_db: 0.0, bands: Vec::new() }
    }
}

impl Preset {
    pub fn new(name: impl Into<String>, preamp_db: f64, bands: Vec<PresetBand>) -> Self {
        Self { name: name.into(), description: String::new(), preamp_db, bands }
    }

    /// Renders the preset as a complete configuration file.
    pub fn to_config_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# SonicForge preset: {}\n", self.name));
        if !self.description.is_empty() {
            for line in self.description.lines() {
                out.push_str(&format!("# {line}\n"));
            }
        }
        out.push_str(&format!("Preamp: {:.1} dB\n", self.preamp_db));
        for (i, band) in self.bands.iter().enumerate() {
            if !band.enabled {
                continue;
            }
            out.push_str(&band.to_line(i + 1));
            out.push('\n');
        }
        out
    }

    /// Best-effort extraction of a preset from a parsed document. Only
    /// `Preamp:` and `Filter:`/`GraphicEQ:` lines are considered; anything else
    /// (channel selection, convolution, ...) is ignored.
    pub fn from_document(doc: &Document) -> Self {
        let mut preset = Preset::default();
        for line in &doc.lines {
            match line.command {
                Command::Preamp => {
                    if let Some(v) = line.params.split_whitespace().next().and_then(super::filter::parse_f64) {
                        preset.preamp_db += v;
                    }
                }
                Command::Filter => {
                    if let Ok(cmd) = parse_filter(&line.params) {
                        if cmd.order.is_some() {
                            continue;
                        }
                        preset.bands.push(PresetBand {
                            enabled: cmd.enabled,
                            kind: cmd.kind,
                            freq: cmd.freq,
                            gain_db: cmd.gain_db,
                            q: match cmd.bandwidth {
                                crate::dsp::filters::Bandwidth::Q(q) => q,
                                other => other.to_q(cmd.freq.max(1.0), 48000.0),
                            },
                        });
                    }
                }
                Command::GraphicEq => {
                    if let Ok(points) = super::filter::parse_graphic_eq(&line.params) {
                        for (i, (f, g)) in points.iter().enumerate() {
                            preset.bands.push(PresetBand {
                                enabled: true,
                                kind: FilterKind::Peak,
                                freq: *f,
                                gain_db: *g,
                                q: if i == 0 { 1.4 } else { 2.0 },
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        preset
    }

    /// Total positive boost of all enabled bands, used to suggest a preamp.
    pub fn peak_boost_db(&self) -> f64 {
        self.bands
            .iter()
            .filter(|b| b.enabled && b.gain_db > 0.0)
            .map(|b| b.gain_db)
            .fold(0.0f64, f64::max)
    }

    /// A preamp that keeps the loudest band from clipping.
    pub fn suggested_preamp(&self) -> f64 {
        -self.peak_boost_db()
    }
}

/// Presets shipped with the application. All of them are plain parametric
/// configurations, no hidden magic.
pub fn builtin_presets() -> Vec<Preset> {
    let mut flat = Preset::new("Flat", 0.0, Vec::new());
    flat.description = "No processing. Useful as a neutral starting point.".into();

    let mut bass = Preset::new(
        "Bass Boost",
        -5.0,
        vec![
            PresetBand::low_shelf(105.0, 6.0, 0.707),
            PresetBand::peak(60.0, 2.0, 1.0),
        ],
    );
    bass.description = "Adds weight below 150 Hz without mudding the midrange.".into();

    let mut treble = Preset::new(
        "Treble Lift",
        -3.5,
        vec![
            PresetBand::high_shelf(6500.0, 4.0, 0.707),
            PresetBand::peak(12000.0, 2.0, 0.9),
        ],
    );
    treble.description = "Opens up cymbals and air band.".into();

    let mut vocal = Preset::new(
        "Vocal Clarity",
        -3.0,
        vec![
            PresetBand::peak(260.0, -2.5, 1.1),
            PresetBand::peak(2600.0, 3.0, 1.4),
            PresetBand::peak(4800.0, 1.5, 2.0),
        ],
    );
    vocal.description = "Cuts boxiness and lifts presence for speech and podcasts.".into();

    let mut loudness = Preset::new(
        "Loudness",
        -6.0,
        vec![
            PresetBand::low_shelf(90.0, 6.5, 0.707),
            PresetBand::peak(1800.0, -1.5, 1.2),
            PresetBand::high_shelf(9000.0, 4.5, 0.707),
        ],
    );
    loudness.description = "Smiley curve for quiet listening levels.".into();

    let mut harman = Preset::new(
        "Harman Target (Headphones)",
        -4.0,
        vec![
            PresetBand::low_shelf(105.0, 4.0, 0.707),
            PresetBand::peak(2800.0, -2.0, 1.2),
            PresetBand::peak(4200.0, 1.5, 2.0),
            PresetBand::high_shelf(8000.0, 2.0, 0.707),
        ],
    );
    harman.description = "Approximation of the Harman in-room target for headphones.".into();

    vec![flat, bass, treble, vocal, loudness, harman]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_renders_parsable_configuration() {
        let preset = builtin_presets().into_iter().find(|p| p.name == "Bass Boost").unwrap();
        let text = preset.to_config_text();
        let doc = Document::parse(&text);
        assert_eq!(doc.lines.len(), 1 + preset.bands.len());
        let restored = Preset::from_document(&doc);
        assert!((restored.preamp_db - preset.preamp_db).abs() < 1e-9);
        assert_eq!(restored.bands.len(), preset.bands.len());
        assert!((restored.bands[0].freq - preset.bands[0].freq).abs() < 1e-9);
    }

    #[test]
    fn parsed_preset_round_trips_through_text() {
        let mut preset = Preset::default();
        preset.preamp_db = -3.5;
        preset.bands.push(PresetBand::peak(1000.0, 4.5, 1.41));
        let text = preset.to_config_text();
        let parsed = Preset::from_document(&Document::parse(&text));
        assert_eq!(parsed.bands.len(), 1);
        assert!((parsed.bands[0].gain_db - 4.5).abs() < 0.1);
        assert!((parsed.bands[0].q - 1.41).abs() < 0.001);
    }

    #[test]
    fn suggested_preamp_offsets_the_largest_boost() {
        let preset = Preset::new(
            "t",
            0.0,
            vec![PresetBand::peak(100.0, 6.0, 1.0), PresetBand::peak(1000.0, 2.0, 1.0)],
        );
        assert_eq!(preset.suggested_preamp(), -6.0);
    }

    #[test]
    fn graphic_eq_documents_become_preset_bands() {
        let doc = Document::parse("GraphicEQ: 100 3; 1000 -2; 10000 1\n");
        let preset = Preset::from_document(&doc);
        assert_eq!(preset.bands.len(), 3);
        assert_eq!(preset.bands[1].gain_db, -2.0);
    }

    #[test]
    fn all_builtin_presets_are_valid_configs() {
        for preset in builtin_presets() {
            let doc = Document::parse(&preset.to_config_text());
            assert!(doc.issues.is_empty(), "{}: {:?}", preset.name, doc.issues);
            assert_eq!(doc.lines.len(), 1 + preset.bands.len(), "{}", preset.name);
        }
    }
}
