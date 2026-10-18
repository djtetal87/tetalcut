// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: ready-made movements for a clip - how it comes in, how it goes
//! out and how it moves while on screen - written as the clip's keys, so
//! they play, export and can be reworked like any other keys.
//!
//! An entrance takes the first moments of the clip, an exit the last, and a
//! loop runs from start to end. Where two of them move the same property
//! (a pop in on a pulsing title) they are combined: scale and opacity
//! multiply, position and rotation add.

use concat_core::animate::{Key, Track};

use crate::model::{ClipKey, KeyEase, KeyProperty};

/// Entrances, by the names a panel lists; the first is none.
pub const ENTRANCES: [&str; 10] = [
    "Ninguna",
    "Aparecer",
    "Pop",
    "Zoom",
    "Subir",
    "Bajar",
    "Desde la izquierda",
    "Desde la derecha",
    "Rebote",
    "Girar",
];

/// Exits, by the names a panel lists; the first is none.
pub const EXITS: [&str; 10] = [
    "Ninguna",
    "Desvanecer",
    "Encoger",
    "Zoom",
    "Subir",
    "Bajar",
    "Hacia la izquierda",
    "Hacia la derecha",
    "Rebote",
    "Girar",
];

/// Movements while on screen, by the names a panel lists; the first is none.
pub const LOOPS: [&str; 6] = ["Ninguno", "Latido", "Flotar", "Balanceo", "Temblor", "Respirar"];

/// Longest an entrance or exit takes, seconds.
const EDGE_SECONDS: f64 = 0.45;
/// Most of a clip an entrance or an exit may take.
const EDGE_SHARE: f64 = 0.35;
/// Most keys a loop writes per property, however long the clip.
const LOOP_KEYS: usize = 240;

/// Where a clip rests when nothing moves it: what entrances arrive at and
/// exits leave from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rest {
    /// `Clip::scale`.
    pub scale: f64,
    /// `Clip::offset_x`.
    pub offset_x: f64,
    /// `Clip::offset_y`.
    pub offset_y: f64,
    /// `Clip::rotation`, degrees.
    pub rotation: f64,
    /// `Clip::opacity`.
    pub opacity: f64,
}

impl Default for Rest {
    fn default() -> Rest {
        Rest {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            rotation: 0.0,
            opacity: 1.0,
        }
    }
}

impl Rest {
    fn of(&self, property: KeyProperty) -> f64 {
        match property {
            KeyProperty::Scale => self.scale,
            KeyProperty::OffsetX => self.offset_x,
            KeyProperty::OffsetY => self.offset_y,
            KeyProperty::Rotation => self.rotation,
            KeyProperty::Opacity => self.opacity,
            KeyProperty::Volume => 1.0,
        }
    }
}

/// Whether a property's movements multiply (scale, opacity) or add.
fn multiplies(property: KeyProperty) -> bool {
    matches!(property, KeyProperty::Scale | KeyProperty::Opacity)
}

/// The neutral value of a movement on a property: what changes nothing.
fn neutral(property: KeyProperty) -> f64 {
    if multiplies(property) { 1.0 } else { 0.0 }
}

/// One movement on one property, relative to rest: factors for scale and
/// opacity, amounts for position and rotation. `(seconds, value, ease)`.
type Part = (KeyProperty, Vec<(f64, f64, KeyEase)>);

/// The properties a movement may touch, in the order keys are written.
const MOVED: [KeyProperty; 5] = [
    KeyProperty::Scale,
    KeyProperty::OffsetX,
    KeyProperty::OffsetY,
    KeyProperty::Rotation,
    KeyProperty::Opacity,
];

/// How long an entrance or exit lasts on a clip of `duration` seconds.
fn edge(duration: f64) -> f64 {
    EDGE_SECONDS.min(duration * EDGE_SHARE).max(0.0)
}

/// An entrance's parts over its first `d` seconds, arriving at neutral.
fn entrance_parts(index: usize, d: f64) -> Vec<Part> {
    use KeyEase as E;
    use KeyProperty as P;
    let fade = |share: f64| (P::Opacity, vec![(0.0, 0.0, E::LINEAR), (d * share, 1.0, E::OUT)]);
    match index {
        1 => vec![fade(1.0)],
        2 => vec![
            (P::Scale, vec![(0.0, 0.6, E::LINEAR), (d * 0.65, 1.08, E::OUT), (d, 1.0, E::IN_OUT)]),
            fade(0.5),
        ],
        3 => vec![(P::Scale, vec![(0.0, 1.6, E::LINEAR), (d, 1.0, E::OUT)]), fade(0.7)],
        4 => vec![(P::OffsetY, vec![(0.0, 0.12, E::LINEAR), (d, 0.0, E::OUT)]), fade(0.6)],
        5 => vec![(P::OffsetY, vec![(0.0, -0.12, E::LINEAR), (d, 0.0, E::OUT)]), fade(0.6)],
        6 => vec![(P::OffsetX, vec![(0.0, -0.3, E::LINEAR), (d, 0.0, E::OUT)]), fade(0.6)],
        7 => vec![(P::OffsetX, vec![(0.0, 0.3, E::LINEAR), (d, 0.0, E::OUT)]), fade(0.6)],
        8 => vec![
            (
                P::Scale,
                vec![
                    (0.0, 0.0, E::LINEAR),
                    (d * 0.5, 1.18, E::OUT),
                    (d * 0.75, 0.92, E::IN_OUT),
                    (d, 1.0, E::IN_OUT),
                ],
            ),
            fade(0.3),
        ],
        9 => vec![
            (P::Rotation, vec![(0.0, -180.0, E::LINEAR), (d, 0.0, E::OUT)]),
            (P::Scale, vec![(0.0, 0.3, E::LINEAR), (d, 1.0, E::OUT)]),
            fade(0.5),
        ],
        _ => Vec::new(),
    }
}

/// An exit's parts over its last `d` seconds of a `duration`-second clip,
/// leaving from neutral: the matching entrance played backwards.
fn exit_parts(index: usize, d: f64, duration: f64) -> Vec<Part> {
    let mut parts = entrance_parts(index, d);
    let start = duration - d;
    for (_, keys) in &mut parts {
        // the entrance's last value leads, its first ends; eases mirror
        let reversed: Vec<(f64, f64, KeyEase)> = keys
            .iter()
            .rev()
            .map(|(at, value, _)| (start + (d - at), *value, KeyEase::IN))
            .collect();
        *keys = reversed;
        if let Some(first) = keys.first_mut() {
            first.2 = KeyEase::LINEAR;
        }
    }
    parts
}

/// A loop's parts over the whole `duration`.
fn loop_parts(index: usize, duration: f64) -> Vec<Part> {
    use KeyEase as E;
    use KeyProperty as P;
    // (property, period, values through one period, ease)
    let (property, period, cycle, ease): (KeyProperty, f64, &[f64], KeyEase) = match index {
        1 => (P::Scale, 0.8, &[1.0, 1.08, 1.0], E::IN_OUT),
        2 => (P::OffsetY, 2.0, &[0.0, -0.015, 0.0], E::IN_OUT),
        3 => (P::Rotation, 1.6, &[-4.0, 4.0, -4.0], E::IN_OUT),
        4 => (P::OffsetX, 0.24, &[0.0, 0.006, -0.006, 0.004, -0.004, 0.0], E::LINEAR),
        5 => (P::Opacity, 1.4, &[1.0, 0.72, 1.0], E::IN_OUT),
        _ => return Vec::new(),
    };
    let steps = cycle.len().saturating_sub(1).max(1);
    let step = period / steps as f64;
    let mut keys = Vec::new();
    let mut at = 0.0;
    let mut index = 0usize;
    while at <= duration + 1e-9 && keys.len() < LOOP_KEYS {
        let value = cycle.get(index % steps).copied().unwrap_or(neutral(property));
        keys.push((at, value, if keys.is_empty() { E::LINEAR } else { ease }));
        at += step;
        index += 1;
    }
    vec![(property, keys)]
}

/// The keys of an entrance, an exit and a loop (rows in [`ENTRANCES`],
/// [`EXITS`] and [`LOOPS`]; 0 is none) on a clip of `duration` seconds
/// resting at `rest`. Keys are at fractions of the clip, as clips keep them.
pub fn motion_keys(entrance: usize, exit: usize, looping: usize, duration: f64, rest: &Rest) -> Vec<ClipKey> {
    if !duration.is_finite() || duration <= 0.0 {
        return Vec::new();
    }
    let d = edge(duration);
    let mut parts = entrance_parts(entrance, d);
    parts.extend(exit_parts(exit, d, duration));
    parts.extend(loop_parts(looping, duration));
    let mut out = Vec::new();
    for property in MOVED {
        let mine: Vec<&Vec<(f64, f64, KeyEase)>> = parts
            .iter()
            .filter(|(p, _)| *p == property)
            .map(|(_, keys)| keys)
            .collect();
        let base = rest.of(property);
        let apply = |relative: f64| {
            if multiplies(property) { base * relative } else { base + relative }
        };
        match mine.as_slice() {
            [] => {}
            [only] => {
                for (at, value, ease) in only.iter() {
                    out.push(ClipKey {
                        property,
                        at: (at / duration).clamp(0.0, 1.0),
                        value: apply(*value),
                        ease: *ease,
                    });
                }
            }
            many => {
                // combined: sampled at every part's keys and between them
                let tracks: Vec<Track> = many
                    .iter()
                    .map(|keys| {
                        Track::new(
                            keys.iter()
                                .map(|(at, value, ease)| Key {
                                    at: at / duration,
                                    value: *value,
                                    ease: (*ease).into(),
                                })
                                .collect(),
                        )
                    })
                    .collect();
                let mut times: Vec<f64> = many
                    .iter()
                    .flat_map(|keys| keys.iter().map(|(at, _, _)| (at / duration).clamp(0.0, 1.0)))
                    .collect();
                times.sort_by(f64::total_cmp);
                times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
                let mut samples = Vec::new();
                for pair in times.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    for k in 0..4 {
                        samples.push(a + (b - a) * k as f64 / 4.0);
                    }
                }
                if let Some(last) = times.last() {
                    samples.push(*last);
                }
                samples.truncate(LOOP_KEYS * 2);
                for at in samples {
                    let combined = tracks.iter().fold(neutral(property), |acc, track| {
                        let value = track.value_at(at, neutral(property));
                        if multiplies(property) { acc * value } else { acc + value }
                    });
                    out.push(ClipKey {
                        property,
                        at,
                        value: apply(combined),
                        ease: KeyEase::LINEAR,
                    });
                }
            }
        }
    }
    out.retain(|key| key.at.is_finite() && key.value.is_finite());
    out
}

/// A loop's keys alone (see [`motion_keys`]).
pub fn loop_keys(looping: usize, duration: f64, rest: &Rest) -> Vec<ClipKey> {
    motion_keys(0, 0, looping, duration, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(keys: &[ClipKey], property: KeyProperty) -> Vec<(f64, f64)> {
        keys.iter()
            .filter(|key| key.property == property)
            .map(|key| (key.at, key.value))
            .collect()
    }

    #[test]
    fn an_entrance_arrives_at_rest_and_an_exit_leaves_from_it() {
        let rest = Rest {
            offset_y: 0.35,
            ..Rest::default()
        };
        let keys = motion_keys(4, 4, 0, 3.0, &rest);
        let y = on(&keys, KeyProperty::OffsetY);
        // in from below, out upwards, resting at the caption's place between
        let first = y.first().unwrap();
        let last = y.last().unwrap();
        assert!((first.0 - 0.0).abs() < 1e-9 && (first.1 - 0.47).abs() < 1e-9);
        assert!((last.0 - 1.0).abs() < 1e-9 && (last.1 - 0.47).abs() < 1e-9);
        assert!(y.iter().any(|(at, value)| *at > 0.1 && *at < 0.9 && (value - 0.35).abs() < 1e-9));
        let opacity = on(&keys, KeyProperty::Opacity);
        assert_eq!(opacity.first().unwrap().1, 0.0);
        assert_eq!(opacity.last().unwrap().1, 0.0);
    }

    #[test]
    fn a_loop_runs_the_whole_clip_and_combines_with_a_pop() {
        let keys = loop_keys(1, 4.0, &Rest::default());
        let scale = on(&keys, KeyProperty::Scale);
        assert!(scale.len() >= 10);
        assert!(scale.iter().all(|(at, v)| (0.0..=1.0).contains(at) && (0.99..=1.09).contains(v)));
        // pop in on a pulse: one scale track, starting small, keys in order
        let both = motion_keys(2, 0, 1, 4.0, &Rest::default());
        let scale = on(&both, KeyProperty::Scale);
        assert!(scale.first().unwrap().1 < 0.7);
        assert!(scale.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(scale.iter().any(|(_, v)| *v > 1.05));
    }

    #[test]
    fn nothing_chosen_or_no_time_writes_no_keys() {
        assert!(motion_keys(0, 0, 0, 5.0, &Rest::default()).is_empty());
        assert!(motion_keys(3, 3, 3, 0.0, &Rest::default()).is_empty());
        assert!(motion_keys(3, 3, 3, f64::NAN, &Rest::default()).is_empty());
        // a long clip's loop stays bounded
        assert!(loop_keys(4, 3600.0, &Rest::default()).len() <= LOOP_KEYS);
    }
}
