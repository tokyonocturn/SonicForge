//! Compilation of configuration documents into a processing pipeline.

mod compile;

pub use compile::{compile_document, compile_file, CompileOptions, CompiledChain};

/// APO stage a set of commands applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Before the channel mix; runs per application stream.
    PreMix,
    /// After the channel mix; runs once per device stream.
    PostMix,
    /// Capture (input) devices.
    Capture,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::PreMix => "pre-mix",
            Stage::PostMix => "post-mix",
            Stage::Capture => "capture",
        }
    }

    pub fn parse(token: &str) -> Option<Stage> {
        match token.trim().to_lowercase().as_str() {
            "pre-mix" | "premix" => Some(Stage::PreMix),
            "post-mix" | "postmix" => Some(Stage::PostMix),
            "capture" => Some(Stage::Capture),
            _ => None,
        }
    }
}
