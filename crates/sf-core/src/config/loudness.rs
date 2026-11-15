//! Parser for the `LoudnessCorrection:` command.

use crate::dsp::loudness::LoudnessSettings;

/// Parses `State <0|1> ReferenceLevel <dB> [ReferenceOffset <dB>] [Attenuation <0..1>]`.
///
/// Keys are case sensitive, like every other command parameter in the format.
/// Unknown keys are an error rather than being ignored: a typo such as
/// `ReferenceLevl` must not silently produce a neutral filter.
pub fn parse_loudness(params: &str) -> Result<LoudnessSettings, String> {
    let tokens: Vec<&str> = params.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("expected 'State <0|1> ReferenceLevel <dB>', got no parameters".to_string());
    }
    let mut settings = LoudnessSettings::default();
    let mut seen_state = false;
    let mut seen_reference = false;
    let mut index = 0usize;
    while index < tokens.len() {
        let key = tokens[index];
        let value = tokens
            .get(index + 1)
            .ok_or_else(|| format!("'{key}' is missing its value"))?;
        match key {
            "State" => {
                let state = match *value {
                    "0" => false,
                    "1" => true,
                    other => {
                        return Err(format!("State must be 0 or 1, got '{other}'"));
                    }
                };
                if seen_state {
                    return Err("State is specified twice".into());
                }
                settings.state = state;
                seen_state = true;
            }
            "ReferenceLevel" => {
                if seen_reference {
                    return Err("ReferenceLevel is specified twice".into());
                }
                settings.reference_level = parse_number(value, "ReferenceLevel")?;
                seen_reference = true;
            }
            "ReferenceOffset" => {
                settings.reference_offset = parse_number(value, "ReferenceOffset")?;
            }
            "Attenuation" => {
                let value = parse_number(value, "Attenuation")?;
                if !(0.0..=1.0).contains(&value) {
                    return Err(format!("Attenuation must be between 0 and 1, got {value}"));
                }
                settings.attenuation = value;
            }
            other => {
                return Err(format!(
                    "unknown LoudnessCorrection parameter '{other}' (expected State, ReferenceLevel, ReferenceOffset or Attenuation)"
                ));
            }
        }
        // A unit may be written attached (`-20dB`) or separated (`-20 dB`).
        index += 2;
        if tokens
            .get(index)
            .is_some_and(|token| token.eq_ignore_ascii_case("db"))
        {
            index += 1;
        }
    }
    if !seen_state {
        return Err("LoudnessCorrection requires 'State <0|1>'".into());
    }
    if !seen_reference {
        return Err("LoudnessCorrection requires 'ReferenceLevel <dB>'".into());
    }
    Ok(settings)
}

fn parse_number(text: &str, name: &str) -> Result<f64, String> {
    // Be forgiving about a trailing unit, but not about garbage.
    let cleaned = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    cleaned
        .parse::<f64>()
        .map_err(|_| format!("{name} expects a number, got '{text}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_form() {
        let s =
            parse_loudness("State 1 ReferenceLevel -20 ReferenceOffset 0 Attenuation 1.0").unwrap();
        assert!(s.state);
        assert_eq!(s.reference_level, -20.0);
        assert_eq!(s.reference_offset, 0.0);
        assert_eq!(s.attenuation, 1.0);
    }

    #[test]
    fn defaults_offset_and_attenuation() {
        let s = parse_loudness("State 1 ReferenceLevel -25").unwrap();
        assert_eq!(s.reference_offset, 0.0);
        assert_eq!(s.attenuation, 1.0);
        assert_eq!(s.reference_level, -25.0);
    }

    #[test]
    fn accepts_units() {
        let s = parse_loudness("State 1 ReferenceLevel -20dB ReferenceOffset -5 dB").unwrap();
        assert_eq!(s.reference_level, -20.0);
        assert_eq!(s.reference_offset, -5.0);
    }

    #[test]
    fn state_zero_is_valid() {
        assert!(!parse_loudness("State 0 ReferenceLevel -20").unwrap().state);
    }

    #[test]
    fn rejects_unknown_keys_instead_of_ignoring_them() {
        let error = parse_loudness("State 1 ReferenceLevl -20").unwrap_err();
        assert!(
            error.contains("unknown LoudnessCorrection parameter"),
            "{error}"
        );
    }

    #[test]
    fn rejects_missing_required_keys() {
        assert!(parse_loudness("ReferenceLevel -20")
            .unwrap_err()
            .contains("State"));
        assert!(parse_loudness("State 1")
            .unwrap_err()
            .contains("ReferenceLevel"));
    }

    #[test]
    fn rejects_out_of_range_attenuation() {
        assert!(parse_loudness("State 1 ReferenceLevel -20 Attenuation 2")
            .unwrap_err()
            .contains("between 0 and 1"));
    }

    #[test]
    fn rejects_garbage_numbers() {
        assert!(parse_loudness("State 1 ReferenceLevel banana")
            .unwrap_err()
            .contains("expects a number"));
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(parse_loudness("State 1 State 0 ReferenceLevel -20")
            .unwrap_err()
            .contains("twice"));
    }
}
