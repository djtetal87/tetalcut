// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer and Concat contributors

//! Speed curves: the arithmetic the commands share.
//!
//! A curve is a handful of `(at, speed)` points over the clip, joined by
//! straight lines; the engine's [`SpeedCurve`] turns them into a time map.

use concat_core::SpeedCurve;

use crate::model::SpeedPoint;

/// The engine's curve for these points, or None when they make no curve.
pub fn curve_of(points: &[SpeedPoint]) -> Option<SpeedCurve> {
    let raw: Vec<(f64, f64)> = points
        .iter()
        .map(|point: &SpeedPoint| (point.at, point.speed))
        .collect();
    SpeedCurve::new(&raw)
}

/// Source seconds per timeline second over a whole clip with these points;
/// the constant rate's equivalent for a curve.
pub fn mean_of(points: &[SpeedPoint]) -> f64 {
    curve_of(points).map(|curve| curve.mean()).unwrap_or(1.0)
}

/// TetalCut: the ready-made curves, as CapCut's Speed ▸ Curve, by the names
/// a panel lists. The first, Normal, is no curve: one times speed.
pub const PRESETS: [&str; 9] = [
    "Normal",
    "Montaje",
    "Héroe",
    "Bala",
    "Salto",
    "Flash entrada",
    "Flash salida",
    "Rampa lenta",
    "Rampa rápida",
];

/// A preset's points (see [`PRESETS`]); None for Normal or a row past the end.
pub fn preset(index: usize) -> Option<Vec<SpeedPoint>> {
    let points: &[(f64, f64)] = match index {
        1 => &[(0.0, 1.6), (0.18, 0.6), (0.36, 2.6), (0.56, 0.6), (0.76, 2.2), (1.0, 1.0)],
        2 => &[(0.0, 1.6), (0.32, 1.6), (0.42, 0.25), (0.62, 0.25), (0.72, 1.6), (1.0, 1.6)],
        3 => &[(0.0, 2.2), (0.3, 2.2), (0.42, 0.15), (0.66, 0.15), (0.78, 2.2), (1.0, 2.2)],
        4 => &[(0.0, 1.0), (0.4, 1.0), (0.5, 4.0), (0.6, 1.0), (1.0, 1.0)],
        5 => &[(0.0, 5.0), (0.25, 3.0), (0.5, 1.0), (1.0, 1.0)],
        6 => &[(0.0, 1.0), (0.5, 1.0), (0.75, 3.0), (1.0, 5.0)],
        7 => &[(0.0, 1.0), (0.35, 1.0), (0.65, 0.3), (1.0, 0.3)],
        8 => &[(0.0, 1.0), (0.35, 1.0), (0.65, 3.0), (1.0, 3.0)],
        _ => return None,
    };
    Some(
        points
            .iter()
            .map(|&(at, speed)| SpeedPoint { at, speed })
            .collect(),
    )
}

/// The row in [`PRESETS`] these points are, if any: 0 for no curve.
pub fn preset_of(curve: Option<&[SpeedPoint]>) -> Option<usize> {
    let Some(points) = curve else {
        return Some(0);
    };
    (1..PRESETS.len()).find(|&index| preset(index).as_deref() == Some(points))
}

/// TetalCut: a curve editor's height for a speed, and back: 0.1x at the
/// bottom, 1x in the middle, 10x at the top, evenly in stops.
pub fn height_of(speed: f64) -> f64 {
    (0.5 + speed.max(1e-6).log10() / 2.0).clamp(0.0, 1.0)
}

/// See [`height_of`].
pub fn speed_of(height: f64) -> f64 {
    10f64.powf((height.clamp(0.0, 1.0) - 0.5) * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_make_curves_and_are_recognised() {
        assert!(preset(0).is_none());
        for index in 1..PRESETS.len() {
            let points = preset(index).unwrap();
            assert!(curve_of(&points).is_some(), "{}", PRESETS[index]);
            assert_eq!(preset_of(Some(&points)), Some(index));
        }
        assert_eq!(preset_of(None), Some(0));
        // a hero shot is slower on the whole than its fast ends
        assert!(mean_of(&preset(2).unwrap()) < 1.6);
    }

    #[test]
    fn heights_and_speeds_round_trip() {
        assert!((height_of(1.0) - 0.5).abs() < 1e-9);
        assert!((speed_of(height_of(4.0)) - 4.0).abs() < 1e-9);
        assert!((speed_of(0.0) - 0.1).abs() < 1e-9);
    }
}
