//! Channel copy / mixing matrix used by the `Copy:` command.

/// One addition inside a channel assignment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixTerm {
    pub gain: f32,
    /// Source channel index, or `None` for a constant.
    pub source: Option<usize>,
    /// Constant sample value when `source` is `None`.
    pub constant: f32,
}

/// Replaces the audio of `target` with the weighted sum of `terms`.
#[derive(Clone, Debug, PartialEq)]
pub struct MixAssignment {
    pub target: usize,
    pub terms: Vec<MixTerm>,
}

impl MixAssignment {
    /// Highest channel index referenced by this assignment.
    pub fn max_channel(&self) -> usize {
        self.terms
            .iter()
            .filter_map(|t| t.source)
            .max()
            .unwrap_or(0)
            .max(self.target)
    }
}

/// Applies `assignments` to planar channel data.
///
/// All reads happen against a snapshot of the input, so `Copy: L=R R=L` swaps
/// the channels and self references (`Copy: L=L+0.5*R`) behave as documented.
pub fn apply_mix(channels: &mut [Vec<f32>], assignments: &[MixAssignment]) {
    if assignments.is_empty() || channels.is_empty() {
        return;
    }
    let snapshot: Vec<Vec<f32>> = channels.to_vec();
    let frames = channels[0].len();
    for assignment in assignments {
        if assignment.target >= channels.len() {
            continue;
        }
        let target = &mut channels[assignment.target];
        for (n, out) in target.iter_mut().enumerate().take(frames) {
            let mut acc = 0.0f32;
            for term in &assignment.terms {
                acc += match term.source {
                    Some(idx) => match snapshot.get(idx) {
                        Some(src) => src.get(n).copied().unwrap_or(0.0) * term.gain,
                        None => 0.0,
                    },
                    None => term.constant * term.gain,
                };
            }
            *out = acc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(data: &[&[f32]]) -> Vec<Vec<f32>> {
        data.iter().map(|d| d.to_vec()).collect()
    }

    #[test]
    fn simple_copy_duplicates_a_channel() {
        let mut ch = channels(&[&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0]]);
        apply_mix(
            &mut ch,
            &[MixAssignment {
                target: 1,
                terms: vec![MixTerm {
                    gain: 1.0,
                    source: Some(0),
                    constant: 0.0,
                }],
            }],
        );
        assert_eq!(ch[1], vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn copy_is_simultaneous_so_channels_swap() {
        let mut ch = channels(&[&[1.0, 1.0], &[2.0, 2.0]]);
        apply_mix(
            &mut ch,
            &[
                MixAssignment {
                    target: 0,
                    terms: vec![MixTerm {
                        gain: 1.0,
                        source: Some(1),
                        constant: 0.0,
                    }],
                },
                MixAssignment {
                    target: 1,
                    terms: vec![MixTerm {
                        gain: 1.0,
                        source: Some(0),
                        constant: 0.0,
                    }],
                },
            ],
        );
        assert_eq!(ch[0], vec![2.0, 2.0]);
        assert_eq!(ch[1], vec![1.0, 1.0]);
    }

    #[test]
    fn self_reference_adds_signal() {
        let mut ch = channels(&[&[1.0, 2.0], &[0.5, 0.5]]);
        apply_mix(
            &mut ch,
            &[MixAssignment {
                target: 0,
                terms: vec![
                    MixTerm {
                        gain: 1.0,
                        source: Some(0),
                        constant: 0.0,
                    },
                    MixTerm {
                        gain: 0.5,
                        source: Some(1),
                        constant: 0.0,
                    },
                ],
            }],
        );
        assert_eq!(ch[0], vec![1.25, 2.25]);
    }

    #[test]
    fn constants_replace_a_channel() {
        let mut ch = channels(&[&[1.0, 2.0]]);
        apply_mix(
            &mut ch,
            &[MixAssignment {
                target: 0,
                terms: vec![MixTerm {
                    gain: 1.0,
                    source: None,
                    constant: 0.5,
                }],
            }],
        );
        assert_eq!(ch[0], vec![0.5, 0.5]);
    }

    #[test]
    fn missing_source_channels_are_silent() {
        let mut ch = channels(&[&[1.0, 2.0]]);
        apply_mix(
            &mut ch,
            &[MixAssignment {
                target: 0,
                terms: vec![MixTerm {
                    gain: 1.0,
                    source: Some(5),
                    constant: 0.0,
                }],
            }],
        );
        assert_eq!(ch[0], vec![0.0, 0.0]);
    }
}
