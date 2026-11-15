//! Coordinate mapping for the frequency response graph.
//!
//! Kept separate from the UI so the mapping can be unit tested: an off-by-one
//! in a log axis is the classic way a "flat" curve ends up looking wrong.

/// Lowest displayed frequency, Hz.
pub const MIN_FREQ: f64 = 20.0;
/// Highest displayed frequency, Hz.
pub const MAX_FREQ: f64 = 20000.0;
/// Half height of the graph in dB (so the visible range is +/- this value).
pub const DB_RANGE: f64 = 24.0;

#[inline]
fn log_min() -> f64 {
    MIN_FREQ.ln()
}

#[inline]
fn log_span() -> f64 {
    (MAX_FREQ / MIN_FREQ).ln()
}

/// Normalised horizontal position in `0..1` for a frequency.
pub fn freq_to_unit(freq: f64) -> f64 {
    let f = freq.clamp(MIN_FREQ, MAX_FREQ);
    (f.ln() - log_min()) / log_span()
}

/// Frequency for a normalised horizontal position.
pub fn unit_to_freq(unit: f64) -> f64 {
    let unit = unit.clamp(0.0, 1.0);
    (log_min() + unit * log_span()).exp()
}

/// Normalised vertical position in `0..1` for a gain in dB (0 dB at 0.5).
pub fn db_to_unit(db: f64) -> f64 {
    0.5 - db.clamp(-DB_RANGE, DB_RANGE) / (2.0 * DB_RANGE)
}

/// Gain in dB for a normalised vertical position.
pub fn unit_to_db(unit: f64) -> f64 {
    let unit = unit.clamp(0.0, 1.0);
    (0.5 - unit) * 2.0 * DB_RANGE
}

/// Tick frequencies drawn on the graph.
pub const FREQ_TICKS: [f64; 10] = [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0];

/// Tick gains drawn on the graph.
pub const DB_TICKS: [f64; 7] = [-24.0, -12.0, -6.0, 0.0, 6.0, 12.0, 24.0];

/// Human readable frequency label.
pub fn freq_label(freq: f64) -> String {
    if freq >= 1000.0 {
        let k = freq / 1000.0;
        if (k - k.round()).abs() < 0.01 {
            format!("{:.0}k", k.round())
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{freq:.0}")
    }
}

/// Formats a gain for display.
pub fn db_label(db: f64) -> String {
    if db.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{db:+.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_of_the_axis_map_to_the_ends() {
        assert!((freq_to_unit(MIN_FREQ) - 0.0).abs() < 1e-12);
        assert!((freq_to_unit(MAX_FREQ) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mapping_round_trips() {
        for freq in [20.0, 63.0, 250.0, 1000.0, 6300.0, 20000.0] {
            let unit = freq_to_unit(freq);
            let back = unit_to_freq(unit);
            assert!((back - freq).abs() < 1e-6, "{freq} -> {back}");
        }
        for db in [-24.0, -6.0, 0.0, 3.5, 24.0] {
            let unit = db_to_unit(db);
            assert!((unit_to_db(unit) - db).abs() < 1e-9);
        }
    }

    #[test]
    fn zero_db_sits_in_the_middle() {
        assert!((db_to_unit(0.0) - 0.5).abs() < 1e-12);
        assert!((db_to_unit(DB_RANGE) - 0.0).abs() < 1e-12);
        assert!((db_to_unit(-DB_RANGE) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn equal_ratios_have_equal_spacing() {
        // A log axis: 20->200 and 200->2000 must be the same distance.
        let a = freq_to_unit(200.0) - freq_to_unit(20.0);
        let b = freq_to_unit(2000.0) - freq_to_unit(200.0);
        assert!((a - b).abs() < 1e-12);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        assert_eq!(freq_to_unit(1.0), 0.0);
        assert_eq!(freq_to_unit(30000.0), 1.0);
        assert_eq!(db_to_unit(-100.0), 1.0);
        assert_eq!(db_to_unit(100.0), 0.0);
    }

    #[test]
    fn labels_are_human_readable() {
        assert_eq!(freq_label(1000.0), "1k");
        assert_eq!(freq_label(20000.0), "20k");
        assert_eq!(freq_label(250.0), "250");
        assert_eq!(db_label(0.01), "0");
        assert_eq!(db_label(3.0), "+3.0");
        assert_eq!(db_label(-6.0), "-6.0");
    }
}
