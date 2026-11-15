//! Configuration language: parsing, expressions, presets and serialization.

pub mod expr;
pub mod filter;
pub mod parser;
pub mod preset;

pub use expr::{expand_inline, Env, EvalError, Value};
pub use filter::{
    parse_copy, parse_delay, parse_filter, parse_graphic_eq, resolve_copy, CopyCommand, CopyTerm,
    DelayCommand, FilterCommand,
};
pub use parser::{Command, ConfigLine, Diagnostic, Document, Severity};
pub use preset::{builtin_presets, Preset, PresetBand};
