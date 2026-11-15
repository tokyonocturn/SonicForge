//! Line oriented parser for configuration documents.
//!
//! The document keeps every line verbatim. Parameter interpretation happens in
//! the compiler so that `Eval:`/inline expressions can influence later lines in
//! file order, exactly like the reference implementation.

use std::path::{Path, PathBuf};

/// Severity of a parsing or compilation message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// A diagnostic tied to a line of a configuration file.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub line: usize,
    pub file: Option<PathBuf>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: Severity, line: usize, message: impl Into<String>) -> Self {
        Self { severity, line, file: None, message: message.into() }
    }

    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}: {}", if let Some(p) = &self.file { p.display().to_string() } else { "<config>".into() }, self.line, self.severity.as_str(), self.message)
    }
}

/// Commands understood by the engine. Anything else is inert text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Preamp,
    Filter,
    GraphicEq,
    Convolution,
    Copy,
    Delay,
    Channel,
    Device,
    Stage,
    Include,
    If,
    ElseIf,
    Else,
    EndIf,
    Eval,
    VstPlugin,
    LoudnessCorrection,
    Comment,
    /// Syntactically valid command with an unrecognised name.
    Unknown,
}

impl Command {
    /// Classifies a command name. Command names are case sensitive in the
    /// reference format, so `preamp:` is inert text there; SonicForge accepts
    /// it as a warning instead of silently ignoring a likely typo.
    pub fn parse(name: &str) -> Option<Command> {
        // `Filter 1` and `Filter 12` carry an index before the colon.
        let base = name.split_whitespace().next().unwrap_or("");
        let cmd = match base {
            "Preamp" => Command::Preamp,
            "Filter" => Command::Filter,
            "GraphicEQ" => Command::GraphicEq,
            "Convolution" => Command::Convolution,
            "Copy" => Command::Copy,
            "Delay" => Command::Delay,
            "Channel" => Command::Channel,
            "Device" => Command::Device,
            "Stage" => Command::Stage,
            "Include" => Command::Include,
            "If" => Command::If,
            "ElseIf" => Command::ElseIf,
            "Else" => Command::Else,
            "EndIf" => Command::EndIf,
            "Eval" => Command::Eval,
            "VSTPlugin" | "Plugin" => Command::VstPlugin,
            "LoudnessCorrection" => Command::LoudnessCorrection,
            "Comment" => Command::Comment,
            _ => {
                // Case-insensitive fallback: report as unknown so the compiler
                // can warn about it.
                let lower = base.to_lowercase();
                match lower.as_str() {
                    "preamp" | "filter" | "graphiceq" | "convolution" | "copy" | "delay" | "channel"
                    | "device" | "stage" | "include" | "if" | "elseif" | "else" | "endif" | "eval"
                    | "vstplugin" | "loudnesscorrection" | "comment" => Command::Unknown,
                    _ => return None,
                }
            }
        };
        Some(cmd)
    }
}

/// One line of a configuration document.
#[derive(Clone, Debug)]
pub struct ConfigLine {
    pub command: Command,
    /// Parameter text after the colon, trimmed.
    pub params: String,
    pub line_no: usize,
    /// Original line including indentation.
    pub raw: String,
}

/// A parsed configuration document.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub lines: Vec<ConfigLine>,
    pub path: Option<PathBuf>,
    pub issues: Vec<Diagnostic>,
}

impl Document {
    /// Parses configuration text.
    pub fn parse(text: &str) -> Self {
        Self::parse_with_path(text, None)
    }

    pub fn parse_with_path(text: &str, path: Option<&Path>) -> Self {
        let mut doc = Document {
            lines: Vec::new(),
            path: path.map(|p| p.to_path_buf()),
            issues: Vec::new(),
        };
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            // A command is `Name: parameters`; lines without a colon are inert.
            let Some((name, params)) = trimmed.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let Some(command) = Command::parse(name) else {
                continue;
            };
            if command == Command::Unknown {
                doc.issues.push(Diagnostic::new(
                    Severity::Warning,
                    line_no,
                    format!("unknown command '{}' ignored (commands are case sensitive)", name),
                ));
                continue;
            }
            // `EndIf:` and `Else:` carry no parameters.
            doc.lines.push(ConfigLine {
                command,
                params: params.trim().to_string(),
                line_no,
                raw: raw.to_string(),
            });
        }
        doc
    }

    /// Loads and parses a file, recording I/O failures as diagnostics.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::parse_with_path(&text, Some(path)))
    }

    /// Compiler diagnostics collected while parsing.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_comments_and_unknown_commands() {
        let doc = Document::parse("# comment\nNonsense: foo\nPreamp: -6 dB\n");
        assert_eq!(doc.lines.len(), 1);
        assert_eq!(doc.lines[0].command, Command::Preamp);
        assert_eq!(doc.lines[0].params, "-6 dB");
    }

    #[test]
    fn parses_indexed_filter_commands() {
        let doc = Document::parse("Filter  1: ON  PK  Fc 50.0 Hz  Gain -10.0 dB  Q 2.5\n");
        assert_eq!(doc.lines.len(), 1);
        assert_eq!(doc.lines[0].command, Command::Filter);
        assert_eq!(doc.lines[0].params, "ON  PK  Fc 50.0 Hz  Gain -10.0 dB  Q 2.5");
    }

    #[test]
    fn records_lowercase_command_typos() {
        let doc = Document::parse("preamp: -3 dB\n");
        assert!(doc.lines.is_empty());
        assert_eq!(doc.issues.len(), 1);
        assert!(doc.issues[0].message.contains("unknown command"));
    }

    #[test]
    fn keeps_line_numbers() {
        let doc = Document::parse("\n\nPreamp: -1 dB\nChannel: L\n");
        assert_eq!(doc.lines[0].line_no, 3);
        assert_eq!(doc.lines[1].line_no, 4);
    }

    #[test]
    fn parses_block_commands_without_parameters() {
        let doc = Document::parse("If: 1\nElse:\nEndIf:\n");
        assert_eq!(doc.lines.len(), 3);
        assert_eq!(doc.lines[1].command, Command::Else);
        assert_eq!(doc.lines[2].command, Command::EndIf);
    }

    #[test]
    fn accept_tabs_and_indentation() {
        let doc = Document::parse("\t  Channel: L\n");
        assert_eq!(doc.lines[0].command, Command::Channel);
        assert_eq!(doc.lines[0].params, "L");
    }
}
