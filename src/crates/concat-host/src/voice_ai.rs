// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: "Mejorar voz (IA)" - a recording's voice cleaned the way a
//! podcast tool does it, offline:
//!
//! 1. the clipping of a hot microphone repaired (FFmpeg's `adeclip`), with
//!    some headroom made for what follows;
//! 2. noise, room and music taken out from under the voice by DeepFilterNet 3
//!    (Hendrik Schröter, MIT/Apache-2.0), its model built into the app;
//! 3. a studio voice's finish: rumble off, mud down, presence and air up,
//!    gentle compression, and the loudness social video expects (-14 LUFS).
//!
//! The whole file's sound is written as a new WAV beside it, at the same
//! length, so a clip that used the file uses the copy at the same times.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use concat_media::{AudioDecoder, AudioOptions, SampleFormat};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{Array2, Axis};

use crate::media::describe;

/// The model's rate.
const RATE: u32 = 48_000;

/// The finish after the model: the studio voice.
const FINISH: [&str; 6] = [
    "highpass=f=80",
    "equalizer=f=200:t=q:w=1:g=-2",
    "equalizer=f=3500:t=q:w=1.2:g=3",
    "equalizer=f=9000:t=h:w=2000:g=2",
    "acompressor=threshold=-20dB:ratio=3:attack=10:release=150:makeup=2",
    "loudnorm=I=-14:TP=-1.5:LRA=9",
];

/// `source`'s sound (stream `stream`, the first when None) enhanced into
/// `destination` (a WAV). `amount`, 0..=1, is how much noise is taken out:
/// all of it at 1, a natural room left in at lower values. `progress`
/// hears 0..=1 as it goes; `cancel` stops it.
pub fn enhance(
    source: &Path,
    stream: Option<usize>,
    destination: &Path,
    amount: f32,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<(), String> {
    // 1. decoded, declipped and given headroom, mono at the model's rate
    let mut decoder = AudioDecoder::open(
        source,
        &AudioOptions {
            filters: vec!["adeclip".to_owned(), "volume=0.6".to_owned()],
            rate: RATE,
            channels: 1,
            format: SampleFormat::F32,
            stream,
            ..AudioOptions::default()
        },
    )
    .map_err(describe)?;
    let noisy = decoder.collect_f32().map_err(describe)?;
    if noisy.is_empty() {
        return Err("El clip no tiene sonido".into());
    }
    progress(0.1);

    // 2. through DeepFilterNet, hop by hop
    let limit = 12.0 + 88.0 * amount.clamp(0.0, 1.0);
    let params = RuntimeParams::default_with_ch(1).with_atten_lim(limit);
    let mut model = DfTract::new(DfParams::default(), &params).map_err(|e| e.to_string())?;
    let hop = model.hop_size;
    // the model hears ahead and answers late by this much: read past the
    // end by it, and drop it from the start
    let delay = model.fft_size - model.hop_size + model.lookahead * model.hop_size;
    let mut input = noisy;
    let length = input.len();
    input.resize(length + delay + hop, 0.0);
    let frames = input.len() / hop;
    let mut output = Vec::with_capacity(frames * hop);
    let mut noisy_frame = Array2::<f32>::zeros((1, hop));
    let mut clean_frame = Array2::<f32>::zeros((1, hop));
    for index in 0..frames {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelado".into());
        }
        let chunk = &input[index * hop..(index + 1) * hop];
        noisy_frame
            .index_axis_mut(Axis(0), 0)
            .iter_mut()
            .zip(chunk)
            .for_each(|(slot, sample)| *slot = *sample);
        model
            .process(noisy_frame.view(), clean_frame.view_mut())
            .map_err(|e| e.to_string())?;
        output.extend(clean_frame.index_axis(Axis(0), 0).iter().copied());
        if index % 200 == 0 {
            progress(0.1 + 0.75 * index as f32 / frames as f32);
        }
    }
    let clean: Vec<f32> = output.into_iter().skip(delay).take(length).collect();

    // 3. the finish, through FFmpeg, from a scratch file
    let scratch = destination.with_extension("clean.wav");
    write_wav(&scratch, &clean)?;
    progress(0.88);
    let finished = (|| {
        let mut decoder = AudioDecoder::open(
            &scratch,
            &AudioOptions {
                filters: FINISH.iter().map(|f| (*f).to_owned()).collect(),
                rate: RATE,
                channels: 1,
                format: SampleFormat::F32,
                ..AudioOptions::default()
            },
        )
        .map_err(describe)?;
        let mut samples = decoder.collect_f32().map_err(describe)?;
        samples.resize(length, 0.0);
        write_wav(destination, &samples)
    })();
    let _ = std::fs::remove_file(&scratch);
    finished?;
    progress(1.0);
    Ok(())
}

/// Mono 48 kHz samples as a 16-bit WAV.
fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}
