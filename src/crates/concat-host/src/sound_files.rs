// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: the sound bank's file work - a video's sound written out as a
//! sound file, and a compilation of effects (one file, one sound after
//! another with a pause between) cut at its silences into one file each.

use std::path::{Path, PathBuf};

use concat_media::audio::{AudioClip, mix_to_file};
use concat_media::peaks::Peaks;

use crate::media::describe;

/// Buckets a second the silences are looked for at.
const BUCKETS: u32 = 100;
/// Quieter than this (about -36 dBFS) is silence.
const QUIET: f32 = 0.016;
/// A pause at least this long separates two sounds.
const GAP: f64 = 0.25;
/// Shorter than this is a click between sounds, not a sound.
const SHORTEST: f64 = 0.06;
/// Kept either side of a sound, so its attack and tail are not clipped.
const PAD: f64 = 0.03;
/// The most pieces one compilation is cut into.
const MOST: usize = 300;

/// Where the sounds are in `peaks`: (start, end) seconds, in order.
pub fn sounds_in(peaks: &Peaks) -> Vec<(f64, f64)> {
    let per = f64::from(peaks.buckets_per_second.max(1.0));
    let count = peaks.min.len().min(peaks.max.len());
    let loud = |i: usize| peaks.max[i].max(-peaks.min[i]) > QUIET;
    let total = count as f64 / per;
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_loud = 0usize;
    for i in 0..count {
        if loud(i) {
            if start.is_none() {
                start = Some(i);
            }
            last_loud = i;
        } else if let Some(s) = start
            && (i - last_loud) as f64 / per >= GAP
        {
            out.push((s as f64 / per, (last_loud + 1) as f64 / per));
            start = None;
        }
    }
    if let Some(s) = start {
        out.push((s as f64 / per, (last_loud + 1) as f64 / per));
    }
    out.into_iter()
        .filter(|(a, b)| b - a >= SHORTEST)
        .map(|(a, b)| ((a - PAD).max(0.0), (b + PAD).min(total)))
        .take(MOST)
        .collect()
}

fn piece(source: &Path, from: f64, seconds: f64) -> AudioClip {
    AudioClip {
        path: source.to_path_buf(),
        stream: None,
        start: 0.0,
        duration: seconds,
        source_start: from,
        speed: 1.0,
        preserve_pitch: false,
        volume: 1.0,
        volume_curve: Default::default(),
        fade_in: 0.0,
        fade_out: 0.0,
        filter_chain: String::new(),
    }
}

/// `source`'s sound (a video's, say) written to `destination` (AAC).
pub fn extract(source: &Path, destination: &Path) -> Result<(), String> {
    let info = concat_media::probe(source).map_err(describe)?;
    let seconds = info
        .duration
        .map(|d| d.as_f64())
        .filter(|s| *s > 0.0)
        .ok_or_else(|| format!("{} no tiene duración", source.display()))?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    mix_to_file(&[piece(source, 0.0, seconds)], seconds, destination).map_err(describe)
}

/// A compilation cut at its silences into `folder`, as "<name> 01.m4a",
/// "<name> 02.m4a", …; the files written.
pub fn split(source: &Path, folder: &Path, name: &str) -> Result<Vec<PathBuf>, String> {
    let peaks = concat_media::peaks::extract(source, BUCKETS, None).map_err(describe)?;
    let parts = sounds_in(&peaks);
    if parts.is_empty() {
        return Err("No se encontraron sonidos en el archivo".into());
    }
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    for (index, (from, to)) in parts.iter().enumerate() {
        let path = folder.join(format!("{name} {:02}.m4a", index + 1));
        mix_to_file(&[piece(source, *from, to - from)], to - from, &path).map_err(describe)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peaks(levels: &[f32]) -> Peaks {
        Peaks {
            min: levels.iter().map(|l| -l).collect(),
            max: levels.to_vec(),
            buckets_per_second: 100.0,
        }
    }

    #[test]
    fn sounds_are_found_between_pauses() {
        // 0.2 s sound, 0.5 s silence, 0.3 s sound with a 0.1 s dip in it
        let mut levels = vec![0.5; 20];
        levels.extend(vec![0.0; 50]);
        levels.extend(vec![0.4; 10]);
        levels.extend(vec![0.0; 10]);
        levels.extend(vec![0.4; 10]);
        levels.extend(vec![0.0; 40]);
        let found = sounds_in(&peaks(&levels));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!((found[0].0 - 0.0).abs() < 1e-9 && (found[0].1 - 0.23).abs() < 1e-6);
        assert!((found[1].0 - 0.67).abs() < 1e-6 && (found[1].1 - 1.03).abs() < 1e-6);
        // a lone click is not a sound; silence finds nothing
        let mut click = vec![0.0; 30];
        click[10] = 0.9;
        assert!(sounds_in(&peaks(&click)).is_empty());
        assert!(sounds_in(&peaks(&[0.0; 100])).is_empty());
    }
}
