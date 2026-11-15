//! Audio device identity and matching helpers that do not depend on Windows
//! APIs, so the behaviour can be unit tested everywhere.

/// Everything the configuration language needs to know about a device.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    /// Friendly device name, e.g. `Speakers (Realtek High Definition Audio)`.
    pub name: String,
    /// Connection / endpoint name, e.g. `Speakers`.
    pub connection: String,
    /// Endpoint GUID without braces, lower case.
    pub guid: String,
    /// `render` or `capture`.
    pub flow: Flow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flow {
    #[default]
    Render,
    Capture,
}

impl DeviceInfo {
    pub fn new(
        name: impl Into<String>,
        connection: impl Into<String>,
        guid: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            connection: connection.into(),
            guid: guid.into(),
            flow: Flow::Render,
        }
    }

    /// The string `Device:` patterns are matched against: name, connection and
    /// GUID concatenated with spaces.
    pub fn match_string(&self) -> String {
        format!("{} {} {}", self.name, self.connection, self.guid)
    }
}

/// True when every word of `pattern` occurs in the device match string.
///
/// Matching is case-insensitive, mirroring the reference implementation which
/// lets users paste device names without worrying about capitalisation.
pub fn matches_device(pattern: &str, device: &DeviceInfo) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    if pattern.eq_ignore_ascii_case("all") {
        return true;
    }
    let haystack = device.match_string().to_lowercase();
    pattern
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// Matches a `Device:` parameter, which may contain several `;` separated
/// alternatives of which at least one must match.
pub fn matches_device_list(parameter: &str, device: &DeviceInfo) -> bool {
    parameter
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|pattern| matches_device(pattern, device))
}

/// Channel layout derived from the channel count, following the table in the
/// configuration reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Surround3,
    Quad,
    Surround4,
    FiveOne,
    SevenOne,
    Custom(usize),
}

impl ChannelLayout {
    pub fn from_count(channels: usize) -> Self {
        match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            3 => ChannelLayout::Surround3,
            4 => ChannelLayout::Surround4,
            5 => ChannelLayout::FiveOne,
            6 => ChannelLayout::FiveOne,
            7 => ChannelLayout::SevenOne,
            8 => ChannelLayout::SevenOne,
            n => ChannelLayout::Custom(n),
        }
    }

    /// Resolves a channel identifier to a zero based channel index.
    ///
    /// Numeric identifiers are 1-based, matching the configuration format.
    pub fn resolve(&self, token: &str, channels: usize) -> Option<usize> {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        if let Ok(n) = token.parse::<usize>() {
            return if n >= 1 && n <= channels {
                Some(n - 1)
            } else {
                None
            };
        }
        if token.eq_ignore_ascii_case("all") {
            return None;
        }
        let up = token.to_uppercase();
        let idx = match (channels, up.as_str()) {
            (1, "C") => 0,
            (_, "L") => 0,
            (_, "R") => 1,
            (3, "C") => 2,
            (4, "RL") => 2,
            (4, "RR") => 3,
            (4, "RC") => 2,
            (5.., "C") => 2,
            (5.., "LFE") => 3,
            // 5.1 and 7.1: side channels are 5/6 for six channels and 7/8 for
            // eight channels; RL/RR alias the side pair for six channels.
            (6, "SL") | (6, "RL") => 4,
            (6, "SR") | (6, "RR") => 5,
            (8, "RL") => 4,
            (8, "RR") => 5,
            (8, "RC") => 6,
            (8, "SL") => 6,
            (8, "SR") => 7,
            _ => return None,
        };
        if idx < channels {
            Some(idx)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_pattern_requires_all_words() {
        let device = DeviceInfo::new(
            "Speakers (Realtek(R) Audio)",
            "Speakers",
            "a1b2c3d4-0000-0000-0000-000000000000",
        );
        assert!(matches_device("Realtek Speakers", &device));
        assert!(!matches_device("Realtek Headphones", &device));
        assert!(matches_device("a1b2c3d4", &device));
        assert!(matches_device("all", &device));
        assert!(matches_device("", &device));
    }

    #[test]
    fn device_matching_is_case_insensitive() {
        let device = DeviceInfo::new("Headphones", "Headphones", "guid");
        assert!(matches_device("headphones", &device));
    }

    #[test]
    fn alternatives_are_separated_by_semicolons() {
        let device = DeviceInfo::new("Speakers", "Speakers", "guid");
        assert!(matches_device_list("Microphone; Speakers", &device));
        assert!(!matches_device_list("Microphone; Line In", &device));
    }

    #[test]
    fn numeric_channel_identifiers_are_one_based() {
        let layout = ChannelLayout::from_count(2);
        assert_eq!(layout.resolve("1", 2), Some(0));
        assert_eq!(layout.resolve("2", 2), Some(1));
        assert_eq!(layout.resolve("3", 2), None);
    }

    #[test]
    fn stereo_channel_identifiers() {
        let layout = ChannelLayout::from_count(2);
        assert_eq!(layout.resolve("L", 2), Some(0));
        assert_eq!(layout.resolve("R", 2), Some(1));
        assert_eq!(layout.resolve("C", 2), None);
    }

    #[test]
    fn five_one_channel_identifiers() {
        let layout = ChannelLayout::from_count(6);
        assert_eq!(layout.resolve("L", 6), Some(0));
        assert_eq!(layout.resolve("R", 6), Some(1));
        assert_eq!(layout.resolve("C", 6), Some(2));
        assert_eq!(layout.resolve("LFE", 6), Some(3));
        assert_eq!(layout.resolve("SL", 6), Some(4));
        assert_eq!(layout.resolve("SR", 6), Some(5));
        assert_eq!(layout.resolve("RL", 6), Some(4));
    }

    #[test]
    fn seven_one_channel_identifiers() {
        let layout = ChannelLayout::from_count(8);
        assert_eq!(layout.resolve("LFE", 8), Some(3));
        assert_eq!(layout.resolve("RL", 8), Some(4));
        assert_eq!(layout.resolve("RC", 8), Some(6));
        assert_eq!(layout.resolve("SL", 8), Some(6));
        assert_eq!(layout.resolve("SR", 8), Some(7));
    }

    #[test]
    fn mono_center_channel() {
        let layout = ChannelLayout::from_count(1);
        assert_eq!(layout.resolve("C", 1), Some(0));
        assert_eq!(layout.resolve("1", 1), Some(0));
    }
}
