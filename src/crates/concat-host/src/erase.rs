// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut)

//! TetalCut: Erase object - a copy of the span a clip shows with the
//! thing painted over taken out of every frame.
//!
//! Like Enhance and Reverse this is not done as the clip plays: a frame
//! takes the inpainting network the better part of a second on a phone.
//! The job reads the span the clip covers of its file, asks the mask
//! store what the clip's erase strokes cover at each frame's instant (the
//! smart brush's regions follow the thing along the span), fills that
//! with [`concat_vision::erase`], and writes the frames into the
//! project's cache with the span's sound beside them. The window then
//! points the clip at the copy, its in-point at the copy's zero, the way
//! a reversed clip is pointed at its copy, and the cutout that held the
//! strokes goes with the same step - so one undo brings the original and
//! its strokes back.
//!
//! The copy is named after the file, the span and every stroke, so the
//! same erase asked for twice is written once.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use concat_core::Rational;
use concat_media::audio::{AudioClip, mix_to_file, mux};
use concat_media::{
    DecodeOptions, Decoder, EncodeOptions, Encoder, FrameSink, FrameSource, RateMode, VideoCodec,
};
use concat_project::model::Cutout;
use concat_vision::erase::{Eraser, Made};
use concat_vision::{MaskStore, ModelId, mask_dir, models};

pub use crate::cutout::Progress;
use crate::jobs::SingleFlight;

/// What one erase covers.
#[derive(Clone, Debug)]
pub struct EraseRequest {
    /// The project folder, where the strokes' regions are cached.
    pub project: PathBuf,
    /// The media file to read.
    pub media_path: String,
    /// A still: one frame in, one PNG out.
    pub still: bool,
    /// The cutout holding the strokes, `erase` set.
    pub cutout: Cutout,
    /// The source's width over its height, for round brushes.
    pub aspect: f32,
    /// Where the span of the file the clip shows begins, in seconds.
    pub start: f64,
    /// How long that span is, in seconds of the file.
    pub duration: f64,
    /// Where the copy goes; see [`target_for`].
    pub target: PathBuf,
}

/// The erase service: the network, loaded once, and the one-job slot.
pub struct Erasers {
    gate: Arc<SingleFlight>,
    /// The app's data directory, where the downloaded network lives.
    data: PathBuf,
    model: Mutex<Option<Arc<Eraser>>>,
}

/// Where the erased copy of `media`'s span lives under `project`: in the
/// cache, named by the file's path, size and modification time, the span
/// and the strokes. None for a file that cannot be stat'ed.
pub fn target_for(
    project: &Path,
    media: &str,
    start: f64,
    duration: f64,
    cutout: &Cutout,
    still: bool,
) -> Option<PathBuf> {
    let meta = std::fs::metadata(media).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    media.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    if let Ok(modified) = meta.modified()
        && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        since.as_secs().hash(&mut hasher);
    }
    for stroke in &cutout.strokes {
        concat_vision::strokes::stroke_key(stroke).hash(&mut hasher);
    }
    let extension = if still { "png" } else { "mp4" };
    Some(project.join("cache").join(format!(
        "erase-{:016x}-{}-{}.{extension}",
        hasher.finish(),
        (start * 1000.0).round() as u64,
        (duration * 1000.0).round() as u64
    )))
}

impl Erasers {
    /// A service with nothing loaded yet, keeping its network under `data`.
    pub fn new(data: &Path) -> Erasers {
        Erasers {
            gate: Arc::new(SingleFlight::new()),
            data: data.to_path_buf(),
            model: Mutex::new(None),
        }
    }

    /// Whether an erase is running.
    pub fn is_busy(&self) -> bool {
        self.gate.is_busy()
    }

    /// Asks the running erase to stop after the frame in hand.
    pub fn cancel(&self) {
        self.gate.cancel();
    }

    /// The network, fetched first when it is not on disk.
    fn loaded(
        &self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Arc<Eraser>, String> {
        if let Some(loaded) = self
            .model
            .lock()
            .map_err(|_| "model slot poisoned")?
            .as_ref()
        {
            return Ok(Arc::clone(loaded));
        }
        let file = models::model_file(&self.data, ModelId::Eraser);
        if !models::installed(&self.data, ModelId::Eraser) {
            crate::cutout::fetch(ModelId::Eraser, &file, cancel, progress)?;
        }
        let eraser = Arc::new(Eraser::load(&file)?);
        *self.model.lock().map_err(|_| "model slot poisoned")? = Some(Arc::clone(&eraser));
        Ok(eraser)
    }

    /// Writes the copy `request` asks for, or finds it already written.
    /// Blocks until done, so run it on its own thread. Returns where the
    /// copy is.
    pub fn erase(
        &self,
        request: &EraseRequest,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<PathBuf, String> {
        if request.target.is_file() {
            return Ok(request.target.clone());
        }
        if !request.still
            && (!request.start.is_finite()
                || !request.duration.is_finite()
                || request.duration <= 0.0)
        {
            return Err("nothing to erase".to_owned());
        }
        let job = self.gate.begin("erase")?;
        let cancel = job.cancel_handle();
        let eraser = self.loaded(&cancel, progress)?;
        if let Some(folder) = request.target.parent() {
            std::fs::create_dir_all(folder)
                .map_err(|error| format!("{}: {error}", folder.display()))?;
        }
        progress(Progress::Analysing(0.0));
        let store = MaskStore::open(&mask_dir(
            &request.project,
            &request.media_path,
            request.cutout.subject,
        ));
        let result = if request.still {
            still(&eraser, &store, request)
        } else {
            footage(&eraser, &store, request, &job, progress)
        };
        result.map(|()| request.target.clone())
    }
}

/// The one frame, as a PNG.
fn still(eraser: &Eraser, store: &MaskStore, request: &EraseRequest) -> Result<(), String> {
    let mut decoder = Decoder::open(&request.media_path, &DecodeOptions::default().in_software())
        .map_err(|error| error.to_string())?;
    let frame = decoder
        .next_frame()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{} holds no picture", request.media_path))?;
    let mask = store
        .erase_mask(0.0, &request.cutout, request.aspect)
        .ok_or_else(|| "nothing is painted to erase".to_owned())?;
    let erased = eraser
        .erase(&frame, &mask, None)?
        .map(|(frame, _, _)| frame)
        .unwrap_or(frame);
    let partial = request.target.with_extension("part.png");
    let written = (|| {
        let file = std::fs::File::create(&partial).map_err(|error| error.to_string())?;
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(file),
            erased.width(),
            erased.height(),
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(erased.pixels())
            .map_err(|error| error.to_string())
    })();
    land(&partial, &request.target, written)
}

/// The span, frame by frame, each with its hole filled, then the span's
/// sound beside it.
fn footage(
    eraser: &Eraser,
    store: &MaskStore,
    request: &EraseRequest,
    job: &crate::jobs::Job,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), String> {
    let info = concat_media::probe(&request.media_path).map_err(|error| error.to_string())?;
    let video = info
        .video
        .as_ref()
        .ok_or_else(|| "no video stream".to_owned())?;
    let rate = video.frame_rate;
    let fps = rate.fps().as_f64().max(1.0);
    let expected = (request.duration * fps).max(1.0);
    let end = request.start + request.duration;

    let start = Rational::approximate(request.start).unwrap_or(Rational::ZERO);
    let options = DecodeOptions::default()
        .in_software()
        .threaded(2)
        .starting_at(start);
    let mut decoder =
        Decoder::open(&request.media_path, &options).map_err(|error| error.to_string())?;
    // Even sides, which the encoder wants, of the picture as decoded -
    // turned upright, so the copy needs no rotation of its own.
    let width = decoder.width() & !1;
    let height = decoder.height() & !1;
    if width == 0 || height == 0 {
        return Err("the picture is too small to erase".to_owned());
    }
    let partial_video = request.target.with_extension("part.mp4");
    let mut encoder = Encoder::create(
        &partial_video,
        width,
        height,
        rate,
        &EncodeOptions {
            codec: VideoCodec::H264,
            // A copy the timeline reads for as long as the project lives:
            // near-transparent, and the time goes on the network anyway.
            preset: "medium".to_owned(),
            crf: 16,
            rate_mode: RateMode::Vbr,
            bitrate_kbps: 0,
            ten_bit: false,
            color_range: concat_media::ColorRange::Limited,
            hardware: false,
            threads: 2,
        },
    )
    .map_err(|error| error.to_string())?;
    let encoded = (|| -> Result<u64, String> {
        let mut count = 0u64;
        let mut previous: Option<Made> = None;
        while let Some(frame) = decoder.next_frame().map_err(|error| error.to_string())? {
            if job.cancelled() {
                return Err("erase cancelled".to_owned());
            }
            // The frame's instant by its own clock where the container
            // keeps one, and by count where it does not.
            let at = decoder
                .position()
                .map(|at| at.as_f64())
                .unwrap_or(request.start + count as f64 / fps);
            if at >= end - 0.5 / fps && count > 0 {
                break;
            }
            let frame = crate::enhance::crop(&frame, width, height);
            let made = match store.erase_mask(at, &request.cutout, request.aspect) {
                Some(mask) => eraser.erase(&frame, &mask, previous.as_ref())?,
                None => None,
            };
            match &made {
                Some((erased, _, _)) => encoder.write_frame(erased),
                None => encoder.write_frame(&frame),
            }
            .map_err(|error| error.to_string())?;
            previous = made;
            count += 1;
            progress(Progress::Analysing(
                (count as f64 / expected).min(0.99) as f32
            ));
        }
        encoder.finish().map_err(|error| error.to_string())?;
        Ok(count)
    })();
    drop(encoder);
    drop(decoder);
    match encoded {
        Ok(0) => {
            let _ = std::fs::remove_file(&partial_video);
            return Err("the span holds no frames".to_owned());
        }
        Ok(_) => {}
        Err(error) => {
            let _ = std::fs::remove_file(&partial_video);
            return Err(error);
        }
    }

    if info.audio.is_none() {
        return land(&partial_video, &request.target, Ok(()));
    }
    let partial_audio = request.target.with_extension("part.m4a");
    let partial_muxed = request.target.with_extension("muxed.part.mp4");
    let sound = AudioClip {
        path: PathBuf::from(&request.media_path),
        stream: None,
        start: 0.0,
        duration: request.duration,
        source_start: request.start,
        speed: 1.0,
        preserve_pitch: true,
        volume: 1.0,
        volume_curve: concat_core::animate::Track::new(Vec::new()),
        fade_in: 0.0,
        fade_out: 0.0,
        filter_chain: String::new(),
    };
    let muxed = mix_to_file(&[sound], request.duration, &partial_audio)
        .and_then(|()| mux(&partial_video, &partial_audio, &partial_muxed))
        .map_err(|error| error.to_string());
    let _ = std::fs::remove_file(&partial_video);
    let _ = std::fs::remove_file(&partial_audio);
    land(&partial_muxed, &request.target, muxed)
}

/// A partial file moved into place, or cleared away with the error.
fn land(partial: &Path, target: &Path, written: Result<(), String>) -> Result<(), String> {
    match written {
        Ok(()) => std::fs::rename(partial, target)
            .map_err(|error| format!("{}: {error}", target.display())),
        Err(error) => {
            let _ = std::fs::remove_file(partial);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use concat_project::model::{BrushTool, Stroke};

    #[test]
    fn the_copy_is_named_after_the_file_the_span_and_the_strokes() {
        let dir = std::env::temp_dir().join(format!("concat-erase-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let media = dir.join("clip.mp4");
        std::fs::write(&media, b"not really").expect("write");
        let media = media.to_string_lossy().into_owned();
        let project = dir.join("project");
        let mut cutout = Cutout::erase();
        cutout.strokes.push(Stroke {
            at: Some(1.0),
            tool: BrushTool::SmartBrush,
            size: 0.05,
            points: vec![[0.5, 0.5]],
        });
        let one = target_for(&project, &media, 1.0, 2.0, &cutout, false).expect("named");
        assert!(one.starts_with(project.join("cache")));
        assert!(one.to_string_lossy().ends_with("-1000-2000.mp4"), "{one:?}");
        assert_eq!(
            target_for(&project, &media, 1.0, 2.0, &cutout, false),
            Some(one.clone())
        );
        cutout.strokes[0].points.push([0.6, 0.5]);
        assert_ne!(
            target_for(&project, &media, 1.0, 2.0, &cutout, false),
            Some(one)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// TetalCut: the whole job on a real file, when one is to hand: `TC_DATA`
/// an app data folder holding `cutout-models/migan-512.onnx`,
/// `TC_ERASE_VIDEO` a clip with a thing in the middle of its picture.
#[cfg(test)]
mod real {
    use super::*;
    use concat_project::model::{BrushTool, Stroke};

    #[test]
    #[ignore = "needs the model and a video"]
    fn erases_the_middle_of_a_real_clip() {
        let var = |name: &str| std::env::var(name).expect(name);
        let data = PathBuf::from(var("TC_DATA"));
        let video = var("TC_ERASE_VIDEO");
        let project = data.join("project");
        let mut cutout = Cutout::erase();
        cutout.strokes.push(Stroke {
            at: Some(0.0),
            tool: BrushTool::Brush,
            size: 0.12,
            points: vec![[0.45, 0.5], [0.55, 0.5]],
        });
        let request = EraseRequest {
            project: project.clone(),
            media_path: video.clone(),
            still: false,
            target: target_for(&project, &video, 0.5, 2.0, &cutout, false).expect("named"),
            cutout,
            aspect: 16.0 / 9.0,
            start: 0.5,
            duration: 2.0,
        };
        let _ = std::fs::remove_file(&request.target);
        let erasers = Erasers::new(&data);
        let started = std::time::Instant::now();
        let out = erasers.erase(&request, &mut |_| {}).expect("erased");
        eprintln!("erased in {:?} into {}", started.elapsed(), out.display());
        let info = concat_media::probe(&out).expect("probe");
        let duration = info.duration.expect("a length").as_f64();
        assert!((duration - 2.0).abs() < 0.15, "{duration}");
        assert!(info.audio.is_some());
    }
}
