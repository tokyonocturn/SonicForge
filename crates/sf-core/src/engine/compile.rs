//! The configuration compiler.
//!
//! Walks the document in file order while tracking the current device filter,
//! channel selection, stage selection and conditional state, producing a
//! [`Pipeline`] plus diagnostics. Every command is interpreted exactly once, so
//! `Eval:` variables and inline expressions behave like the reference format.

use super::Stage;
use crate::config::expr::{expand_inline, Env};
use crate::config::filter::{
    parse_copy, parse_delay, parse_filter, parse_graphic_eq, resolve_copy,
};
use crate::config::parser::{Command, ConfigLine, Diagnostic, Document, Severity};
use crate::config::preset::Preset;
use crate::device::{matches_device_list, ChannelLayout, DeviceInfo};
use crate::dsp::biquad::IirFilter;
use crate::dsp::convolver::{Convolver, DEFAULT_BLOCK};
use crate::dsp::delay::DelayLine;
use crate::dsp::filters::design;
use crate::dsp::graphic_eq::design_graphic_eq;
use crate::dsp::pipeline::{ChannelMask, Op, Pipeline};
use crate::wav::read_wave;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Options controlling compilation.
#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub sample_rate: u32,
    /// Number of channels of the device stream.
    pub channels: usize,
    pub device: DeviceInfo,
    /// Which APO stage is being compiled.
    pub stage: Stage,
    /// Maximum `Include:` nesting depth.
    pub max_include_depth: usize,
    /// Convolution partition size (power of two).
    pub convolution_block: usize,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            device: DeviceInfo::default(),
            stage: Stage::PostMix,
            max_include_depth: 16,
            convolution_block: DEFAULT_BLOCK,
        }
    }
}

/// The result of compiling a document.
#[derive(Clone, Debug)]
pub struct CompiledChain {
    pub pipeline: Pipeline,
    pub diagnostics: Vec<Diagnostic>,
    /// The editable subset of the configuration, when it can be represented.
    pub preset: Preset,
}

impl CompiledChain {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
}

/// Compiles parsed configuration text.
pub fn compile_document(doc: &Document, options: &CompileOptions) -> CompiledChain {
    let mut compiler = Compiler::new(options);
    for issue in &doc.issues {
        compiler.diagnostics.push(issue.clone());
    }
    compiler.run_lines(&doc.lines, doc.path.as_deref());
    compiler.finish(doc)
}

/// Loads and compiles a configuration file.
pub fn compile_file(path: &Path, options: &CompileOptions) -> std::io::Result<CompiledChain> {
    let doc = Document::load(path)?;
    Ok(compile_document(&doc, options))
}

struct Compiler<'a> {
    options: &'a CompileOptions,
    env: Env,
    pipeline: Pipeline,
    diagnostics: Vec<Diagnostic>,
    device_active: bool,
    channel_mask: ChannelMask,
    stage_active: bool,
    include_stack: Vec<PathBuf>,
    current_file: Option<PathBuf>,
    virtual_channels: HashMap<String, usize>,
    layout: ChannelLayout,
    max_channels: usize,
}

impl<'a> Compiler<'a> {
    fn new(options: &'a CompileOptions) -> Self {
        let channels = options.channels.max(1);
        let pipeline = Pipeline::new(options.sample_rate, channels);
        let env = Env::for_stream(
            options.sample_rate,
            channels,
            channels,
            &options.device.name,
            &options.device.connection,
            &options.device.guid,
            options.stage.as_str(),
        );
        Self {
            options,
            env,
            pipeline,
            diagnostics: Vec::new(),
            device_active: true,
            channel_mask: if channels >= 64 {
                u64::MAX
            } else {
                (1u64 << channels) - 1
            },
            stage_active: true,
            include_stack: Vec::new(),
            current_file: None,
            virtual_channels: HashMap::new(),
            layout: ChannelLayout::from_count(channels),
            max_channels: channels,
        }
    }

    fn error(&mut self, line: usize, message: impl Into<String>) {
        self.push(Severity::Error, line, message);
    }

    fn warn(&mut self, line: usize, message: impl Into<String>) {
        self.push(Severity::Warning, line, message);
    }

    fn push(&mut self, severity: Severity, line: usize, message: impl Into<String>) {
        let mut d = Diagnostic::new(severity, line, message);
        if let Some(file) = &self.current_file {
            d = d.with_file(file.clone());
        }
        self.diagnostics.push(d);
    }

    /// Resolves a channel token, creating virtual channels on demand.
    fn resolve_channel(&mut self, token: &str) -> Option<usize> {
        let total = self.pipeline.channels().max(self.max_channels);
        if let Some(idx) = self.layout.resolve(token, total) {
            return Some(idx);
        }
        if token.eq_ignore_ascii_case("all") {
            return Some(usize::MAX);
        }
        // Virtual channel names (for example `L2` in the shipped selective
        // delay example) are allocated beyond the device channels.
        if let Some(&idx) = self.virtual_channels.get(token) {
            return Some(idx);
        }
        let looks_virtual = token.len() >= 2
            && token
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic())
                .unwrap_or(false)
            && token.chars().any(|c| c.is_ascii_digit());
        if looks_virtual {
            let idx = self.max_channels;
            self.max_channels += 1;
            self.virtual_channels.insert(token.to_string(), idx);
            let total = self.max_channels;
            self.pipeline.set_channels(total);
            return Some(idx);
        }
        None
    }

    fn active(&self) -> bool {
        self.device_active && self.stage_active
    }

    fn expand(&mut self, params: &str, line: usize) -> Option<String> {
        match expand_inline(params, &mut self.env) {
            Ok(s) => Some(s),
            Err(e) => {
                self.error(line, format!("expression error: {e}"));
                None
            }
        }
    }

    fn run_lines(&mut self, lines: &[ConfigLine], file: Option<&Path>) {
        let saved_file = self.current_file.clone();
        if let Some(path) = file {
            self.current_file = Some(path.to_path_buf());
        }
        let mut i = 0usize;
        while i < lines.len() {
            match lines[i].command {
                Command::If => {
                    let (branches, end) = analyze_if(lines, i);
                    let mut taken = false;
                    for (condition, start, finish) in &branches {
                        let matches = match condition {
                            Some(cond) => {
                                let cond = cond.clone();
                                match self.env.eval(&cond) {
                                    Ok(v) => v.as_bool(),
                                    Err(e) => {
                                        self.error(
                                            lines[*start].line_no,
                                            format!("invalid condition: {e}"),
                                        );
                                        false
                                    }
                                }
                            }
                            None => true,
                        };
                        if !taken && matches {
                            taken = true;
                            let body = &lines[*start..*finish];
                            self.run_lines(body, None);
                        }
                    }
                    i = end + 1;
                }
                Command::ElseIf | Command::Else | Command::EndIf => {
                    // Stray block commands outside an If: ignore with a warning.
                    self.warn(lines[i].line_no, "block command without a matching If");
                    i += 1;
                }
                _ => {
                    self.run_line(&lines[i]);
                    i += 1;
                }
            }
        }
        self.current_file = saved_file;
    }

    fn run_line(&mut self, line: &ConfigLine) {
        let line_no = line.line_no;
        match line.command {
            Command::Device => {
                // Device state changes even while a stage is not selected, and
                // it is evaluated from the raw parameter text.
                self.device_active = matches_device_list(&line.params, &self.options.device);
            }
            Command::Channel => {
                let tokens: Vec<String> = line
                    .params
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                if tokens.is_empty() {
                    self.warn(line_no, "Channel command without channels");
                    return;
                }
                let mut mask = 0u64;
                let mut all = false;
                for token in tokens {
                    let resolved = self.resolve_channel(&token);
                    match resolved {
                        Some(usize::MAX) => all = true,
                        Some(idx) => {
                            if idx < 64 {
                                mask |= 1 << idx;
                            }
                        }
                        None => self.warn(line_no, format!("unknown channel '{token}'")),
                    }
                }
                if all || mask == 0 {
                    let channels = self.pipeline.channels().min(64);
                    mask = if channels >= 64 {
                        u64::MAX
                    } else {
                        (1u64 << channels) - 1
                    };
                }
                self.channel_mask = mask;
            }
            Command::Stage => {
                let mut stages = Vec::new();
                for token in line.params.split_whitespace() {
                    match Stage::parse(token) {
                        Some(s) => stages.push(s),
                        None => self.warn(line_no, format!("unknown stage '{token}'")),
                    }
                }
                if stages.is_empty() {
                    self.stage_active = true;
                } else {
                    self.stage_active = stages.contains(&self.options.stage);
                }
            }
            _ if !self.active() => {}
            Command::Preamp => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                match params
                    .split_whitespace()
                    .next()
                    .and_then(crate::config::filter::parse_f64)
                {
                    Some(db) => {
                        let gain = 10f64.powf(db / 20.0) as f32;
                        self.pipeline.push(Op::Gain {
                            mask: self.channel_mask,
                            gain,
                        });
                    }
                    None => self.error(line_no, format!("invalid preamp value '{}'", line.params)),
                }
            }
            Command::Filter => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                match parse_filter(&params) {
                    Ok(cmd) => {
                        if !cmd.enabled {
                            return;
                        }
                        if let Some(coeffs) = &cmd.coefficients {
                            let order = cmd.order.unwrap_or(0);
                            let b = coeffs[..=order].to_vec();
                            let a = coeffs[order + 1..].to_vec();
                            match IirFilter::new(&b, &a) {
                                Some(filter) => self.pipeline.push(Op::Iir {
                                    mask: self.channel_mask,
                                    filter,
                                    states: Vec::new(),
                                }),
                                None => self.error(line_no, "invalid IIR coefficients"),
                            }
                            return;
                        }
                        if cmd.kind == crate::dsp::filters::FilterKind::None {
                            return;
                        }
                        match cmd.to_spec().and_then(|spec| design(&spec, self.options.sample_rate as f64)) {
                            Some(bq) => self.pipeline.push(Op::Biquad {
                                mask: self.channel_mask,
                                cascade: vec![bq],
                                states: Vec::new(),
                            }),
                            None => self.warn(
                                line_no,
                                format!(
                                    "filter at {} Hz cannot be represented at {} Hz and was skipped",
                                    cmd.freq, self.options.sample_rate
                                ),
                            ),
                        }
                    }
                    Err(e) => self.error(line_no, format!("invalid Filter command: {e}")),
                }
            }
            Command::GraphicEq => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                match parse_graphic_eq(&params) {
                    Ok(points) => {
                        if points.len() > crate::dsp::graphic_eq::MAX_POINTS {
                            self.warn(
                                line_no,
                                format!(
                                    "graphic EQ has more than {} bands; extra bands ignored",
                                    crate::dsp::graphic_eq::MAX_POINTS
                                ),
                            );
                        }
                        let cascade = design_graphic_eq(&points, self.options.sample_rate as f64);
                        if cascade.is_empty() {
                            self.warn(line_no, "graphic EQ has no usable bands");
                        } else {
                            self.pipeline.push(Op::Biquad {
                                mask: self.channel_mask,
                                cascade,
                                states: Vec::new(),
                            });
                        }
                    }
                    Err(e) => self.error(line_no, format!("invalid GraphicEQ command: {e}")),
                }
            }
            Command::Convolution => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                let file_name = params.trim().trim_matches('"');
                if file_name.is_empty() {
                    self.error(line_no, "Convolution without a file name");
                    return;
                }
                let base = self
                    .current_file
                    .as_ref()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."));
                let path = if Path::new(file_name).is_absolute() {
                    PathBuf::from(file_name)
                } else {
                    base.join(file_name)
                };
                match read_wave(&path) {
                    Ok(audio) => {
                        if audio.sample_rate != self.options.sample_rate {
                            self.error(
                                line_no,
                                format!(
                                    "impulse response '{}' is {} Hz but the device runs at {} Hz",
                                    path.display(),
                                    audio.sample_rate,
                                    self.options.sample_rate
                                ),
                            );
                            return;
                        }
                        if audio.channel_count() == 0 || audio.frames() == 0 {
                            self.error(
                                line_no,
                                format!("impulse response '{}' is empty", path.display()),
                            );
                            return;
                        }
                        let mut convolvers = Vec::new();
                        let mut any = false;
                        for (slot, ch) in (0..self.pipeline.channels().min(64)).enumerate() {
                            if self.channel_mask & (1 << ch) == 0 {
                                continue;
                            }
                            let src = slot % audio.channel_count();
                            let ir = &audio.channels[src];
                            convolvers.push(Convolver::new(ir, self.options.convolution_block));
                            any = true;
                        }
                        if any {
                            self.pipeline.push(Op::Convolver {
                                mask: self.channel_mask,
                                convolvers,
                                scratch: Vec::new(),
                            });
                        }
                    }
                    Err(e) => self.error(line_no, format!("cannot load '{}': {e}", path.display())),
                }
            }
            Command::Delay => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                match parse_delay(&params) {
                    Ok(delay) => {
                        let samples = delay.to_samples(self.options.sample_rate);
                        if samples
                            > crate::dsp::delay::MAX_DELAY_SECONDS * self.options.sample_rate as f64
                        {
                            self.error(
                                line_no,
                                format!(
                                    "delay of {samples} samples exceeds the {} s limit",
                                    crate::dsp::delay::MAX_DELAY_SECONDS
                                ),
                            );
                            return;
                        }
                        let mut lines = Vec::new();
                        let mut count = 0;
                        for ch in 0..self.pipeline.channels().min(64) {
                            if self.channel_mask & (1 << ch) != 0 {
                                lines.push(DelayLine::new(samples));
                                count += 1;
                            }
                        }
                        if count > 0 {
                            self.pipeline.push(Op::Delay {
                                mask: self.channel_mask,
                                lines,
                            });
                        }
                    }
                    Err(e) => self.error(line_no, format!("invalid Delay command: {e}")),
                }
            }
            Command::Copy => {
                let Some(params) = self.expand(&line.params, line_no) else {
                    return;
                };
                match parse_copy(&params) {
                    Ok(cmds) => {
                        let mut assignments = Vec::new();
                        let mut highest = 0usize;
                        for cmd in &cmds {
                            // Allocate the target first so `L2=...` exists.
                            let _ = self.resolve_channel(&cmd.target);
                            let mut resolver_names: Vec<String> = vec![cmd.target.clone()];
                            for term in &cmd.terms {
                                if let Some(src) = &term.source {
                                    resolver_names.push(src.clone());
                                }
                            }
                            for name in &resolver_names {
                                if self.resolve_channel(name).is_none() {
                                    self.warn(line_no, format!("unknown channel '{name}' in Copy"));
                                }
                            }
                            let virtual_map = self.virtual_channels.clone();
                            let layout = self.layout;
                            let total = self.pipeline.channels().max(self.max_channels);
                            let resolve = |name: &str| -> Option<usize> {
                                if let Some(idx) = layout.resolve(name, total) {
                                    return Some(idx);
                                }
                                virtual_map.get(name).copied()
                            };
                            match resolve_copy(cmd, resolve) {
                                Ok(a) => {
                                    highest = highest.max(a.max_channel());
                                    assignments.push(a);
                                }
                                Err(e) => self.error(line_no, format!("invalid Copy command: {e}")),
                            }
                        }
                        if !assignments.is_empty() {
                            let needed = (highest + 1)
                                .max(self.max_channels)
                                .max(self.pipeline.channels());
                            if needed > self.pipeline.channels() {
                                self.pipeline.set_channels(needed);
                                self.max_channels = needed;
                            }
                            self.pipeline.push(Op::Mix { assignments });
                        }
                    }
                    Err(e) => self.error(line_no, format!("invalid Copy command: {e}")),
                }
            }
            Command::Include => {
                let name = line.params.trim().trim_matches('"');
                let base = self
                    .current_file
                    .as_ref()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."));
                let path = if Path::new(name).is_absolute() {
                    PathBuf::from(name)
                } else {
                    base.join(name)
                };
                let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if self.include_stack.iter().any(|p| p == &canonical) {
                    self.error(line_no, format!("circular Include of '{}'", path.display()));
                    return;
                }
                if self.include_stack.len() >= self.options.max_include_depth {
                    self.error(
                        line_no,
                        format!(
                            "Include depth limit ({}) reached",
                            self.options.max_include_depth
                        ),
                    );
                    return;
                }
                match Document::load(&path) {
                    Ok(doc) => {
                        for issue in &doc.issues {
                            self.diagnostics.push(issue.clone());
                        }
                        self.include_stack.push(canonical);
                        let file_for_lines = Some(path.clone());
                        self.run_lines(&doc.lines, file_for_lines.as_deref());
                        self.include_stack.pop();
                    }
                    Err(e) => {
                        self.error(line_no, format!("cannot include '{}': {e}", path.display()))
                    }
                }
            }
            Command::Eval => {
                let params = line.params.clone();
                if let Err(e) = self.env.eval(&params) {
                    self.error(line_no, format!("Eval error: {e}"));
                }
            }
            Command::VstPlugin => {
                self.error(line_no, "VST plugin hosting is not available in this build");
            }
            Command::LoudnessCorrection => {
                self.warn(
                    line_no,
                    "LoudnessCorrection is not implemented and was ignored",
                );
            }
            Command::Comment | Command::Unknown => {}
            Command::If | Command::ElseIf | Command::Else | Command::EndIf => {}
        }
    }

    fn finish(mut self, doc: &Document) -> CompiledChain {
        // Keep the pipeline channel count consistent with what the config used.
        let channels = self.pipeline.channels().max(self.max_channels);
        if channels != self.pipeline.channels() {
            self.pipeline.set_channels(channels);
            self.env.set(
                "outputChannelCount",
                crate::config::Value::Int(channels as i64),
            );
        }
        self.diagnostics.sort_by(|a, b| a.line.cmp(&b.line));
        CompiledChain {
            pipeline: self.pipeline,
            diagnostics: self.diagnostics,
            preset: Preset::from_document(doc),
        }
    }
}

/// Returns the branch ranges of an `If` block starting at `start`, plus the
/// index of the closing `EndIf`.
fn analyze_if(lines: &[ConfigLine], start: usize) -> (Vec<(Option<String>, usize, usize)>, usize) {
    let mut branches = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<String> = Some(lines[start].params.clone());
    let mut branch_start = start + 1;
    let mut i = start + 1;
    while i < lines.len() {
        match lines[i].command {
            Command::If => depth += 1,
            Command::EndIf => {
                if depth == 0 {
                    branches.push((current.take(), branch_start, i));
                    return (branches, i);
                }
                depth -= 1;
            }
            Command::ElseIf if depth == 0 => {
                branches.push((current.take(), branch_start, i));
                current = Some(lines[i].params.clone());
                branch_start = i + 1;
            }
            Command::Else if depth == 0 => {
                branches.push((current.take(), branch_start, i));
                current = None;
                branch_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    // Unterminated block: treat everything to the end as the branch.
    branches.push((current, branch_start, lines.len()));
    (branches, lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::pipeline::Op;

    fn opts() -> CompileOptions {
        CompileOptions {
            sample_rate: 48000,
            channels: 2,
            ..Default::default()
        }
    }

    fn compile(text: &str) -> CompiledChain {
        compile_document(&Document::parse(text), &opts())
    }

    #[test]
    fn preamp_becomes_gain_stage() {
        let chain = compile("Preamp: -6 dB\n");
        assert_eq!(chain.pipeline.ops().len(), 1);
        match &chain.pipeline.ops()[0] {
            Op::Gain { gain, mask } => {
                assert!((*gain - 0.5011872).abs() < 1e-6);
                assert_eq!(*mask, 0b11);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn multiple_preamps_are_summed() {
        let chain = compile("Preamp: -6 dB\nChannel: L\nPreamp: -6 dB\n");
        let total: f64 = chain.pipeline.static_gain();
        assert!((total - 0.2511886).abs() < 1e-4, "total gain {total}");
    }

    #[test]
    fn filter_applies_only_to_selected_channels() {
        let chain = compile("Channel: L\nFilter: ON PK Fc 1000 Hz Gain 6 dB Q 1.0\n");
        match &chain.pipeline.ops()[0] {
            Op::Biquad { mask, cascade, .. } => {
                assert_eq!(*mask, 0b01);
                assert_eq!(cascade.len(), 1);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn disabled_filters_are_skipped() {
        let chain = compile("Filter: OFF PK Fc 1000 Hz Gain 6 dB Q 1.0\n");
        assert!(chain.pipeline.ops().is_empty());
    }

    #[test]
    fn device_selector_suppresses_other_devices() {
        let mut o = opts();
        o.device = DeviceInfo::new("Speakers", "Speakers", "guid");
        let text = "Device: Headphones\nPreamp: -6 dB\nDevice: Speakers\nPreamp: -3 dB\n";
        let chain = compile_document(&Document::parse(text), &o);
        assert_eq!(chain.pipeline.ops().len(), 1);
        assert!((chain.pipeline.static_gain() - 0.7079458).abs() < 1e-5);
    }

    #[test]
    fn conditionals_select_one_branch() {
        let text = "If: sampleRate == 44100\nPreamp: -1 dB\nElseIf: sampleRate == 48000\nPreamp: -2 dB\nElse:\nPreamp: -3 dB\nEndIf:\n";
        let chain = compile(text);
        assert_eq!(chain.pipeline.ops().len(), 1);
        assert!((chain.pipeline.static_gain() - 0.7943282).abs() < 1e-5);
    }

    #[test]
    fn nested_conditionals_work() {
        let text = "If: 1\nIf: 0\nPreamp: -99 dB\nElse:\nPreamp: -2 dB\nEndIf:\nEndIf:\n";
        let chain = compile(text);
        assert_eq!(chain.pipeline.ops().len(), 1);
        assert!((chain.pipeline.static_gain() - 0.7943282).abs() < 1e-5);
    }

    #[test]
    fn eval_variables_feed_inline_expressions() {
        let text =
            "Eval: linGain = 0.5\nFilter: ON PK Fc 1000 Hz Gain `20*log10(linGain)` dB Q 10.0\n";
        let chain = compile(text);
        assert_eq!(chain.pipeline.ops().len(), 1);
        assert!(chain.diagnostics.is_empty(), "{:?}", chain.diagnostics);
    }

    #[test]
    fn stage_selection_filters_commands() {
        let text = "Stage: pre-mix\nPreamp: -9 dB\nStage: post-mix\nPreamp: -3 dB\n";
        let chain = compile(text);
        assert_eq!(chain.pipeline.ops().len(), 1);
        assert!((chain.pipeline.static_gain() - 0.7079458).abs() < 1e-5);
    }

    #[test]
    fn graphic_eq_produces_cascade() {
        let chain = compile("GraphicEQ: 100 3; 1000 -3; 10000 2\n");
        match &chain.pipeline.ops()[0] {
            Op::Biquad { cascade, .. } => assert_eq!(cascade.len(), 3),
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn generic_iir_filter_is_compiled() {
        let chain = compile("Filter: ON IIR Order 2 Coefficients 0.0380602 0.0761205 0.0380602 1.2706 -1.84776 0.729402\n");
        assert!(matches!(chain.pipeline.ops()[0], Op::Iir { .. }));
    }

    #[test]
    fn delay_is_compiled_for_all_channels() {
        let chain = compile("Delay: 5 ms\n");
        match &chain.pipeline.ops()[0] {
            Op::Delay { lines, .. } => {
                assert_eq!(lines.len(), 2);
                assert!((lines[0].delay_samples() - 240.0).abs() < 1e-9);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn copy_expands_virtual_channels() {
        let chain =
            compile("Copy: L2=L R2=R\nChannel: L2 R2\nDelay: 10 ms\nCopy: L=0.5*L+0.5*L2\n");
        assert_eq!(chain.pipeline.channels(), 4);
        assert!(
            chain
                .diagnostics
                .iter()
                .all(|d| d.severity != Severity::Error),
            "{:?}",
            chain.diagnostics
        );
        assert_eq!(chain.pipeline.ops().len(), 3);
    }

    #[test]
    fn invalid_commands_produce_errors_not_panics() {
        let chain = compile("Filter: ON PK Fc\nPreamp: abc dB\nCopy: L\nDelay: x ms\n");
        assert!(chain.error_count() >= 4, "{:?}", chain.diagnostics);
    }

    #[test]
    fn convolution_rejects_wrong_sample_rate() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("ir.wav");
        crate::wav::write_wave_f32(&ir_path, 44100, &[vec![1.0f32; 32]]).unwrap();
        let text = format!("Convolution: {}\n", ir_path.display());
        let mut o = opts();
        o.sample_rate = 48000;
        let chain = compile_document(&Document::parse(&text), &o);
        assert!(chain.error_count() >= 1);
        assert!(chain.diagnostics[0].message.contains("44100"));
    }

    #[test]
    fn convolution_matching_rate_is_compiled() {
        let dir = tempfile::tempdir().unwrap();
        let ir_path = dir.path().join("ir.wav");
        crate::wav::write_wave_f32(&ir_path, 48000, &[vec![1.0f32; 32], vec![0.5f32; 32]]).unwrap();
        let text = format!("Convolution: {}\n", ir_path.display());
        let chain = compile_document(&Document::parse(&text), &opts());
        assert!(chain.error_count() == 0, "{:?}", chain.diagnostics);
        match &chain.pipeline.ops()[0] {
            Op::Convolver { convolvers, .. } => assert_eq!(convolvers.len(), 2),
            other => panic!("unexpected op {other:?}"),
        }
        assert!(chain.pipeline.latency_samples() > 0);
    }

    #[test]
    fn include_loads_relative_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inner.txt"), "Preamp: -3 dB\n").unwrap();
        std::fs::write(
            dir.path().join("config.txt"),
            "Channel: L\nInclude: inner.txt\n",
        )
        .unwrap();
        let chain = compile_file(&dir.path().join("config.txt"), &opts()).unwrap();
        assert_eq!(chain.pipeline.ops().len(), 1);
        match &chain.pipeline.ops()[0] {
            Op::Gain { mask, .. } => assert_eq!(*mask, 0b01),
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn circular_includes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "Include: b.txt\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "Include: a.txt\n").unwrap();
        let chain = compile_file(&dir.path().join("a.txt"), &opts()).unwrap();
        assert!(chain.error_count() >= 1);
        assert!(chain
            .diagnostics
            .iter()
            .any(|d| d.message.contains("circular")));
    }

    #[test]
    fn missing_include_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.txt"), "Include: nope.txt\n").unwrap();
        let chain = compile_file(&dir.path().join("config.txt"), &opts()).unwrap();
        assert_eq!(chain.error_count(), 1);
        assert_eq!(chain.pipeline.ops().len(), 0);
    }

    #[test]
    fn multi_channel_configuration_works() {
        let mut o = opts();
        o.channels = 6;
        let text = "Channel: LFE\nPreamp: -3 dB\nChannel: L R\nPreamp: -6 dB\n";
        let chain = compile_document(&Document::parse(text), &o);
        assert_eq!(chain.pipeline.channels(), 6);
        assert_eq!(chain.pipeline.ops().len(), 2);
        assert!((chain.pipeline.static_gain() - 0.3548134).abs() < 1e-5);
    }

    #[test]
    fn shipped_example_configs_compile() {
        // Every shipped example must compile without errors at a common rate.
        let examples = [
            "Preamp: -6 dB\nGraphicEQ: 25 0; 40 0; 63 0; 100 0; 160 0; 250 0; 400 0; 630 0; 1000 0; 1600 0; 2500 0; 4000 0; 6300 0; 10000 0; 16000 0\n",
            "Preamp: -6 dB\nChannel: L\nPreamp: -5 dB\nChannel: 2 C\nPreamp: -1 dB\n",
        ];
        for text in examples {
            let chain = compile(text);
            assert_eq!(chain.error_count(), 0, "{:?}", chain.diagnostics);
        }
    }
}
