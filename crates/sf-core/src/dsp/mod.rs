//! Digital signal processing primitives used by the SonicForge pipeline.

pub mod biquad;
pub mod convolver;
pub mod delay;
pub mod fft;
pub mod filters;
pub mod graphic_eq;
pub mod mixer;
pub mod pipeline;

pub use biquad::{Biquad, BiquadState};
pub use convolver::Convolver;
pub use delay::DelayLine;
pub use fft::{Complex64, Fft, RealFft};
pub use filters::{design, Bandwidth, FilterKind};
pub use graphic_eq::design_graphic_eq;
pub use pipeline::{ChannelMask, Op, Pipeline};
