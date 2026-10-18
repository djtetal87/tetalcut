// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: HDR and wide-gamut video brought into BT.709 by a 3D LUT.
//!
//! FFmpeg 8.1's swscale converts primaries and transfers itself, and on an
//! iPhone's HLG (Dolby Vision 8.4) footage it gets it wrong: a magenta
//! cast and posterised highlights, in every thumbnail and every eight-bit
//! frame. So the decoder's `scale` only undoes the YCbCr matrix and the
//! range, and this LUT does the rest - the transfer undone (HLG with its
//! system gamma, or PQ), Rec. 2020 to Rec. 709, the highlights rolled off
//! into SDR, and the BT.709 curve put back - with libavfilter's `lut3d`,
//! which every build has.

use std::path::PathBuf;
use std::sync::OnceLock;

use concat_core::frame::Signal;

/// Points a side.
const SIZE: usize = 33;
/// SDR reference white, nits (ITU-R BT.2408).
const WHITE: f64 = 203.0;
/// The peak HDR footage is assumed mastered to, nits.
const PEAK: f64 = 1000.0;
/// Where the highlight roll-off starts, in SDR white.
const KNEE: f64 = 0.75;

fn hlg_to_scene(e: f64) -> f64 {
    const A: f64 = 0.178_832_77;
    const B: f64 = 0.284_668_92;
    const C: f64 = 0.559_910_73;
    if e <= 0.5 { e * e / 3.0 } else { (((e - C) / A).exp() + B) / 12.0 }
}

fn pq_to_nits(e: f64) -> f64 {
    const M1: f64 = 0.159_301_757_812_5;
    const M2: f64 = 78.843_75;
    const C1: f64 = 0.835_937_5;
    const C2: f64 = 18.851_562_5;
    const C3: f64 = 18.687_5;
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1) * 10_000.0
}

fn bt709_to_linear(e: f64) -> f64 {
    if e < 0.081 { e / 4.5 } else { ((e + 0.099) / 1.099).powf(1.0 / 0.45) }
}

fn linear_to_bt709(l: f64) -> f64 {
    let l = l.clamp(0.0, 1.0);
    if l < 0.018 { 4.5 * l } else { 1.099 * l.powf(0.45) - 0.099 }
}

/// One colour of the LUT: `rgb` as the source encodes it (0..1, Rec. 2020)
/// to BT.709 SDR, also 0..1.
pub fn to_sdr(signal: Signal, rgb: [f64; 3]) -> [f64; 3] {
    // light, 1.0 = SDR white
    let (light, peak) = match signal {
        Signal::Hlg => {
            let scene = rgb.map(hlg_to_scene);
            let y = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
            // the system gamma of a 1000-nit display (1.2)
            let gain = y.max(1e-6).powf(0.2) * PEAK / WHITE;
            (scene.map(|v| v * gain), PEAK / WHITE)
        }
        Signal::Pq => (rgb.map(|e| pq_to_nits(e) / WHITE), PEAK / WHITE),
        Signal::SdrWide | Signal::Sdr => (rgb.map(bt709_to_linear), 1.0),
    };
    // Rec. 2020 to Rec. 709, in light
    let [r, g, b] = light;
    let mapped = if signal == Signal::Sdr {
        [r, g, b]
    } else {
        [
            1.6605 * r - 0.5876 * g - 0.0728 * b,
            -0.1246 * r + 1.1329 * g - 0.0083 * b,
            -0.0182 * r - 0.1006 * g + 1.1187 * b,
        ]
        .map(|v: f64| v.max(0.0))
    };
    // the highlights rolled off above a knee, on luminance: untouched
    // below it, the rest up to the peak eased into what is left of SDR
    // (extended Reinhard, its slope 1 at the knee)
    let l = 0.2126 * mapped[0] + 0.7152 * mapped[1] + 0.0722 * mapped[2];
    let ratio = if peak > 1.0 && l > KNEE {
        let top = (peak - KNEE) / (1.0 - KNEE);
        let s = (l - KNEE) / (1.0 - KNEE);
        (KNEE + (1.0 - KNEE) * s * (1.0 + s / (top * top)) / (1.0 + s)) / l
    } else {
        1.0
    };
    mapped.map(|v| linear_to_bt709(v * ratio))
}

/// The LUT as a `.cube` file's text.
pub fn cube(signal: Signal) -> String {
    let mut out = String::with_capacity(SIZE * SIZE * SIZE * 28);
    out.push_str(&format!("LUT_3D_SIZE {SIZE}\n"));
    let step = 1.0 / (SIZE - 1) as f64;
    // red fastest, as the format has it
    for b in 0..SIZE {
        for g in 0..SIZE {
            for r in 0..SIZE {
                let [x, y, z] = to_sdr(signal, [r as f64 * step, g as f64 * step, b as f64 * step]);
                out.push_str(&format!("{x:.6} {y:.6} {z:.6}\n"));
            }
        }
    }
    out
}

/// The LUT's file for `signal`, written once per run into the temporary
/// folder; None when it cannot be written (the caller falls back).
pub fn cube_file(signal: Signal) -> Option<PathBuf> {
    static HLG: OnceLock<Option<PathBuf>> = OnceLock::new();
    static PQ: OnceLock<Option<PathBuf>> = OnceLock::new();
    static WIDE: OnceLock<Option<PathBuf>> = OnceLock::new();
    let (cell, name) = match signal {
        Signal::Hlg => (&HLG, "hlg"),
        Signal::Pq => (&PQ, "pq"),
        Signal::SdrWide => (&WIDE, "wide"),
        Signal::Sdr => return None,
    };
    cell.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("tetalcut-{name}-to-sdr-v1.cube"));
        let part = path.with_extension("part");
        std::fs::write(&part, cube(signal)).ok()?;
        std::fs::rename(&part, &path).ok()?;
        Some(path)
    })
    .clone()
}

/// A path as a filter option's value: quoted, with what the graph parser
/// would read as syntax escaped.
pub fn filter_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let escaped = text.replace('\'', "'\\''").replace(':', "\\:");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black_land_where_sdr_puts_them() {
        // HLG's reference white (75 %) is about SDR white, black is black
        let white = to_sdr(Signal::Hlg, [0.75, 0.75, 0.75]);
        assert!(white.iter().all(|v| (0.85..=1.0).contains(v)), "{white:?}");
        assert_eq!(to_sdr(Signal::Hlg, [0.0; 3]), [0.0; 3]);
        // a grey stays grey: no cast
        let grey = to_sdr(Signal::Hlg, [0.5, 0.5, 0.5]);
        assert!((grey[0] - grey[2]).abs() < 1e-3 && (grey[0] - grey[1]).abs() < 1e-3);
        // PQ's 203 nits is SDR white, give or take the roll-off
        let e = 0.58; // about 203 nits
        let pq = to_sdr(Signal::Pq, [e, e, e]);
        assert!(pq.iter().all(|v| (0.8..=1.0).contains(v)), "{pq:?}");
        // the brightest HDR stays in range
        assert!(to_sdr(Signal::Pq, [1.0; 3]).iter().all(|v| *v <= 1.0));
    }

    #[test]
    fn the_cube_has_every_point_and_paths_are_escaped() {
        let text = cube(Signal::Hlg);
        assert_eq!(text.lines().count(), 1 + SIZE * SIZE * SIZE);
        assert_eq!(
            filter_path(std::path::Path::new("C:\\a b\\x.cube")),
            "'C\\:/a b/x.cube'"
        );
    }
}
