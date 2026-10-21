// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut)

//! TetalCut: where a voice is speaking and where it is not, for the
//! automatic edit that takes the pauses out of a talking clip.
//!
//! The sound is read the way the transcriber reads it - 16 kHz mono - and
//! measured in short frames. A frame is quiet when it sits near the
//! recording's own floor: the threshold is set between the quietest and
//! the loudest frames of this clip, so a noisy room and a quiet one are
//! both judged against themselves. A run of quiet frames long enough to be
//! a pause is cut, leaving a little air either side so a word is never
//! clipped. The transcriber's word times are not trusted for this - whisper
//! stretches a word over the pause after it - so the sound alone decides.

use concat_media::{AudioDecoder, AudioOptions, SampleFormat};

/// The rate the sound is measured at: the transcriber's.
pub const RATE: u32 = 16_000;

/// How the pauses are found and cut.
#[derive(Clone, Copy, Debug)]
pub struct Pauses {
    /// A quiet run at least this long, seconds, is a pause.
    pub shortest: f64,
    /// Kept before the voice comes back, seconds.
    pub lead: f64,
    /// Kept after the voice stops, seconds.
    pub tail: f64,
}

impl Default for Pauses {
    fn default() -> Self {
        Pauses {
            // TetalCut: short enough to take out the mini pauses of a take
            // spoken in bits, with only a breath of air left either side.
            shortest: 0.3,
            lead: 0.08,
            tail: 0.12,
        }
    }
}

/// A frame's length, seconds.
const FRAME: f64 = 0.02;
/// A recording whose loud frames are less than this many dB above its
/// quiet ones has no pauses to find: music, wind, a voice-over over a bed.
const LEAST_RANGE_DB: f64 = 10.0;

/// The clip's sound from `start` for `window` seconds of its file, as the
/// pauses are measured in.
pub fn read(path: &str, start: f64, window: f64, stream: Option<u32>) -> Result<Vec<f32>, String> {
    let mut decoder = AudioDecoder::open(
        path,
        &AudioOptions {
            start: Some(start),
            duration: Some(window),
            filters: Vec::new(),
            rate: RATE,
            channels: 1,
            format: SampleFormat::F32,
            stream: stream.map(|index| index as usize),
        },
    )
    .map_err(|error| error.to_string())?;
    decoder.collect_f32().map_err(|error| error.to_string())
}

/// The spans of `samples` (at `rate`) to keep, in seconds from its start:
/// everything but its pauses. The whole when nothing can be told apart.
pub fn keep(samples: &[f32], rate: u32, pauses: Pauses) -> Vec<(f64, f64)> {
    let length = samples.len() as f64 / f64::from(rate.max(1));
    if length <= 0.0 {
        return Vec::new();
    }
    let whole = vec![(0.0, length)];
    let step = ((f64::from(rate) * FRAME) as usize).max(1);
    let levels: Vec<f64> = samples
        .chunks(step)
        .map(|frame| {
            let power = frame.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>()
                / frame.len() as f64;
            10.0 * power.max(1e-12).log10()
        })
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let (floor, peak) = (at(0.1), at(0.95));
    if peak - floor < LEAST_RANGE_DB {
        return whole;
    }
    let threshold = floor + (peak - floor) * 0.3;

    // Quiet runs, as frame ranges.
    let mut quiet: Vec<(usize, usize)> = Vec::new();
    let mut from = None;
    for (index, level) in levels.iter().enumerate() {
        match (from, *level < threshold) {
            (None, true) => from = Some(index),
            (Some(begun), false) => {
                quiet.push((begun, index));
                from = None;
            }
            _ => {}
        }
    }
    if let Some(begun) = from {
        quiet.push((begun, levels.len()));
    }

    // The pauses to cut, in seconds, each with air left either side. At
    // the very start and end only the side the voice is on keeps any.
    let mut cut: Vec<(f64, f64)> = Vec::new();
    for (a, b) in quiet {
        let (start, end) = (a as f64 * FRAME, (b as f64 * FRAME).min(length));
        if end - start < pauses.shortest {
            continue;
        }
        let from = if start <= 0.0 { 0.0 } else { start + pauses.tail };
        let to = if end >= length { length } else { end - pauses.lead };
        if to - from > FRAME {
            cut.push((from, to));
        }
    }
    if cut.is_empty() {
        return whole;
    }
    let mut kept = Vec::new();
    let mut at = 0.0;
    for (from, to) in cut {
        if from - at > FRAME {
            kept.push((at, from));
        }
        at = to;
    }
    if length - at > FRAME {
        kept.push((at, length));
    }
    kept
}

/// Where a moment of the clip lands once only `kept` is left, as seconds
/// from the edited clip's start; `None` for a moment that was cut.
pub fn moved(kept: &[(f64, f64)], at: f64) -> Option<f64> {
    let mut before = 0.0;
    for (from, to) in kept {
        if at >= *from && at <= *to {
            return Some(before + at - from);
        }
        before += to - from;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone for `seconds` and silence (a whisper of noise) for `gap`,
    /// alternating as `parts` says.
    fn take(parts: &[(bool, f64)]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut n = 0u32;
        for (voiced, seconds) in parts {
            for _ in 0..(seconds * f64::from(RATE)) as usize {
                n = n.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = ((n >> 16) as f32 / 32_768.0 - 1.0) * 0.001;
                let tone = if *voiced {
                    (out.len() as f32 * 0.07).sin() * 0.3
                } else {
                    0.0
                };
                out.push(tone + noise);
            }
        }
        out
    }

    #[test]
    fn pauses_are_cut_with_air_left_around_the_voice() {
        let samples = take(&[(false, 1.0), (true, 2.0), (false, 1.0), (true, 1.0), (false, 0.4), (true, 1.0), (false, 0.2), (true, 1.0)]);
        let kept = keep(&samples, RATE, Pauses::default());
        // the leading second goes up to the lead; each pause loses all but
        // the air, a mini one too; one shorter than a pause stays.
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert!((kept[0].0 - 0.92).abs() < 0.05, "{kept:?}");
        assert!((kept[0].1 - 3.12).abs() < 0.05, "{kept:?}");
        assert!((kept[1].0 - 3.92).abs() < 0.05, "{kept:?}");
        assert!((kept[1].1 - 5.12).abs() < 0.05, "{kept:?}");
        assert!((kept[2].0 - 5.32).abs() < 0.05, "{kept:?}");
        assert!((kept[2].1 - 7.6).abs() < 0.05, "{kept:?}");
    }

    #[test]
    fn a_flat_recording_is_kept_whole() {
        let samples = take(&[(true, 3.0)]);
        assert_eq!(keep(&samples, RATE, Pauses::default()), vec![(0.0, 3.0)]);
    }

    #[test]
    fn moments_move_up_with_the_cuts() {
        let kept = [(1.0, 2.0), (3.0, 4.0)];
        assert_eq!(moved(&kept, 1.5), Some(0.5));
        assert_eq!(moved(&kept, 3.5), Some(1.5));
        assert_eq!(moved(&kept, 2.5), None);
    }
}
