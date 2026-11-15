//! Parsers for the individual command grammars of the configuration language.

use crate::dsp::filters::{Bandwidth, FilterKind, FilterSpec};
use crate::dsp::mixer::{MixAssignment, MixTerm};

/// Turns `50,0` into `50.0`, leaving other tokens untouched.
pub fn normalize_number(token: &str) -> String {
    let cleaned = token.trim();
    if cleaned.contains(',')
        && !cleaned.contains('.')
        && cleaned
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '-' || c == '+')
    {
        cleaned.replace(',', ".")
    } else {
        cleaned.to_string()
    }
}

/// Parses a floating point parameter, accepting the comma decimal separator
/// used by Room EQ Wizard exports.
pub fn parse_f64(token: &str) -> Option<f64> {
    let text = normalize_number(token);
    text.parse::<f64>().ok()
}

/// A parsed `Filter:` command.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterCommand {
    pub enabled: bool,
    pub kind: FilterKind,
    pub freq: f64,
    pub gain_db: f64,
    pub bandwidth: Bandwidth,
    /// Slope for `LSC n dB` / `HSC n dB` shelf variants.
    pub slope: f64,
    /// Generic IIR coefficients (b0..bm followed by a0..am).
    pub coefficients: Option<Vec<f64>>,
    /// Order of a generic IIR filter.
    pub order: Option<usize>,
}

impl FilterCommand {
    /// Converts to a designable spec. Returns `None` for bypass or generic IIR.
    pub fn to_spec(&self) -> Option<FilterSpec> {
        if self.kind == FilterKind::None || self.order.is_some() {
            return None;
        }
        Some(FilterSpec {
            kind: self.kind,
            freq: self.freq,
            gain_db: self.gain_db,
            bandwidth: self.bandwidth,
            slope: self.slope,
        })
    }
}

fn tokenize(params: &str) -> Vec<String> {
    params.split_whitespace().map(|t| t.to_string()).collect()
}

/// Parses the parameter part of a `Filter:` command.
pub fn parse_filter(params: &str) -> Result<FilterCommand, String> {
    let tokens = tokenize(params);
    if tokens.is_empty() {
        return Err("missing filter parameters".into());
    }
    let mut i = 0usize;
    let mut enabled = true;
    if tokens[i].eq_ignore_ascii_case("ON") {
        i += 1;
    } else if tokens[i].eq_ignore_ascii_case("OFF") {
        enabled = false;
        i += 1;
    }
    if i >= tokens.len() {
        return Err("missing filter type".into());
    }
    let mut cmd = FilterCommand {
        enabled,
        kind: FilterKind::None,
        freq: 0.0,
        gain_db: 0.0,
        bandwidth: Bandwidth::default(),
        slope: 12.0,
        coefficients: None,
        order: None,
    };

    let upper = tokens[i].to_uppercase();
    i += 1;
    match upper.as_str() {
        "PK" | "PEQ" | "PEAK" | "MODAL" => cmd.kind = FilterKind::Peak,
        "LP" => cmd.kind = FilterKind::LowPass,
        "LPQ" => cmd.kind = FilterKind::LowPass,
        "HP" => cmd.kind = FilterKind::HighPass,
        "HPQ" => cmd.kind = FilterKind::HighPass,
        "BP" => cmd.kind = FilterKind::BandPass,
        "NO" | "NOTCH" => cmd.kind = FilterKind::Notch,
        "AP" | "ALLPASS" => cmd.kind = FilterKind::AllPass,
        "LS" | "HS" => {
            let is_low = upper == "LS";
            let mut slope = 12.0;
            if i < tokens.len() {
                let probe = tokens[i].to_uppercase();
                if let Some(rest) = probe.strip_suffix("DB") {
                    if let Some(v) = parse_f64(rest) {
                        slope = v;
                        i += 1;
                    }
                }
            }
            cmd.slope = slope;
            cmd.kind = match (is_low, slope) {
                (true, s) if s <= 6.5 => FilterKind::LowShelf6,
                (false, s) if s <= 6.5 => FilterKind::HighShelf6,
                (true, _) => FilterKind::LowShelf12,
                (false, _) => FilterKind::HighShelf12,
            };
        }
        "LSC" | "HSC" => {
            let is_low = upper == "LSC";
            // Optional slope in dB per octave: `LSC 10.8 dB Fc 300 Hz ...`.
            if i + 1 < tokens.len() {
                if let Some(v) = parse_f64(&tokens[i]) {
                    if tokens[i + 1].eq_ignore_ascii_case("dB") {
                        cmd.slope = v;
                        i += 2;
                    }
                }
            }
            cmd.kind = if is_low {
                FilterKind::LowShelfSlope
            } else {
                FilterKind::HighShelfSlope
            };
        }
        "IIR" => {
            cmd.kind = FilterKind::None;
            // `Order <m> Coefficients <b0> ... <bm> <a0> ... <am>`
            while i < tokens.len() {
                let kw = tokens[i].to_uppercase();
                if kw == "ORDER" {
                    i += 1;
                    let order = tokens
                        .get(i)
                        .and_then(|t| parse_f64(t))
                        .ok_or_else(|| "missing IIR order".to_string())?;
                    cmd.order = Some(order.round() as usize);
                    i += 1;
                } else if kw == "COEFFICIENTS" {
                    i += 1;
                    let mut coeffs = Vec::new();
                    while i < tokens.len() {
                        match parse_f64(&tokens[i]) {
                            Some(v) => coeffs.push(v),
                            None => break,
                        }
                        i += 1;
                    }
                    cmd.coefficients = Some(coeffs);
                } else {
                    i += 1;
                }
            }
            let order = cmd
                .order
                .ok_or_else(|| "IIR filter without order".to_string())?;
            let coeffs = cmd.coefficients.clone().unwrap_or_default();
            if coeffs.len() != 2 * (order + 1) {
                return Err(format!(
                    "IIR order {order} needs {} coefficients, got {}",
                    2 * (order + 1),
                    coeffs.len()
                ));
            }
            return Ok(cmd);
        }
        "NONE" => {
            cmd.kind = FilterKind::None;
            return Ok(cmd);
        }
        other => return Err(format!("unknown filter type '{other}'")),
    }

    // Keyword/value pairs in any order.
    while i < tokens.len() {
        let kw = tokens[i].to_uppercase();
        match kw.as_str() {
            "FC" => {
                i += 1;
                cmd.freq = tokens
                    .get(i)
                    .and_then(|t| parse_f64(t))
                    .ok_or_else(|| "missing filter frequency".to_string())?;
                i += 1;
                if tokens
                    .get(i)
                    .map(|t| t.eq_ignore_ascii_case("Hz"))
                    .unwrap_or(false)
                {
                    i += 1;
                }
            }
            "GAIN" => {
                i += 1;
                cmd.gain_db = tokens
                    .get(i)
                    .and_then(|t| parse_f64(t))
                    .ok_or_else(|| "missing filter gain".to_string())?;
                i += 1;
                if tokens
                    .get(i)
                    .map(|t| t.eq_ignore_ascii_case("dB"))
                    .unwrap_or(false)
                {
                    i += 1;
                }
            }
            "Q" => {
                i += 1;
                let q = tokens
                    .get(i)
                    .and_then(|t| parse_f64(t))
                    .ok_or_else(|| "missing filter Q".to_string())?;
                cmd.bandwidth = Bandwidth::Q(q);
                i += 1;
            }
            "BW" => {
                i += 1;
                if tokens
                    .get(i)
                    .map(|t| t.eq_ignore_ascii_case("Oct"))
                    .unwrap_or(false)
                {
                    i += 1;
                }
                let bw = tokens
                    .get(i)
                    .and_then(|t| parse_f64(t))
                    .ok_or_else(|| "missing filter bandwidth".to_string())?;
                cmd.bandwidth = Bandwidth::Octaves(bw);
                i += 1;
            }
            "T60" => {
                // `T60 target 100 ms` is accepted and ignored: the reference
                // implementation also ignores the modal decay target.
                i += 1;
                if tokens
                    .get(i)
                    .map(|t| t.eq_ignore_ascii_case("target"))
                    .unwrap_or(false)
                {
                    i += 1;
                }
                i += 1;
                if tokens
                    .get(i)
                    .map(|t| t.eq_ignore_ascii_case("ms"))
                    .unwrap_or(false)
                {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    if cmd.freq <= 0.0 && cmd.kind != FilterKind::None {
        return Err("filter without a valid Fc".into());
    }
    Ok(cmd)
}

/// Parses `GraphicEQ: f g; f g; ...`.
pub fn parse_graphic_eq(params: &str) -> Result<Vec<(f64, f64)>, String> {
    let mut points = Vec::new();
    for band in params.split(';') {
        let band = band.trim();
        if band.is_empty() {
            continue;
        }
        let mut parts = band.split_whitespace();
        let f = parts
            .next()
            .and_then(parse_f64)
            .ok_or_else(|| format!("invalid graphic EQ frequency in '{band}'"))?;
        let g = parts
            .next()
            .and_then(parse_f64)
            .ok_or_else(|| format!("invalid graphic EQ gain in '{band}'"))?;
        points.push((f, g));
    }
    if points.is_empty() {
        return Err("graphic EQ without bands".into());
    }
    Ok(points)
}

/// A parsed `Delay:` command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DelayCommand {
    Milliseconds(f64),
    Samples(u32),
}

impl DelayCommand {
    /// Converts the delay into samples for a given rate.
    pub fn to_samples(self, sample_rate: u32) -> f64 {
        match self {
            DelayCommand::Milliseconds(ms) => ms.max(0.0) * sample_rate as f64 / 1000.0,
            DelayCommand::Samples(n) => n as f64,
        }
    }
}

/// Parses `Delay: 50.5 ms` or `Delay: 480 samples`.
pub fn parse_delay(params: &str) -> Result<DelayCommand, String> {
    let tokens = tokenize(params);
    let value = tokens
        .first()
        .and_then(|t| parse_f64(t))
        .ok_or_else(|| "missing delay value".to_string())?;
    match tokens.get(1).map(|s| s.to_lowercase()) {
        Some(unit) if unit == "ms" => Ok(DelayCommand::Milliseconds(value)),
        Some(unit) if unit == "samples" => Ok(DelayCommand::Samples(value.max(0.0).round() as u32)),
        Some(unit) => Err(format!("unknown delay unit '{unit}'")),
        None => Ok(DelayCommand::Milliseconds(value)),
    }
}

/// A parsed `Copy:` assignment with unresolved channel names.
#[derive(Clone, Debug, PartialEq)]
pub struct CopyCommand {
    pub target: String,
    pub terms: Vec<CopyTerm>,
}

/// One term of a `Copy:` assignment.
#[derive(Clone, Debug, PartialEq)]
pub struct CopyTerm {
    pub gain: f32,
    /// Source channel name, or `None` for a constant.
    pub source: Option<String>,
    pub constant: f32,
}

fn parse_gain(text: &str) -> Option<f32> {
    let lower = text.trim().to_lowercase();
    if let Some(db) = lower.strip_suffix("db") {
        let v = parse_f64(db.trim())?;
        return Some(10f32.powf(v as f32 / 20.0));
    }
    parse_f64(&lower).map(|v| v as f32)
}

/// Parses `Copy: L=L+0.5*R R=0.5`.
pub fn parse_copy(params: &str) -> Result<Vec<CopyCommand>, String> {
    let mut out = Vec::new();
    for assignment in params.split_whitespace() {
        let (target, expr) = assignment
            .split_once('=')
            .ok_or_else(|| format!("copy assignment '{assignment}' has no '='"))?;
        if target.is_empty() {
            return Err(format!(
                "copy assignment '{assignment}' has no target channel"
            ));
        }
        let terms = parse_copy_expression(expr).map_err(|e| format!("in '{assignment}': {e}"))?;
        out.push(CopyCommand {
            target: target.to_string(),
            terms,
        });
    }
    if out.is_empty() {
        return Err("empty copy command".into());
    }
    Ok(out)
}

fn parse_copy_expression(expr: &str) -> Result<Vec<CopyTerm>, String> {
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0usize;
    let mut terms = Vec::new();
    while i < chars.len() {
        let mut sign = 1.0f32;
        // Leading signs, e.g. `R+-6dB*C`.
        loop {
            match chars.get(i) {
                Some('+') => {
                    i += 1;
                }
                Some('-') => {
                    sign = -sign;
                    i += 1;
                }
                Some(c) if c.is_whitespace() => i += 1,
                _ => break,
            }
        }
        let start = i;
        while i < chars.len() && chars[i] != '+' && chars[i] != '-' {
            i += 1;
        }
        let body: String = chars[start..i].iter().collect();
        if body.is_empty() {
            continue;
        }
        let (factor, source) = match body.split_once('*') {
            Some((factor, source)) => {
                let trimmed = factor.trim().to_lowercase();
                let gain = if let Some(db) = trimmed.strip_suffix("db") {
                    // `-6dB` means a gain of -6 dB, not a negative linear factor.
                    let value =
                        parse_f64(db.trim()).ok_or_else(|| format!("bad gain '{factor}'"))?;
                    (10f64.powf(sign as f64 * value / 20.0)) as f32
                } else {
                    let value = parse_gain(factor).ok_or_else(|| format!("bad gain '{factor}'"))?;
                    sign * value
                };
                (gain, source)
            }
            None => (sign * 1.0f32, body.as_str()),
        };
        let sign = 1.0f32;
        let looks_constant = source.contains('.') && parse_f64(source).is_some();
        if looks_constant {
            let constant = parse_f64(source).unwrap_or(0.0) as f32;
            terms.push(CopyTerm {
                gain: sign * factor,
                source: None,
                constant,
            });
        } else {
            terms.push(CopyTerm {
                gain: sign * factor,
                source: Some(source.to_string()),
                constant: 0.0,
            });
        }
    }
    if terms.is_empty() {
        return Err("empty copy expression".into());
    }
    Ok(terms)
}

/// Converts a parsed copy command to a mixer assignment once channels resolve.
pub fn resolve_copy(
    cmd: &CopyCommand,
    resolve: impl Fn(&str) -> Option<usize>,
) -> Result<MixAssignment, String> {
    let target =
        resolve(&cmd.target).ok_or_else(|| format!("unknown target channel '{}'", cmd.target))?;
    let mut terms = Vec::new();
    for term in &cmd.terms {
        match &term.source {
            Some(name) => {
                let idx =
                    resolve(name).ok_or_else(|| format!("unknown source channel '{name}'"))?;
                terms.push(MixTerm {
                    gain: term.gain,
                    source: Some(idx),
                    constant: 0.0,
                });
            }
            None => terms.push(MixTerm {
                gain: term.gain,
                source: None,
                constant: term.constant,
            }),
        }
    }
    Ok(MixAssignment { target, terms })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rew_style_peak_filter() {
        let cmd = parse_filter("ON  PK       Fc    50,0 Hz  Gain -10,0 dB  Q  2,50").unwrap();
        assert!(cmd.enabled);
        assert_eq!(cmd.kind, FilterKind::Peak);
        assert!((cmd.freq - 50.0).abs() < 1e-9);
        assert!((cmd.gain_db + 10.0).abs() < 1e-9);
        assert_eq!(cmd.bandwidth, Bandwidth::Q(2.5));
    }

    #[test]
    fn parses_filters_without_number_or_units() {
        let cmd = parse_filter("ON HP Fc 30 Hz").unwrap();
        assert_eq!(cmd.kind, FilterKind::HighPass);
        assert_eq!(cmd.freq, 30.0);
        let cmd = parse_filter(
            "Filter: ON LS 6dB Fc 50.0 Hz Gain 7.2 dB"
                .trim_start_matches("Filter: ")
                .trim(),
        )
        .unwrap();
        assert_eq!(cmd.kind, FilterKind::LowShelf6);
        assert_eq!(cmd.slope, 6.0);
    }

    #[test]
    fn parses_twelve_db_shelf_default() {
        let cmd = parse_filter("ON LS Fc 300 Hz Gain 5.0 dB").unwrap();
        assert_eq!(cmd.kind, FilterKind::LowShelf12);
        let cmd = parse_filter("ON HS 12dB Fc 500 Hz Gain 5.0 dB").unwrap();
        assert_eq!(cmd.kind, FilterKind::HighShelf12);
    }

    #[test]
    fn parses_shelf_with_slope() {
        let cmd = parse_filter("ON LSC 10.8 dB Fc 300 Hz Gain 5.0 dB").unwrap();
        assert_eq!(cmd.kind, FilterKind::LowShelfSlope);
        assert!((cmd.slope - 10.8).abs() < 1e-9);
        assert_eq!(cmd.freq, 300.0);
        let cmd = parse_filter("ON LSC Fc 300 Hz Gain 5.0 dB Q 0.6473").unwrap();
        assert_eq!(cmd.kind, FilterKind::LowShelfSlope);
        assert_eq!(cmd.slope, 12.0);
        assert_eq!(cmd.bandwidth, Bandwidth::Q(0.6473));
    }

    #[test]
    fn parses_bandwidth_in_octaves() {
        let cmd = parse_filter("ON PEQ Fc 100 Hz Gain 1.0 dB BW Oct 0.167").unwrap();
        assert_eq!(cmd.bandwidth, Bandwidth::Octaves(0.167));
    }

    #[test]
    fn parses_generic_iir() {
        let cmd = parse_filter(
            "ON IIR Order 2 Coefficients 0.0380602 0.0761205 0.0380602 1.2706 -1.84776 0.729402",
        )
        .unwrap();
        assert_eq!(cmd.order, Some(2));
        assert_eq!(cmd.coefficients.as_ref().unwrap().len(), 6);
    }

    #[test]
    fn rejects_iir_with_wrong_coefficient_count() {
        assert!(parse_filter("ON IIR Order 3 Coefficients 1 2 3 4 5 6").is_err());
    }

    #[test]
    fn disabled_filters_are_recognised() {
        let cmd = parse_filter("OFF None").unwrap();
        assert!(!cmd.enabled);
        assert_eq!(cmd.kind, FilterKind::None);
    }

    #[test]
    fn modal_filter_with_t60_target() {
        let cmd =
            parse_filter("ON  Modal    Fc     100 Hz  Gain   3.0 dB  Q  5.41  T60 target   100 ms")
                .unwrap();
        assert_eq!(cmd.kind, FilterKind::Peak);
        assert_eq!(cmd.freq, 100.0);
        assert_eq!(cmd.gain_db, 3.0);
    }

    #[test]
    fn parses_graphic_eq_bands() {
        let pts = parse_graphic_eq("25 0; 40 0; 63 0; 100 0").unwrap();
        assert_eq!(
            pts,
            vec![(25.0, 0.0), (40.0, 0.0), (63.0, 0.0), (100.0, 0.0)]
        );
    }

    #[test]
    fn parses_delay_in_both_units() {
        assert_eq!(
            parse_delay("50.5 ms").unwrap(),
            DelayCommand::Milliseconds(50.5)
        );
        assert_eq!(
            parse_delay("480 samples").unwrap(),
            DelayCommand::Samples(480)
        );
        assert_eq!(parse_delay("50.5").unwrap().to_samples(48000), 50.5 * 48.0);
    }

    #[test]
    fn parses_copy_with_factors_and_constants() {
        let cmds = parse_copy("L=L+0.5*R R=0.5").unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].target, "L");
        assert_eq!(cmds[0].terms.len(), 2);
        assert_eq!(cmds[0].terms[1].gain, 0.5);
        assert_eq!(cmds[0].terms[1].source.as_deref(), Some("R"));
        assert_eq!(cmds[1].terms[0].source, None);
        assert_eq!(cmds[1].terms[0].constant, 0.5);
    }

    #[test]
    fn parses_negative_db_gain_in_copy() {
        let cmds = parse_copy("L=R+-6dB*C").unwrap();
        let term = &cmds[0].terms[1];
        assert!(term.source.is_some());
        assert!((term.gain - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        assert!(cmds[0].terms[0].gain > 0.0);
    }

    #[test]
    fn numeric_source_channel_is_not_a_constant() {
        let cmds = parse_copy("L=2").unwrap();
        assert_eq!(cmds[0].terms[0].source.as_deref(), Some("2"));
        assert_eq!(cmds[0].terms[0].constant, 0.0);
    }

    #[test]
    fn resolves_copy_to_channel_indices() {
        let cmds = parse_copy("L=L+0.5*R").unwrap();
        let assignment = resolve_copy(&cmds[0], |name| match name {
            "L" => Some(0),
            "R" => Some(1),
            _ => None,
        })
        .unwrap();
        assert_eq!(assignment.target, 0);
        assert_eq!(assignment.terms[1].source, Some(1));
    }
}
