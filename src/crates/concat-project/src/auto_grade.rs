// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: automatic colour and light, as CapCut's Adjust ▸ Auto.
//!
//! A frame of the clip is measured - its levels and its average colour -
//! and the Adjust panel's knobs (`concat.adjust`) are set to bring it to a
//! well exposed, neutral picture: exposure and contrast, highlights and
//! shadows, whites and blacks for the light; temperature, tint and a
//! touch of saturation for the colour. Gentle on purpose: a picture that
//! is already right comes back nearly untouched.

/// Knob settings, by the adjust package's keys.
pub type Knobs = Vec<(&'static str, f64)>;

/// What a frame's pixels add up to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// Mean display luma, 0..1.
    pub mean: f64,
    /// The 2nd percentile of display luma.
    pub low: f64,
    /// The 98th percentile of display luma.
    pub high: f64,
    /// Share of pixels at the very top (clipped).
    pub clipped: f64,
    /// Average colour of the midtones, in light (linear), not normalised.
    pub average: [f64; 3],
    /// Mean display chroma (max - min of the channels).
    pub chroma: f64,
}

fn to_linear(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Measures RGBA8 pixels (every few, for speed). None when there are none.
pub fn measure(rgba: &[u8]) -> Option<Levels> {
    let mut histogram = [0u64; 256];
    let (mut sum, mut count, mut chroma) = (0.0f64, 0u64, 0.0f64);
    let mut average = [0.0f64; 3];
    let mut mids = 0u64;
    for pixel in rgba.chunks_exact(4).step_by(3) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        let (rf, gf, bf) = (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
        let luma = 0.2126 * rf + 0.7152 * gf + 0.0722 * bf;
        let bin = (luma * 255.0).round().clamp(0.0, 255.0) as usize;
        if let Some(slot) = histogram.get_mut(bin) {
            *slot += 1;
        }
        sum += luma;
        count += 1;
        let top = rf.max(gf).max(bf);
        let bottom = rf.min(gf).min(bf);
        chroma += top - bottom;
        // the grey world: midtones that are not strongly coloured
        if (0.12..=0.92).contains(&luma) && top - bottom < 0.45 {
            average[0] += to_linear(rf);
            average[1] += to_linear(gf);
            average[2] += to_linear(bf);
            mids += 1;
        }
    }
    if count == 0 {
        return None;
    }
    let percentile = |share: f64| {
        let target = (count as f64 * share).ceil() as u64;
        let mut seen = 0u64;
        for (bin, n) in histogram.iter().enumerate() {
            seen += n;
            if seen >= target.max(1) {
                return bin as f64 / 255.0;
            }
        }
        1.0
    };
    let clipped = histogram.get(250..).map_or(0, |top| top.iter().sum::<u64>()) as f64 / count as f64;
    if mids > 0 {
        for channel in &mut average {
            *channel /= mids as f64;
        }
    } else {
        average = [1.0; 3];
    }
    Some(Levels {
        mean: sum / count as f64,
        low: percentile(0.02),
        high: percentile(0.98),
        clipped,
        average,
        chroma: chroma / count as f64,
    })
}

/// Rounds a knob to a whole step, as the panel shows it.
fn knob(value: f64, limit: f64) -> f64 {
    (value.clamp(-limit, limit) * 10.0).round() / 10.0
}

/// The light knobs for these levels.
pub fn auto_light(levels: &Levels) -> Knobs {
    let mean = levels.mean.clamp(0.02, 0.98);
    // exposure towards a mean of about 0.46, in stops of light, gently
    let exposure = (2.2 * (0.46 / mean).log2() * 0.6).clamp(-1.5, 1.5);
    let gain = 2f64.powf(exposure / 2.2);
    let (low, high) = ((levels.low * gain).min(1.0), (levels.high * gain).min(1.0));
    let range = (high - low).max(0.0);
    let contrast = if range < 0.75 { (0.75 - range) * 70.0 } else { 0.0 };
    let highlights = if levels.clipped > 0.01 || high > 0.97 {
        -(15.0 + levels.clipped * 300.0)
    } else {
        0.0
    };
    let shadows = if low < 0.03 && mean < 0.4 { 18.0 } else if mean < 0.3 { 12.0 } else { 0.0 };
    let blacks = if low > 0.07 { -(low - 0.04) * 250.0 } else { 0.0 };
    let whites = if high < 0.88 { (0.88 - high) * 120.0 } else { 0.0 };
    vec![
        ("exposure", knob(exposure, 1.5)),
        ("contrast", knob(contrast, 30.0)),
        ("highlights", knob(highlights, 45.0)),
        ("shadows", knob(shadows, 30.0)),
        ("whites", knob(whites, 30.0)),
        ("blacks", knob(blacks, 30.0)),
    ]
}

/// The colour knobs for these levels: the white balance that greys the
/// midtones' average (most of the way), and saturation for a dull picture.
pub fn auto_color(levels: &Levels) -> Knobs {
    let [r, g, b] = levels.average;
    let (mut best, mut best_cost) = ((6500.0, 0.0), f64::MAX);
    let mut kelvin = 2500.0;
    while kelvin <= 11000.0 {
        let mut tint = -60.0;
        while tint <= 60.0 {
            let [r2, g2, b2] = white_balance([r, g, b], kelvin, tint);
            let mean = (r2 + g2 + b2) / 3.0;
            if mean > 0.0 {
                let cost = ((r2 - mean).powi(2) + (g2 - mean).powi(2) + (b2 - mean).powi(2)) / (mean * mean)
                    // a little pull towards as shot, so noise does not swing it
                    + ((kelvin - 6500.0) / 40000.0).powi(2)
                    + (tint / 600.0).powi(2);
                if cost < best_cost {
                    best_cost = cost;
                    best = (kelvin, tint);
                }
            }
            tint += 2.0;
        }
        kelvin += 50.0;
    }
    // most of the way: a sunset should stay a little warm
    let temperature = 6500.0 + (best.0 - 6500.0) * 0.8;
    let tint = best.1 * 0.8;
    let saturation = if levels.chroma < 0.12 { (0.12 - levels.chroma) * 150.0 } else { 0.0 };
    vec![
        ("temperature", (temperature.clamp(2000.0, 11000.0) / 10.0).round() * 10.0),
        ("tint", knob(tint, 60.0)),
        ("saturation", knob(saturation.min(15.0), 15.0)),
    ]
}

// ── the adjust shader's white balance, in Rust (see concat-effects'
// shader.rs `white_balance`); matrices by columns, as WGSL writes them ──

type Mat = [[f64; 3]; 3];

const BRADFORD: Mat = [[0.8951, -0.7502, 0.0389], [0.2664, 1.7135, -0.0685], [-0.1614, 0.0367, 1.0296]];
const CONES: Mat = [
    [0.4227252927, 0.0556997770, 0.0213826437],
    [0.4913453244, 0.9615340509, 0.0876418678],
    [0.0273579445, 0.0231838105, 0.9805081326],
];
const FROM_CONES: Mat = [
    [2.5380445168, -0.1460040576, -0.0422985104],
    [-1.2932769979, 1.1166483055, -0.0716072203],
    [-0.0402368843, -0.0223290262, 1.0227526883],
];

fn mul(m: &Mat, v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (column, scale) in m.iter().zip(v) {
        for (o, c) in out.iter_mut().zip(column) {
            *o += c * scale;
        }
    }
    out
}

fn planckian(kelvin: f64) -> [f64; 2] {
    let t = kelvin.clamp(1667.0, 25000.0);
    let k = 1000.0 / t;
    let x = if t <= 4000.0 {
        ((-0.2661239 * k - 0.2343589) * k + 0.8776956) * k + 0.179910
    } else {
        ((-3.0258469 * k + 2.1070379) * k + 0.2226347) * k + 0.240390
    };
    let y = if t <= 2222.0 {
        ((-1.1063814 * x - 1.34811020) * x + 2.18555832) * x - 0.20219683
    } else if t <= 4000.0 {
        ((-0.9549476 * x - 1.37418593) * x + 2.09137015) * x - 0.16748867
    } else {
        ((3.0817580 * x - 5.87338670) * x + 3.75112997) * x - 0.37001483
    };
    let d = -2.0 * x + 12.0 * y + 3.0;
    [4.0 * x / d, 6.0 * y / d]
}

fn white_cones(kelvin: f64, tint: f64) -> [f64; 3] {
    let here = planckian(kelvin);
    let next = planckian(kelvin * 1.01);
    let along = [next[0] - here[0], next[1] - here[1]];
    let length = (along[0] * along[0] + along[1] * along[1]).sqrt().max(1e-12);
    let along = [along[0] / length, along[1] / length];
    let across = [along[1], -along[0]];
    let magenta = if across[1] > 0.0 { [-across[0], -across[1]] } else { across };
    let uv = [here[0] + magenta[0] * tint * 0.0007, here[1] + magenta[1] * tint * 0.0007];
    let d = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
    let x = 3.0 * uv[0] / d;
    let y = 2.0 * uv[1] / d;
    mul(&BRADFORD, [x / y, 1.0, (1.0 - x - y) / y])
}

fn luma(rgb: [f64; 3]) -> f64 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// The adjust package's white balance of a colour in light.
pub fn white_balance(rgb: [f64; 3], kelvin: f64, tint: f64) -> [f64; 3] {
    let to = white_cones(kelvin, tint);
    let from = white_cones(6500.0, 0.0);
    let gains = [to[0] / from[0], to[1] / from[1], to[2] / from[2]];
    let adapt = |v: [f64; 3]| {
        let cones = mul(&CONES, v);
        mul(&FROM_CONES, [cones[0] * gains[0], cones[1] * gains[1], cones[2] * gains[2]])
    };
    let white = luma(adapt([1.0; 3])).max(1e-6);
    let out = adapt(rgb);
    [out[0] / white, out[1] / white, out[2] / white]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(r: u8, g: u8, b: u8, spread: u8) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..4000u32 {
            let d = (i % 50) as i32 - 25;
            let s = i32::from(spread) * d / 25;
            let c = |v: u8| (i32::from(v) + s).clamp(0, 255) as u8;
            out.extend([c(r), c(g), c(b), 255]);
        }
        out
    }

    fn value(knobs: &Knobs, key: &str) -> f64 {
        knobs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap()
    }

    #[test]
    fn as_shot_white_balance_changes_nothing() {
        let out = white_balance([0.2, 0.4, 0.6], 6500.0, 0.0);
        assert!((out[0] - 0.2).abs() < 1e-6 && (out[2] - 0.6).abs() < 1e-6);
        // lower kelvin warms: red up against blue
        let warm = white_balance([0.5, 0.5, 0.5], 4000.0, 0.0);
        assert!(warm[0] > warm[2]);
    }

    #[test]
    fn a_dark_flat_frame_is_lifted_and_given_contrast() {
        let levels = measure(&frame(50, 50, 50, 15)).unwrap();
        let light = auto_light(&levels);
        assert!(value(&light, "exposure") > 0.5);
        assert!(value(&light, "contrast") > 5.0);
        // a well exposed, full-range frame is barely touched
        let good = measure(&frame(118, 118, 118, 118)).unwrap();
        let light = auto_light(&good);
        assert!(value(&light, "exposure").abs() < 0.3, "{light:?}");
        assert!(value(&light, "contrast").abs() < 3.0, "{light:?}");
    }

    #[test]
    fn a_blue_cast_is_warmed_and_a_neutral_frame_kept() {
        let levels = measure(&frame(100, 120, 170, 20)).unwrap();
        let color = auto_color(&levels);
        assert!(value(&color, "temperature") < 6000.0, "{color:?}");
        // the result really is greyer
        let t = value(&color, "temperature");
        let n = value(&color, "tint");
        let [r, g, b] = white_balance(levels.average, t, n);
        let [r0, _, b0] = levels.average;
        assert!((b / r) < (b0 / r0));
        let _ = g;
        let neutral = measure(&frame(120, 120, 120, 30)).unwrap();
        let color = auto_color(&neutral);
        assert!((value(&color, "temperature") - 6500.0).abs() < 300.0, "{color:?}");
        assert!(value(&color, "tint").abs() < 5.0, "{color:?}");
    }

    #[test]
    fn no_pixels_measure_nothing() {
        assert!(measure(&[]).is_none());
        assert!(measure(&[1, 2, 3]).is_none());
    }
}
