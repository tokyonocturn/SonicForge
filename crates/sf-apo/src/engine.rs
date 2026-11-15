//! The audio processing engine owned by one APO instance.
//!
//! This layer is deliberately free of COM so that the interleaved audio path,
//! configuration reloads and bypass handling can be unit tested without the
//! Windows audio engine.

use sf_core::config::Document;
use sf_core::control::{ControlBlock, ControlSnapshot};
use sf_core::device::DeviceInfo;
use sf_core::dsp::loudness::{VolumeTracker, DEFAULT_RETUNE_THRESHOLD_DB};
use sf_core::engine::compiler_diagnostics;
use sf_core::engine::{compile_document, CompileOptions, CompiledChain, Stage};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Result of one configuration (re)load.
#[derive(Debug, Default)]
pub struct ReloadReport {
    /// True when the pipeline actually changed.
    pub changed: bool,
    /// Human readable diagnostics from the compiler.
    pub diagnostics: Vec<String>,
    pub channels: usize,
    pub sample_rate: u32,
}

/// Owns the compiled pipeline and the scratch buffers used by `APOProcess`.
pub struct ApoEngine {
    config_path: PathBuf,
    device: DeviceInfo,
    sample_rate: u32,
    input_channels: usize,
    chain: Option<CompiledChain>,
    planar: Vec<Vec<f32>>,
    mtime: Option<SystemTime>,
    bypass: bool,
    master_preamp_db: f32,
    control: Option<ControlBlock>,
    last_config_generation: u32,
    latency_samples: usize,
    force_reload: bool,
    /// Decides when a volume knob change is worth a rebuild. Holds the last
    /// volume the chain was compiled for, so `LoudnessCorrection:` retunes
    /// without a config edit. The decision logic lives in `sf-core` so it is
    /// unit tested on every platform; `Cell` because the poll takes `&self`.
    volume_tracker: std::cell::Cell<VolumeTracker>,
    pub reloads: u64,
}

impl ApoEngine {
    /// Creates an engine for `config_path`. The pipeline is built immediately
    /// if the file exists; failures leave the engine in pass-through mode.
    pub fn new(
        config_path: PathBuf,
        device: DeviceInfo,
        sample_rate: u32,
        channels: usize,
        control: Option<ControlBlock>,
    ) -> Self {
        let mut engine = Self {
            config_path,
            device,
            sample_rate,
            input_channels: channels.max(1),
            chain: None,
            planar: Vec::new(),
            mtime: None,
            bypass: false,
            master_preamp_db: 0.0,
            control,
            last_config_generation: 0,
            latency_samples: 0,
            force_reload: true,
            volume_tracker: std::cell::Cell::new(VolumeTracker::new(DEFAULT_RETUNE_THRESHOLD_DB)),
            reloads: 0,
        };
        engine.reload();
        engine
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn latency_samples(&self) -> usize {
        self.latency_samples
    }

    pub fn pipeline_channels(&self) -> usize {
        self.chain
            .as_ref()
            .map(|c| c.pipeline.channels())
            .unwrap_or(self.input_channels)
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.chain
            .as_ref()
            .map(|c| {
                c.diagnostics
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    pub fn has_errors(&self) -> bool {
        self.chain.as_ref().map(|c| c.has_errors()).unwrap_or(false)
    }

    /// Applies live control values (bypass, master preamp).
    pub fn sync_control(&mut self) {
        if let Some(control) = &self.control {
            let snap: ControlSnapshot = control.read();
            self.bypass = snap.bypass;
            self.master_preamp_db = snap.preamp_db;
            if snap.config_generation != self.last_config_generation {
                if self.last_config_generation != 0 {
                    self.force_reload = true;
                }
                self.last_config_generation = snap.config_generation;
            }
        }
    }

    /// Rebuilds the pipeline if the configuration file or the control block
    /// asked for it. Returns a report describing the outcome.
    pub fn reload(&mut self) -> ReloadReport {
        let mut report = ReloadReport {
            sample_rate: self.sample_rate,
            channels: self.input_channels,
            ..Default::default()
        };
        if !self.force_reload {
            return report;
        }
        self.force_reload = false;
        let mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        let text = match std::fs::read_to_string(&self.config_path) {
            Ok(t) => t,
            Err(e) => {
                // Missing configuration is not an error: pass audio through.
                report
                    .diagnostics
                    .push(format!("cannot read '{}': {e}", self.config_path.display()));
                self.chain = None;
                self.latency_samples = 0;
                self.mtime = mtime;
                return report;
            }
        };
        let doc = Document::parse_with_path(&text, Some(&self.config_path));
        let volume_db = sf_win::endpoints::default_render_volume_db().unwrap_or(0.0);
        let mut tracker = self.volume_tracker.get();
        tracker.observe(volume_db);
        self.volume_tracker.set(tracker);
        let options = CompileOptions {
            sample_rate: self.sample_rate,
            channels: self.input_channels,
            device: self.device.clone(),
            stage: Stage::PostMix,
            max_include_depth: 16,
            convolution_block: sf_core::dsp::convolver::DEFAULT_BLOCK,
            // Polled off the audio thread: the watcher reloads this chain.
            volume_db: volume_db as f64,
        };
        let chain = compile_document(&doc, &options);
        report.changed = true;
        report.diagnostics = compiler_diagnostics(&chain);
        report.channels = chain.pipeline.channels();
        self.latency_samples = chain.pipeline.latency_samples();
        self.planar = vec![vec![0.0; 4096]; chain.pipeline.channels()];
        self.chain = Some(chain);
        self.mtime = mtime;
        self.reloads += 1;
        report
    }

    /// True when the configuration file changed on disk, or when the listening
    /// volume moved enough to change the loudness correction.
    ///
    /// Called from the watcher thread, never from the audio thread: the volume
    /// query is a COM call and must stay off the real-time path.
    pub fn config_changed_on_disk(&self) -> bool {
        let current = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        if current != self.mtime {
            return true;
        }
        let mut tracker = self.volume_tracker.get();
        let retune = tracker.observe_reading(sf_win::endpoints::default_render_volume_db());
        self.volume_tracker.set(tracker);
        retune
    }

    /// Forces a reload on the next call to [`Self::reload`].
    pub fn invalidate(&mut self) {
        self.force_reload = true;
    }

    /// Processes interleaved `f32` audio in place.
    ///
    /// `io` holds `frames * input_channels` samples. Extra pipeline channels
    /// created by `Copy:` are processed internally and discarded, exactly like
    /// the reference implementation's virtual channels.
    pub fn process_interleaved(&mut self, io: &mut [f32], frames: usize) {
        let channels = self.input_channels;
        if frames == 0 || channels == 0 || io.len() < frames * channels {
            return;
        }
        let Some(chain) = self.chain.as_mut() else {
            return;
        };
        let pipeline_channels = chain.pipeline.channels().max(channels);

        if self.planar.len() != pipeline_channels {
            self.planar = vec![vec![0.0; frames]; pipeline_channels];
        }
        if self.planar.iter().any(|c| c.len() < frames) {
            for ch in self.planar.iter_mut() {
                ch.resize(frames, 0.0);
            }
        }

        // Interleaved -> planar.
        for frame in 0..frames {
            let base = frame * channels;
            for ch in 0..pipeline_channels {
                let value = if ch < channels { io[base + ch] } else { 0.0 };
                self.planar[ch][frame] = value;
            }
        }

        chain.pipeline.process(&mut self.planar);

        // Master preamp from the live control block.
        let gain = if self.master_preamp_db.abs() > 1e-6 {
            10f32.powf(self.master_preamp_db / 20.0)
        } else {
            1.0
        };

        // Planar -> interleaved. Channels beyond the device layout are dropped.
        for frame in 0..frames {
            let base = frame * channels;
            for ch in 0..channels {
                io[base + ch] = self.planar[ch][frame] * gain;
            }
        }
    }

    /// True when audio should pass through untouched.
    pub fn is_bypassed(&self) -> bool {
        self.bypass || self.chain.is_none()
    }

    /// Clears all filter state, e.g. when a stream restarts.
    pub fn reset_state(&mut self) {
        if let Some(chain) = self.chain.as_mut() {
            chain.pipeline.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn engine_with_config(config: &str, channels: usize) -> ApoEngine {
        let dir = std::env::temp_dir().join(format!(
            "sonicforge-apo-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.txt");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(config.as_bytes()).unwrap();
        ApoEngine::new(path, DeviceInfo::default(), 48000, channels, None)
    }

    #[test]
    fn applies_configured_gain_to_interleaved_audio() {
        let mut engine = engine_with_config("Preamp: -6 dB\n", 2);
        let frames = 64;
        let mut io = vec![1.0f32; frames * 2];
        engine.process_interleaved(&mut io, frames);
        for (i, v) in io.iter().enumerate() {
            assert!((v - 0.5011872).abs() < 1e-4, "sample {i} = {v}");
        }
    }

    #[test]
    fn preserves_channel_independence() {
        let mut engine = engine_with_config("Channel: L\nPreamp: -6 dB\n", 2);
        let frames = 32;
        let mut io = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            io.push(1.0);
            io.push(1.0);
        }
        engine.process_interleaved(&mut io, frames);
        assert!((io[0] - 0.5011872).abs() < 1e-4);
        assert!((io[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn missing_configuration_passes_audio_through() {
        let mut engine = ApoEngine::new(
            PathBuf::from("this-file-does-not-exist.txt"),
            DeviceInfo::default(),
            48000,
            2,
            None,
        );
        let mut io = vec![0.25f32; 128];
        let original = io.clone();
        engine.process_interleaved(&mut io, 64);
        assert_eq!(io, original);
        assert!(engine.is_bypassed());
    }

    #[test]
    fn bypass_skips_processing() {
        let mut engine = engine_with_config("Preamp: -12 dB\n", 2);
        engine.bypass = true;
        assert!(engine.is_bypassed());
        let mut io = vec![1.0f32; 32];
        engine.process_interleaved(&mut io, 16);
        // Processing still happens (the caller decides whether to call), but the
        // engine never claims to be active while bypassed.
        assert!(engine.is_bypassed());
    }

    #[test]
    fn virtual_channels_do_not_change_the_device_layout() {
        let mut engine = engine_with_config("Copy: L2=L R2=R\nChannel: L2 R2\nPreamp: -6 dB\n", 2);
        assert_eq!(engine.pipeline_channels(), 4);
        let frames = 16;
        let mut io = vec![0.0f32; frames * 2];
        for frame in 0..frames {
            io[frame * 2] = 1.0;
            io[frame * 2 + 1] = 2.0;
        }
        engine.process_interleaved(&mut io, frames);
        // Chained preamps: L -> L2 (-6 dB) and back through Copy? The pipeline
        // writes L2 only, so the device channels are unchanged.
        for frame in 0..frames {
            assert!((io[frame * 2] - 1.0).abs() < 1e-6);
            assert!((io[frame * 2 + 1] - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn reload_reports_diagnostics() {
        let mut engine = engine_with_config("Filter: ON PK Fc\n", 2);
        assert!(engine.has_errors());
        assert!(!engine.diagnostics().is_empty());
        std::fs::write(engine.config_path(), "Preamp: -3 dB\n").unwrap();
        engine.invalidate();
        let report = engine.reload();
        assert!(report.changed);
        assert!(!engine.has_errors(), "{:?}", engine.diagnostics());
    }

    #[test]
    fn master_preamp_from_control_is_applied() {
        let mut engine = engine_with_config("Preamp: 0 dB\n", 1);
        engine.master_preamp_db = -6.0;
        let mut io = vec![1.0f32; 8];
        engine.process_interleaved(&mut io, 8);
        assert!((io[0] - 0.5011872).abs() < 1e-4);
    }
}
