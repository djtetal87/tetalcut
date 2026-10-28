// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut)

//! TetalCut: Erase object - a thing painted over on the monitor taken out
//! of the picture, and the hole it leaves filled from what is around it.
//!
//! The filling is MI-GAN, a small inpainting network made to run on a
//! phone: a 512 square in, with a mask saying which pixels are missing,
//! and the same square out with them made up. A frame is far larger than
//! that and the hole is usually a small part of it, so only a window
//! around the hole is read - twice the hole's size, so the network sees
//! enough of what surrounds it, and never under 512 pixels, so a small
//! thing in a 4K frame is filled at its own detail rather than from a
//! blurred thumbnail. The answer is scaled back into the window and laid
//! over the frame through the hole's softened edge, so the frame outside
//! the hole is never touched.
//!
//! The hole is the mask a custom cutout paints - the strokes, and the
//! regions the smart brush follows along the clip - grown a little, since
//! a thing's edge and its shadow sit just outside where a brush or a
//! model puts it, and an outline left behind reads worse than a slightly
//! larger fill. Everything but the network is a pure function here,
//! tested without a model; [`Eraser`] needs the `infer` feature.

use concat_core::Frame;
use concat_core::frame::BYTES_PER_PIXEL;

use crate::Mask;

/// The network's square.
pub const SIDE: u32 = 512;
/// A mask pixel at least this likely is part of the hole.
const HOLE_IS: f32 = 0.35;
/// How far the hole grows past the mask, as a fraction of the frame's
/// longer side.
const GROW: f32 = 0.012;
/// The least it grows, in frame pixels.
const GROW_LEAST: f32 = 4.0;
/// How soft the hole's edge is when the fill is laid over the frame, in
/// network pixels.
const FEATHER: f32 = 2.0;
/// How much of the previous frame's fill is kept where both frames have a
/// hole: enough to calm the flicker of a fill made one frame at a time,
/// little enough that a moving camera does not smear.
pub const CARRY: f32 = 0.35;

/// A rectangle of frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Where the mask says the hole is, as fractions of the picture: left,
/// top, right, bottom. `None` when nothing is painted.
pub fn bounds(mask: &Mask) -> Option<[f32; 4]> {
    let (width, height) = (mask.width() as usize, mask.height() as usize);
    let threshold = (HOLE_IS * 255.0) as u8;
    let mut found: Option<[usize; 4]> = None;
    for (index, value) in mask.bytes().iter().enumerate() {
        if *value < threshold {
            continue;
        }
        let (x, y) = (index % width, index / width);
        found = Some(match found {
            None => [x, y, x, y],
            Some([l, t, r, b]) => [l.min(x), t.min(y), r.max(x), b.max(y)],
        });
    }
    let [l, t, r, b] = found?;
    Some([
        l as f32 / width as f32,
        t as f32 / height as f32,
        (r + 1) as f32 / width as f32,
        (b + 1) as f32 / height as f32,
    ])
}

/// How far the hole grows, in pixels of a `width` × `height` frame.
pub fn grow(width: u32, height: u32) -> f32 {
    (width.max(height) as f32 * GROW).max(GROW_LEAST)
}

/// The window of a `width` × `height` frame the network reads for a hole
/// at `bounds` (fractions): the hole grown, then twice its size around its
/// centre, at least [`SIDE`] where the frame allows, square where it fits,
/// and kept inside the frame.
pub fn window(bounds: [f32; 4], width: u32, height: u32) -> Window {
    let (fw, fh) = (width.max(1) as f32, height.max(1) as f32);
    let g = grow(width, height);
    let left = (bounds[0] * fw - g).max(0.0);
    let top = (bounds[1] * fh - g).max(0.0);
    let right = (bounds[2] * fw + g).min(fw);
    let bottom = (bounds[3] * fh + g).min(fh);
    let (hole_w, hole_h) = ((right - left).max(1.0), (bottom - top).max(1.0));
    let least = (SIDE as f32).min(fw).min(fh);
    let side = (hole_w.max(hole_h) * 2.0).max(least);
    let w = side.max(hole_w).min(fw).round().max(1.0);
    let h = side.max(hole_h).min(fh).round().max(1.0);
    let cx = (left + right) / 2.0;
    let cy = (top + bottom) / 2.0;
    let x = (cx - w / 2.0).clamp(0.0, fw - w).round();
    let y = (cy - h / 2.0).clamp(0.0, fh - h).round();
    Window {
        x: x as u32,
        y: y as u32,
        width: (w as u32).min(width - x as u32),
        height: (h as u32).min(height - y as u32),
    }
}

/// The hole over the window, at the network's size: 255 where a pixel is
/// missing, grown by `grow_px` frame pixels. `mask` is over the whole
/// frame, in fractions.
pub fn hole(mask: &Mask, window: Window, frame: (u32, u32), grow_px: f32) -> Mask {
    let side = SIDE as usize;
    let (fw, fh) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let mut out = Mask::filled(SIDE, SIDE, 0);
    {
        let data = out.bytes_mut();
        for y in 0..side {
            let v = (window.y as f32 + (y as f32 + 0.5) / side as f32 * window.height as f32) / fh;
            for x in 0..side {
                let u =
                    (window.x as f32 + (x as f32 + 0.5) / side as f32 * window.width as f32) / fw;
                if mask.sample(u, v) >= HOLE_IS {
                    data[y * side + x] = 255;
                }
            }
        }
    }
    // The growth in network pixels, which may differ across and down when
    // the window is not square.
    let rx = (grow_px * side as f32 / window.width.max(1) as f32).round() as usize;
    let ry = (grow_px * side as f32 / window.height.max(1) as f32).round() as usize;
    dilate(&out, rx, ry)
}

/// `mask` with every set pixel spread `rx` across and `ry` down: a box,
/// row pass then column pass.
pub fn dilate(mask: &Mask, rx: usize, ry: usize) -> Mask {
    let (width, height) = (mask.width() as usize, mask.height() as usize);
    let source = mask.bytes();
    let mut rows = vec![0u8; source.len()];
    for y in 0..height {
        let row = &source[y * width..(y + 1) * width];
        for x in 0..width {
            let from = x.saturating_sub(rx);
            let to = (x + rx).min(width - 1);
            rows[y * width + x] = row[from..=to].iter().copied().max().unwrap_or(0);
        }
    }
    let mut out = vec![0u8; source.len()];
    for x in 0..width {
        for y in 0..height {
            let from = y.saturating_sub(ry);
            let to = (y + ry).min(height - 1);
            out[y * width + x] = (from..=to).map(|yy| rows[yy * width + x]).max().unwrap_or(0);
        }
    }
    Mask::from_bytes(mask.width(), mask.height(), out).expect("same size")
}

/// The window of `frame` scaled to the network's square, as planar RGB
/// bytes: bilinear, which is all a downscale of at most a few times needs.
pub fn read_window(frame: &Frame, window: Window) -> Vec<u8> {
    let side = SIDE as usize;
    let mut out = vec![0u8; 3 * side * side];
    for y in 0..side {
        let fy = window.y as f32 + (y as f32 + 0.5) / side as f32 * window.height as f32 - 0.5;
        for x in 0..side {
            let fx = window.x as f32 + (x as f32 + 0.5) / side as f32 * window.width as f32 - 0.5;
            let rgb = bilinear(frame, fx, fy);
            for c in 0..3 {
                out[(c * side + y) * side + x] = rgb[c];
            }
        }
    }
    out
}

/// A frame's colour between pixels, edges clamped.
fn bilinear(frame: &Frame, x: f32, y: f32) -> [u8; 3] {
    let (width, height) = (frame.width() as i64, frame.height() as i64);
    let pixels = frame.pixels();
    let x0 = x.floor();
    let y0 = y.floor();
    let (fx, fy) = (x - x0, y - y0);
    let at = |px: i64, py: i64, c: usize| -> f32 {
        let px = px.clamp(0, width - 1) as usize;
        let py = py.clamp(0, height - 1) as usize;
        f32::from(pixels[(py * width as usize + px) * BYTES_PER_PIXEL + c])
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let mut out = [0u8; 3];
    for (c, value) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
        let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
        *value = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// A planar RGB square's colour between pixels, edges clamped.
fn planar(rgb: &[u8], x: f32, y: f32) -> [f32; 3] {
    let side = SIDE as i64;
    let x0 = x.floor();
    let y0 = y.floor();
    let (fx, fy) = (x - x0, y - y0);
    let at = |px: i64, py: i64, c: usize| -> f32 {
        let px = px.clamp(0, side - 1) as usize;
        let py = py.clamp(0, side - 1) as usize;
        f32::from(rgb[(c * SIDE as usize + py) * SIDE as usize + px])
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let mut out = [0f32; 3];
    for (c, value) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
        let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
        *value = top * (1.0 - fy) + bottom * fy;
    }
    out
}

/// `frame` with the network's answer `filled` (planar RGB over the
/// window) laid in through `hole`, softened at its edge. Where `previous`
/// - the last frame this made - had a hole too, a little of its fill is
/// kept, so the fill does not flicker from frame to frame.
pub fn lay_in(
    frame: &Frame,
    window: Window,
    filled: &[u8],
    hole: &Mask,
    previous: Option<&(Frame, Mask, Window)>,
) -> Frame {
    let mut out = frame.clone();
    let soft = hole.blurred(FEATHER);
    let side = SIDE as f32;
    let width = frame.width() as usize;
    let pixels = out.pixels_mut();
    for y in window.y..window.y + window.height {
        let v = (y - window.y) as f32 + 0.5;
        let ny = v / window.height as f32 * side - 0.5;
        for x in window.x..window.x + window.width {
            let u = (x - window.x) as f32 + 0.5;
            let nx = u / window.width as f32 * side - 0.5;
            let alpha = soft.sample(u / window.width as f32, v / window.height as f32);
            if alpha <= 0.0 {
                continue;
            }
            let mut fill = planar(filled, nx, ny);
            if let Some((before, before_hole, before_window)) = previous
                && before.width() == frame.width()
                && before.height() == frame.height()
                && x >= before_window.x
                && y >= before_window.y
                && x < before_window.x + before_window.width
                && y < before_window.y + before_window.height
            {
                let bu = (x - before_window.x) as f32 + 0.5;
                let bv = (y - before_window.y) as f32 + 0.5;
                let was = before_hole.sample(
                    bu / before_window.width as f32,
                    bv / before_window.height as f32,
                );
                if was >= 0.5 {
                    let index = (y as usize * width + x as usize) * BYTES_PER_PIXEL;
                    let old = &before.pixels()[index..index + 3];
                    for c in 0..3 {
                        fill[c] = fill[c] * (1.0 - CARRY) + f32::from(old[c]) * CARRY;
                    }
                }
            }
            let index = (y as usize * width + x as usize) * BYTES_PER_PIXEL;
            for c in 0..3 {
                let shot = f32::from(pixels[index + c]);
                pixels[index + c] =
                    (shot * (1.0 - alpha) + fill[c] * alpha).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// What [`Eraser::erase`] made of a frame, to hand to the next one.
pub type Made = (Frame, Mask, Window);

#[cfg(feature = "infer")]
pub use inference::Eraser;

#[cfg(feature = "infer")]
mod inference {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::runtime::{Input, Model};

    /// The inpainting network, loaded.
    pub struct Eraser {
        model: Mutex<Model>,
    }

    impl Eraser {
        /// Loads the network from its downloaded file; see
        /// [`crate::models::ModelId::Eraser`].
        pub fn load(path: &Path) -> Result<Eraser, String> {
            Ok(Eraser {
                model: Mutex::new(Model::from_file_on_cpu(path)?),
            })
        }

        /// `frame` with what `mask` (over the whole source, in fractions)
        /// covers taken out and filled. `None` when the mask is empty:
        /// the frame is fine as it is. `previous` is what this returned
        /// for the frame before, for a steadier fill.
        pub fn erase(
            &self,
            frame: &Frame,
            mask: &Mask,
            previous: Option<&Made>,
        ) -> Result<Option<Made>, String> {
            let Some(bounds) = bounds(mask) else {
                return Ok(None);
            };
            if frame.width() == 0 || frame.height() == 0 {
                return Ok(None);
            }
            let size = (frame.width(), frame.height());
            let window = window(bounds, size.0, size.1);
            let hole = hole(mask, window, size, grow(size.0, size.1));
            if hole.is_blank() {
                return Ok(None);
            }
            let image = read_window(frame, window);
            // The network's mask is the other way round: 255 is a pixel
            // to keep, 0 one to make up.
            let keep: Vec<u8> = hole.bytes().iter().map(|v| 255 - *v).collect();
            let side = SIDE as usize;
            let mut answered = self
                .model
                .lock()
                .map_err(|_| "eraser poisoned".to_owned())?
                .run_bytes(
                    vec![
                        Input {
                            name: "image",
                            dims: vec![1, 3, side, side],
                            data: image.into(),
                        },
                        Input {
                            name: "mask",
                            dims: vec![1, 1, side, side],
                            data: keep.into(),
                        },
                    ],
                    &["result"],
                )?;
            let (dims, filled) = answered.pop().ok_or("eraser: no answer")?;
            if dims != [1, 3, side, side] || filled.len() != 3 * side * side {
                return Err(format!("eraser: an answer of the wrong shape {dims:?}"));
            }
            let out = lay_in(frame, window, &filled, &hole, previous);
            Ok(Some((out, hole, window)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted(width: u32, height: u32, rect: [u32; 4]) -> Mask {
        let mut mask = Mask::filled(width, height, 0);
        for y in rect[1]..rect[3] {
            for x in rect[0]..rect[2] {
                mask.bytes_mut()[(y * width + x) as usize] = 255;
            }
        }
        mask
    }

    #[test]
    fn nothing_painted_has_no_bounds() {
        assert_eq!(bounds(&Mask::filled(16, 9, 0)), None);
        let b = bounds(&painted(16, 8, [4, 2, 8, 4])).expect("painted");
        assert_eq!(b, [0.25, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn a_small_thing_in_a_big_frame_is_read_at_its_own_detail() {
        // A 40 pixel thing in the middle of a 4K frame: the window is the
        // network's own size, around it, so nothing is scaled.
        let (w, h) = (3840, 2160);
        let b = [1900.0 / 3840.0, 1060.0 / 2160.0, 1940.0 / 3840.0, 1100.0 / 2160.0];
        let win = window(b, w, h);
        assert_eq!((win.width, win.height), (512, 512));
        assert!(win.x < 1900 && win.x + win.width > 1940, "{win:?}");
        assert!(win.y < 1060 && win.y + win.height > 1100, "{win:?}");
    }

    #[test]
    fn a_big_thing_gets_room_around_it_and_stays_in_the_frame() {
        let (w, h) = (1920, 1080);
        // A person on the right edge, 300 wide and most of the height.
        let b = [1600.0 / 1920.0, 200.0 / 1080.0, 1900.0 / 1920.0, 1000.0 / 1080.0];
        let win = window(b, w, h);
        assert!(win.x + win.width <= w && win.y + win.height <= h, "{win:?}");
        assert!(win.width >= 300 && win.height >= 800, "{win:?}");
        assert!(win.x <= 1600 - 23, "{win:?}");
        // A thing wider than the frame is tall still fits the window.
        let wide = window([0.05, 0.4, 0.95, 0.6], w, h);
        assert!(wide.width >= 1700, "{wide:?}");
        assert_eq!(wide.height, 1080);
    }

    #[test]
    fn the_hole_grows_past_what_was_painted() {
        let mask = painted(64, 64, [28, 28, 36, 36]);
        let win = Window {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        let hole = hole(&mask, win, (64, 64), 4.0);
        // 64 frame pixels over 512: a frame pixel is 8 network pixels.
        let at = |x: f32, y: f32| hole.sample(x / 64.0, y / 64.0);
        assert!(at(32.0, 32.0) > 0.9);
        assert!(at(25.0, 32.0) > 0.9, "grown to the left");
        assert!(at(20.0, 32.0) < 0.1, "not past the growth");
        assert!(at(5.0, 5.0) < 0.1);
    }

    #[test]
    fn only_the_hole_changes() {
        let mut frame = Frame::black(64, 64);
        frame.fill([10, 20, 30, 255]);
        let win = Window {
            x: 16,
            y: 16,
            width: 32,
            height: 32,
        };
        let mut hole = Mask::filled(SIDE, SIDE, 0);
        for y in 192..320 {
            for x in 192..320 {
                hole.bytes_mut()[y * SIDE as usize + x] = 255;
            }
        }
        let filled = vec![200u8; 3 * (SIDE * SIDE) as usize];
        let out = lay_in(&frame, win, &filled, &hole, None);
        assert_eq!(out.pixel(2, 2), Some([10, 20, 30, 255]));
        assert_eq!(out.pixel(17, 17), Some([10, 20, 30, 255]));
        assert_eq!(out.pixel(32, 32), Some([200, 200, 200, 255]));
    }

    #[test]
    fn the_last_fill_steadies_the_next() {
        let mut frame = Frame::black(64, 64);
        frame.fill([0, 0, 0, 255]);
        let win = Window {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        let hole = Mask::filled(SIDE, SIDE, 255);
        let mut before = Frame::black(64, 64);
        before.fill([100, 100, 100, 255]);
        let filled = vec![200u8; 3 * (SIDE * SIDE) as usize];
        let out = lay_in(&frame, win, &filled, &hole, Some(&(before, hole.clone(), win)));
        let [r, ..] = out.pixel(32, 32).expect("inside");
        assert!((160..=170).contains(&r), "{r}");
    }

    #[test]
    fn the_window_is_read_as_planar_rgb() {
        let mut frame = Frame::black(8, 8);
        frame.fill([255, 0, 7, 255]);
        let image = read_window(
            &frame,
            Window {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
        );
        let side = (SIDE * SIDE) as usize;
        assert_eq!(image.len(), 3 * side);
        assert_eq!(image[0], 255);
        assert_eq!(image[side], 0);
        assert_eq!(image[2 * side + 5], 7);
    }
}

/// TetalCut: the network on a real picture, when one is to hand:
/// `TC_MIGAN` the model, `TC_ERASE_IN` a PNG, `TC_ERASE_MASK` a grey PNG
/// of the same picture (white is the thing), `TC_ERASE_OUT` where to
/// write the result.
#[cfg(all(test, feature = "infer"))]
mod real {
    use super::*;

    fn read_png(path: &str) -> (u32, u32, png::ColorType, Vec<u8>) {
        let decoder = png::Decoder::new(std::fs::File::open(path).expect("open"));
        let mut reader = decoder.read_info().expect("info");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("frame");
        buf.truncate(info.buffer_size());
        (info.width, info.height, info.color_type, buf)
    }

    #[test]
    #[ignore = "needs the model and a picture"]
    fn erases_a_real_picture() {
        let var = |name: &str| std::env::var(name).expect(name);
        let eraser = Eraser::load(std::path::Path::new(&var("TC_MIGAN"))).expect("load");
        let (w, h, colour, bytes) = read_png(&var("TC_ERASE_IN"));
        let rgba: Vec<u8> = match colour {
            png::ColorType::Rgba => bytes,
            png::ColorType::Rgb => bytes
                .chunks_exact(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            other => panic!("{other:?}"),
        };
        let frame = Frame::from_rgba(w, h, rgba).expect("frame");
        let (mw, mh, _, grey) = read_png(&var("TC_ERASE_MASK"));
        let mask = Mask::from_bytes(mw, mh, grey).expect("mask");
        let started = std::time::Instant::now();
        let (out, _, window) = eraser.erase(&frame, &mask, None).expect("erase").expect("a hole");
        eprintln!("erased in {:?}, window {window:?}", started.elapsed());
        let file = std::fs::File::create(var("TC_ERASE_OUT")).expect("create");
        let mut encoder = png::Encoder::new(file, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("header")
            .write_image_data(out.pixels())
            .expect("write");
    }
}
