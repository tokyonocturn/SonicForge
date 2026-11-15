//! SonicForge audio engine.
//!
//! This crate is deliberately free of Windows-specific code so that the whole
//! signal path can be unit tested on any host. The Windows APO, tray
//! application and GUI all consume this crate.
//!
//! Layers:
//!
//! * [`dsp`] - sample accurate DSP primitives (biquads, FFT convolution, ...).
//! * [`config`] - parser and serializer for the Equalizer APO configuration
//!   language, plus the expression evaluator used by `Eval`/`If`/inline
//!   expressions.
//! * [`engine`] - compiles a configuration document into a [`engine::Pipeline`]
//!   that processes planar audio blocks.
//! * [`wav`] - minimal RIFF/WAVE reader for WAVE impulse responses.
//! * [`ir`] - impulse response loading for WAVE, FLAC, Ogg/Vorbis, raw float
//!   and text files.

pub mod config;
pub mod control;
pub mod device;
pub mod dsp;
pub mod engine;
pub mod ir;
pub mod wav;

pub use engine::{CompileOptions, CompiledChain, Stage};

/// Version string reported by the CLI, GUI and APO.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
