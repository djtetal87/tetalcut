// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer and Concat contributors

//! The captions sheet: the tray's Captions tool, as a form and then as a
//! progress report.
//!
//! One switch says what it captions: "Use voice from timeline", on, takes
//! the words from a clip with sound, chosen in the sheet from the clips on
//! the timeline that have any; off, from a script typed in it. It opens on
//! when a clip with sound is selected, or when a clip's menu opened it on
//! that clip, and off otherwise. Either lands as one batch of title clips,
//! one undo step.
//! The transcriber runs on a worker and reports as
//! [`CaptionsMsg::Progress`], ending as [`CaptionsMsg::Finished`].

use std::sync::Arc;

use concat_project::Command;
use concat_project::model::{ClipKey, KeyEase, KeyProperty, TextStyle};
use concat_speech::JobStage;
use concat_speech::transcribe::{Segment, Word};

use crate::host::{on_ui_in_project, spawn_in_project};
use crate::i18n::{t, tf};
use crate::panes::Msg;
use crate::panes::pace::{Pace, Stage, Timings};
use crate::panes::settings::installed;
use crate::studio::Studio;
use crate::ui::CaptionsSheetData;

/// Where a caption sits, by the sheet's row: a frame-height fraction from
/// the centre, positive down. Bottom, centre, top.
const CAPTION_OFFSETS: [f64; 3] = [0.35, 0.0, -0.35];
/// A caption's cap height by the sheet's row, as a fraction of the frame.
const CAPTION_SIZES: [f64; 3] = [0.04, 0.05, 0.065];
/// A rough speaking rate, for a script's timing and the speech sheet's
/// estimate.
pub const CHARS_PER_SECOND: f32 = 14.0;

/// Everything that can happen to the captions sheet.
#[derive(Clone, Debug)]
pub enum CaptionsMsg {
    /// The tray's Captions tool: the selected clip's voice when it has
    /// sound, the script otherwise.
    Open,
    /// A clip's menu: the sheet on that clip's voice.
    OpenOnClip(String),
    Close,
    /// The "Use voice from timeline" switch.
    VoiceToggled(bool),
    /// Row in the list of clips with sound.
    SourceChanged(i32),
    TextEdited(String),
    ModelChanged(i32),
    PlacementChanged(i32),
    SizeChanged(i32),
    /// TetalCut: still, pop in, or word by word.
    StyleChanged(i32),
    /// TetalCut: the caption template (colours, outline, box, font).
    TemplateChanged(i32),
    /// Run the pass the sheet describes.
    Begin,
    Cancel,
    /// The transcriber's worker reporting the stage it is in.
    Progress(JobStage),
    /// The transcriber's worker is done: what was said, or why not.
    Finished(Result<Vec<Segment>, String>),
}

/// The clip a transcription is running over, as the finished segments
/// need it: where it starts on the timeline and how fast it plays.
#[derive(Clone, Debug)]
struct Subject {
    start: f64,
    speed: f64,
    /// The look chosen when the run began.
    look: (f64, f64),
    /// The style chosen when the run began (see [`CaptionsPane::style`]).
    style: usize,
    /// The template chosen when the run began.
    template: usize,
    /// The transcriber model, and the thirty-second windows whisper reads
    /// the sound in: what the run's timing is kept by.
    model: String,
    windows: f32,
}

/// Seconds whisper takes over one thirty-second window, per megabyte of
/// model, before a run on the model has been timed: about a second for
/// base and ten for medium on eight cores of a recent CPU.
const WINDOW_SECONDS_PER_MB: f32 = 1.0 / 160.0;

/// The captions sheet's state.
#[derive(Default)]
pub struct CaptionsPane {
    pub open: bool,
    /// "Use voice from timeline": the words come from [`Self::clip`]'s sound
    /// when on, from [`Self::text`] when off.
    pub from_voice: bool,
    /// The clip whose voice is transcribed: one with sound, chosen from the
    /// timeline's. Kept while the switch is off, so turning it back on
    /// returns to the same clip.
    pub clip: Option<String>,
    /// The script's words.
    pub text: String,
    /// Row in the installed transcriber list.
    pub model: usize,
    /// 0 bottom, 1 centre, 2 top.
    pub placement: usize,
    /// 0 small, 1 medium, 2 large.
    pub size: usize,
    /// TetalCut: 0 still (Concat's), 1 each phrase pops in, 2 word by word.
    pub style: usize,
    /// Whether a style was chosen this session (else the sheet opens on pop in).
    style_chosen: bool,
    /// TetalCut: row in [`TEMPLATES`], Clásico first. Kept for the session.
    pub template: usize,
    pub running: bool,
    /// How far the running transcription looks.
    pub pace: Pace,
    /// Why the last run failed, when it did.
    pub message: String,
    /// The clip under the running transcription.
    subject: Option<Subject>,
    /// How long each model took this session, for the next run's bar.
    timings: Timings,
}

impl CaptionsPane {
    /// Applies one message. The studio is the rest of the window; while
    /// this runs the studio's copy of the pane is a blank it must not read.
    pub fn update(&mut self, msg: CaptionsMsg, studio: &mut Studio) {
        match msg {
            CaptionsMsg::Open => {
                let selected = studio.sole_selection();
                self.open_on(studio, selected, false);
            }
            CaptionsMsg::OpenOnClip(id) => self.open_on(studio, Some(id), true),
            CaptionsMsg::Close => self.open = false,
            CaptionsMsg::VoiceToggled(on) => {
                self.from_voice = on;
                self.message.clear();
                let sources = sources(studio);
                if on
                    && !self
                        .clip
                        .as_ref()
                        .is_some_and(|id| sources.iter().any(|s| &s.id == id))
                {
                    self.clip = sources.first().map(|source| source.id.clone());
                }
            }
            CaptionsMsg::SourceChanged(index) => {
                if let Some(source) = usize::try_from(index)
                    .ok()
                    .and_then(|index| sources(studio).into_iter().nth(index))
                {
                    self.clip = Some(source.id);
                    self.message.clear();
                }
            }
            CaptionsMsg::TextEdited(text) => self.text = text,
            CaptionsMsg::ModelChanged(index) => self.model = index.max(0) as usize,
            CaptionsMsg::PlacementChanged(index) => {
                self.placement = (index.max(0) as usize).min(2);
            }
            CaptionsMsg::SizeChanged(index) => self.size = (index.max(0) as usize).min(2),
            CaptionsMsg::TemplateChanged(index) => {
                self.template = (index.max(0) as usize).min(TEMPLATES.len() - 1);
            }
            CaptionsMsg::StyleChanged(index) => {
                self.style = (index.max(0) as usize).min(2);
                self.style_chosen = true;
            }
            CaptionsMsg::Begin => {
                if self.from_voice {
                    self.run_sound(studio);
                } else {
                    self.run_script(studio);
                }
            }
            CaptionsMsg::Cancel => {
                studio.host.transcriber.cancel();
                self.running = false;
                self.open = false;
            }
            CaptionsMsg::Progress(stage) => self.pace.report(stage),
            CaptionsMsg::Finished(result) => {
                self.running = false;
                let subject = self.subject.take();
                if let (Ok(_), Some(subject)) = (&result, &subject) {
                    self.timings
                        .keep(&subject.model, self.pace.learned(subject.windows));
                }
                match result {
                    Ok(segments) => {
                        let Some(subject) = subject else {
                            return;
                        };
                        let (base, accent) =
                            template_styles(subject.template, studio.prefs.title_style.as_ref());
                        let commands = segment_captions(
                            &segments,
                            subject.start,
                            subject.speed,
                            subject.look,
                            subject.style,
                            base.as_ref(),
                            accent.as_ref(),
                        );
                        let count = commands.len();
                        self.open = false;
                        if count == 0 {
                            studio.notify(&t("captions.nothingSaidClip"), true);
                        } else {
                            studio.apply(Command::Batch { commands });
                            studio.notify(&tf("captions.addedCaptions", &[&count]), false);
                        }
                    }
                    // Asked for: the sheet is already on its way down.
                    Err(error) if error.contains("cancel") => self.open = false,
                    Err(error) => self.message = error,
                }
            }
        }
    }

    /// The sheet opened afresh. `clip` is the clip it was opened on, if
    /// any: the switch starts on when that clip has sound, or when a clip's
    /// menu asked for its voice (`voice`), in which case a clip without
    /// sound falls to the first clip that has some.
    fn open_on(&mut self, studio: &Studio, clip: Option<String>, voice: bool) {
        let installed = installed(&studio.settings.transcribers);
        let model = installed.iter().position(|model| model.active).unwrap_or(0);
        let sources = sources(studio);
        let sounding = clip.filter(|id| sources.iter().any(|source| &source.id == id));
        let from_voice = voice || sounding.is_some();
        let clip = sounding.or_else(|| {
            from_voice
                .then(|| sources.first().map(|source| source.id.clone()))
                .flatten()
        });
        *self = CaptionsPane {
            open: true,
            from_voice,
            clip,
            model,
            placement: 0,
            size: 1,
            // the last style chosen this session; pop in the first time
            style: if self.style_chosen { self.style } else { 1 },
            style_chosen: self.style_chosen,
            template: self.template,
            timings: std::mem::take(&mut self.timings),
            ..CaptionsPane::default()
        };
    }

    /// A caption's look, by the sheet's rows: where it sits and its size.
    fn look(&self) -> (f64, f64) {
        (
            CAPTION_OFFSETS[self.placement.min(2)],
            CAPTION_SIZES[self.size.min(2)],
        )
    }

    /// The script as titles, one after another from the playhead.
    fn run_script(&mut self, studio: &mut Studio) {
        let lines = script_captions(&self.text);
        if lines.is_empty() {
            self.message = t("captions.nothingToCaptionYet");
            return;
        }
        let look = self.look();
        let mut at = 0.0;
        let (base, accent) = template_styles(self.template, studio.prefs.title_style.as_ref());
        // the script's lines as segments without word times: their words
        // are spread over each line by length (see `segment_captions`)
        let segments: Vec<Segment> = lines
            .into_iter()
            .map(|(text, seconds)| {
                let segment = Segment {
                    start: at,
                    end: at + seconds,
                    text,
                    words: Vec::new(),
                };
                at += seconds;
                segment
            })
            .collect();
        let commands = segment_captions(
            &segments,
            f64::from(studio.playhead),
            1.0,
            look,
            self.style,
            base.as_ref(),
            accent.as_ref(),
        );
        let count = commands.len();
        self.open = false;
        studio.apply(Command::Batch { commands });
        studio.notify(&tf("captions.addedCaptions", &[&count]), false);
    }

    /// The sheet's clip through the transcriber on a worker, reporting into
    /// the sheet as it goes.
    fn run_sound(&mut self, studio: &mut Studio) {
        let Some(clip) = self.clip.as_ref().and_then(|id| studio.clip(id)).cloned() else {
            self.message = t("captions.clipNoLongerTimeline");
            return;
        };
        let Some(media) = studio.project().media_by_id(&clip.media_id).cloned() else {
            self.message = t("captions.clipHasNoFile");
            return;
        };
        let Some((model, megabytes)) = installed(&studio.settings.transcribers)
            .get(self.model)
            .map(|model| (model.id.clone(), model.megabytes))
        else {
            self.message = t("captions.downloadTranscriberModelFirst");
            return;
        };
        let request = concat_speech::transcribe::TranscribeRequest {
            path: media.path.clone(),
            audio_stream: clip.audio_stream,
            source_start: clip.source_start,
            window: clip.duration * clip.speed,
            model_id: model.clone(),
        };
        let dirs = studio.host.dirs.clone();
        let transcriber = Arc::clone(&studio.host.transcriber);
        // The run as the bar lays it out: the sound read in at a few
        // hundred times its length, the model loaded unless it still is,
        // and whisper a window at a time - at the pace the last run on
        // this model measured, or the estimate by its size.
        let windows = (request.window / 30.0).ceil().max(1.0) as f32;
        let learned = self.timings.get(&model);
        let loaded = studio.host.transcriber.loaded_model().as_deref() == Some(model.as_str());
        let load = if loaded {
            0.0
        } else {
            learned.load.unwrap_or(0.5 + megabytes / 400.0)
        };
        let per_window = learned
            .per_unit
            .unwrap_or(0.3 + megabytes * WINDOW_SECONDS_PER_MB);
        self.pace = Pace::new(vec![
            (Stage::Reading, 0.2 + request.window as f32 / 400.0),
            (Stage::Loading, load),
            (Stage::Working, windows * per_window),
        ]);
        self.subject = Some(Subject {
            start: clip.start,
            speed: clip.speed,
            look: self.look(),
            style: self.style,
            template: self.template,
            model,
            windows,
        });
        self.running = true;
        self.message.clear();
        let epoch = crate::host::project_epoch();
        spawn_in_project(
            move || {
                transcriber.transcribe(&dirs, &request, move |stage| {
                    on_ui_in_project(epoch, move |studio, _, _| {
                        studio.handle(Msg::Captions(CaptionsMsg::Progress(stage)));
                    });
                })
            },
            |studio, _, _, result| studio.handle(Msg::Captions(CaptionsMsg::Finished(result))),
        );
    }

    /// The sheet as Slint shows it.
    pub fn data(&self, studio: &Studio) -> CaptionsSheetData {
        let sources = if self.open {
            sources(studio)
        } else {
            Vec::new()
        };
        let source = self
            .clip
            .as_ref()
            .and_then(|id| sources.iter().position(|source| &source.id == id))
            .map_or(-1, |index| index as i32);
        let labels: Vec<slint::SharedString> = sources
            .into_iter()
            .map(|source| source.label.into())
            .collect();
        CaptionsSheetData {
            open: self.open,
            from_voice: self.from_voice,
            sources: std::rc::Rc::new(slint::VecModel::from(labels)).into(),
            source,
            text: self.text.as_str().into(),
            model: self.model as i32,
            placement: self.placement as i32,
            size: self.size as i32,
            style: self.style as i32,
            templates: std::rc::Rc::new(slint::VecModel::from(
                TEMPLATES
                    .iter()
                    .map(|template| slint::SharedString::from(template.name))
                    .collect::<Vec<_>>(),
            ))
            .into(),
            template: self.template as i32,
            running: self.running,
            progress: self.pace.floor,
            ceiling: self.pace.ceiling,
            step: self.pace.step,
            left: self.pace.left,
            stage: match self.pace.stage() {
                Some(Stage::Loading) => t("captions.loadingTheModel"),
                Some(Stage::Working) => t("captions.transcribing"),
                _ => t("captions.readingTheSound"),
            }
            .into(),
            ready: !installed(&studio.settings.transcribers).is_empty(),
            message: self.message.as_str().into(),
        }
    }
}

/// A clip on the timeline whose voice can be captioned, as the sheet's
/// list shows it.
struct Source {
    id: String,
    label: String,
}

/// The clips on the current timeline that have sound, in the order they
/// play: an audio clip, or a video whose file has an audio stream.
fn sources(studio: &Studio) -> Vec<Source> {
    let project = studio.project();
    let mut clips: Vec<_> = studio
        .timeline()
        .clips
        .iter()
        .filter(|clip| studio.clip_has_sound(clip))
        .map(|clip| {
            let name = project
                .media_by_id(&clip.media_id)
                .map_or(clip.name.as_str(), |media| media.name.as_str());
            (clip.start, clip.id.clone(), name.to_owned())
        })
        .collect();
    clips.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    clips
        .into_iter()
        .map(|(start, id, name)| Source {
            id,
            label: source_label(&name, start),
        })
        .collect()
}

/// A clip as the list names it: its file and where it starts on the
/// timeline, so two clips of one file are told apart.
fn source_label(name: &str, start: f64) -> String {
    let seconds = start.max(0.0).floor() as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{name} · {hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{name} · {minutes}:{seconds:02}")
    }
}

/// One caption as a title clip: its words, when and for how long, and
/// its look.
fn caption_clip(
    text: String,
    start: f64,
    duration: f64,
    look: (f64, f64),
    base: Option<&TextStyle>,
) -> Command {
    Command::AddTextClip {
        track_id: None,
        above: true,
        start,
        style: Some(caption_style(text, look.1, base)),
        duration: Some(duration),
        offset_y: Some(look.0),
    }
}

/// A caption's words in the kept look - family, weight, colours, stroke,
/// shadow, background - at the caption's own size; the bundled face
/// otherwise.
fn caption_style(text: String, font_size: f64, base: Option<&TextStyle>) -> TextStyle {
    match base {
        Some(base) => TextStyle {
            content: text,
            font_size,
            max_width: 0.0,
            max_height: 0.0,
            ..base.clone()
        },
        None => TextStyle {
            content: text,
            font_family: "Hanken Grotesk".to_owned(),
            font_size,
            font_weight: 600.0,
            ..TextStyle::default()
        },
    }
}

/// TetalCut: the captions of transcribed (or scripted) segments, in the
/// sheet's style, starting at `start` on the timeline for a clip playing at
/// `speed`.
///
/// - 0, still: one title per phrase, as Concat makes them.
/// - 1, pop in: the same titles, each entering with a small pop.
/// - 2, word by word: one title per word, from when it is said until the
///   next one is (or a moment after it ends, across a pause), each popping
///   in; a little larger than a phrase.
fn segment_captions(
    segments: &[Segment],
    start: f64,
    speed: f64,
    look: (f64, f64),
    style: usize,
    base: Option<&TextStyle>,
    accent: Option<&TextStyle>,
) -> Vec<Command> {
    let speed = if speed.is_finite() && speed > 0.0 { speed } else { 1.0 };
    let at = |seconds: f64| start + seconds / speed;
    let mut out = Vec::new();
    if style == 2 {
        // each phrase's key word (its longest) in the template's accent
        let mut words: Vec<Word> = Vec::new();
        let mut keys: Vec<bool> = Vec::new();
        for segment in segments {
            let phrase = timed_words(segment);
            let key = phrase
                .iter()
                .enumerate()
                .filter(|(_, word)| word.text.chars().filter(|c| c.is_alphanumeric()).count() >= 4)
                .max_by_key(|(index, word)| (word.text.chars().count(), usize::MAX - index))
                .map(|(index, _)| index);
            keys.extend((0..phrase.len()).map(|index| Some(index) == key));
            words.extend(phrase);
        }
        let big = (look.0, look.1 * WORD_SIZE);
        for (index, word) in words.iter().enumerate() {
            let begin = at(word.start);
            let next = words.get(index + 1).map(|next| at(next.start));
            let spoken_end = at(word.end) + WORD_LINGER;
            let end = match next {
                // the next word takes over, unless a pause comes first
                Some(next) if next - spoken_end < WORD_GAP => next,
                _ => spoken_end,
            };
            let duration = (end - begin).max(WORD_MIN);
            let keyed = accent.filter(|_| keys.get(index).copied().unwrap_or(false));
            let (look, style) = match keyed {
                Some(accent) => ((big.0, big.1 * ACCENT_SIZE), Some(accent)),
                None => (big, base),
            };
            out.push(keyed_caption(word.text.clone(), begin, duration, look, style, WORD_POP));
        }
        return out;
    }
    for segment in segments {
        let text = segment.text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let begin = at(segment.start);
        let duration = ((segment.end - segment.start) / speed).max(0.2);
        if style == 1 {
            out.push(keyed_caption(text, begin, duration, look, base, PHRASE_POP));
        } else {
            out.push(caption_clip(text, begin, duration, look, base));
        }
    }
    out
}

/// A segment's words: whisper's, or, without them (a script, an old
/// transcription), its text's words spread over the segment by length.
fn timed_words(segment: &Segment) -> Vec<Word> {
    if !segment.words.is_empty() {
        return segment.words.clone();
    }
    let words: Vec<&str> = segment.text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
    let span = (segment.end - segment.start).max(0.0);
    let mut at = segment.start;
    words
        .into_iter()
        .map(|word| {
            let share = span * (word.chars().count() + 1) as f64 / total.max(1) as f64;
            let timed = Word {
                start: at,
                end: at + share,
                text: word.to_owned(),
            };
            at += share;
            timed
        })
        .collect()
}

/// How long a phrase's pop takes, seconds.
const PHRASE_POP: f64 = 0.18;
/// How long a word's pop takes, seconds.
const WORD_POP: f64 = 0.12;
/// A word stays this long after it is said when a pause follows.
const WORD_LINGER: f64 = 0.25;
/// A pause shorter than this leaves the word up until the next one.
const WORD_GAP: f64 = 0.6;
/// The shortest a word is on screen.
const WORD_MIN: f64 = 0.12;
/// A word-by-word caption's size over a phrase's.
const WORD_SIZE: f64 = 1.35;
/// A key word's size over the other words'.
const ACCENT_SIZE: f64 = 1.15;

/// TetalCut: a caption template, as CapCut's caption looks - the words'
/// font, fill, outline and box, and the key word's fill and outline when
/// captions go word by word.
struct Template {
    name: &'static str,
    font: &'static str,
    weight: f64,
    color: &'static str,
    stroke: f64,
    stroke_color: &'static str,
    background: &'static str,
    /// The key word's (fill, outline); the outline empty keeps the base's.
    accent: Option<(&'static str, &'static str)>,
}

/// The templates, in the sheet's order; the sheet starts on the first. The
/// last is the kept title look (Concat's own behaviour), with no fields of
/// its own.
const TEMPLATES: [Template; 10] = [
    Template { name: "Clásico", font: "Montserrat", weight: 800.0, color: "#ffffff", stroke: 0.006, stroke_color: "#000000", background: "", accent: Some(("#ffe81a", "")) },
    Template { name: "Amarillo viral", font: "Montserrat", weight: 900.0, color: "#ffe81a", stroke: 0.008, stroke_color: "#000000", background: "", accent: Some(("#ffffff", "")) },
    Template { name: "Caja negra", font: "Montserrat", weight: 800.0, color: "#ffffff", stroke: 0.0, stroke_color: "#000000", background: "#000000cc", accent: Some(("#ffe81a", "")) },
    Template { name: "Caja roja", font: "Montserrat", weight: 900.0, color: "#ffffff", stroke: 0.0, stroke_color: "#000000", background: "#e0182dee", accent: Some(("#ffe81a", "")) },
    Template { name: "Neón", font: "Montserrat", weight: 900.0, color: "#ffffff", stroke: 0.006, stroke_color: "#ff2bd6", background: "", accent: Some(("#2bf0ff", "")) },
    Template { name: "Impacto", font: "Anton", weight: 400.0, color: "#ffffff", stroke: 0.007, stroke_color: "#000000", background: "", accent: Some(("#ff3b30", "")) },
    Template { name: "Bebas", font: "Bebas Neue", weight: 400.0, color: "#ffffff", stroke: 0.006, stroke_color: "#000000", background: "", accent: Some(("#ffe81a", "")) },
    // AJ Media Studio's clients: La Ferre (white, key word yellow on navy),
    // Buffet Imperial (white and orange, Montserrat)
    Template { name: "La Ferre", font: "Montserrat", weight: 900.0, color: "#ffffff", stroke: 0.008, stroke_color: "#0b2a5b", background: "", accent: Some(("#ffd21f", "#0b2a5b")) },
    Template { name: "Buffet Imperial", font: "Montserrat", weight: 800.0, color: "#ffffff", stroke: 0.007, stroke_color: "#e8650a", background: "", accent: Some(("#ff7a00", "#ffffff")) },
    // the look titles were last given (Concat's own captions)
    Template { name: "Título guardado", font: "", weight: 0.0, color: "", stroke: 0.0, stroke_color: "", background: "", accent: None },
];

/// A template's base look and key-word look. The kept title look has no
/// key word of its own.
fn template_styles(index: usize, kept: Option<&TextStyle>) -> (Option<TextStyle>, Option<TextStyle>) {
    let Some(template) = TEMPLATES.get(index).filter(|template| !template.font.is_empty()) else {
        return (kept.cloned(), None);
    };
    let base = TextStyle {
        font_family: template.font.to_owned(),
        font_weight: template.weight,
        color: template.color.to_owned(),
        stroke_width: template.stroke,
        stroke_color: template.stroke_color.to_owned(),
        shadow: template.background.is_empty(),
        background: template.background.to_owned(),
        ..TextStyle::default()
    };
    let accent = template.accent.map(|(fill, outline)| TextStyle {
        color: fill.to_owned(),
        stroke_color: if outline.is_empty() { base.stroke_color.clone() } else { outline.to_owned() },
        ..base.clone()
    });
    (Some(base), accent)
}

/// A caption that pops in: scale from 75 % past full and back, opacity up,
/// over `pop` seconds (at most 40 % of the caption).
fn keyed_caption(
    text: String,
    start: f64,
    duration: f64,
    look: (f64, f64),
    base: Option<&TextStyle>,
    pop: f64,
) -> Command {
    let end = (pop / duration.max(1e-6)).clamp(0.02, 0.4);
    let key = |property, at: f64, value, ease| ClipKey {
        property,
        at,
        value,
        ease,
    };
    Command::AddKeyedTextClip {
        start,
        style: Some(caption_style(text, look.1, base)),
        duration: Some(duration),
        offset_y: Some(look.0),
        keys: vec![
            key(KeyProperty::Scale, 0.0, 0.75, KeyEase::LINEAR),
            key(KeyProperty::Scale, end * 0.65, 1.08, KeyEase::OUT),
            key(KeyProperty::Scale, end, 1.0, KeyEase::IN_OUT),
            key(KeyProperty::Opacity, 0.0, 0.0, KeyEase::LINEAR),
            key(KeyProperty::Opacity, end * 0.6, 1.0, KeyEase::OUT),
        ],
    }
}

/// Longest a caption line gets before it is wrapped: about what two lines
/// of broadcast subtitle hold, and what a reader takes in at a glance.
const CAPTION_CHARS: usize = 42;

/// A script as caption lines, each with how long it stays up: a line's
/// reading time at [`CHARS_PER_SECOND`], held to one second at least so
/// a short word is not a flicker, and seven at most so a long line does
/// not hang. A line break in the script is a break the author asked for;
/// within a paragraph a sentence is a caption, and a long sentence wraps
/// at its words.
fn script_captions(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .flat_map(sentences)
        .flat_map(|sentence| wrap_caption(&sentence))
        .map(|line| {
            let seconds =
                (line.chars().count() as f64 / f64::from(CHARS_PER_SECOND)).clamp(1.0, 7.0);
            (line, seconds)
        })
        .collect()
}

/// A paragraph's sentences. A full stop, question or exclamation mark ends
/// one when it is followed by space or by the end - so "3.5" and "e.g." hold
/// together - and the CJK marks end one on their own.
fn sentences(paragraph: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = paragraph.chars().peekable();
    while let Some(ch) = chars.next() {
        current.push(ch);
        let ends = match ch {
            '。' | '！' | '？' => true,
            '.' | '!' | '?' => chars.peek().is_none_or(|next| next.is_whitespace()),
            _ => false,
        };
        if ends {
            let sentence = current.trim();
            if !sentence.is_empty() {
                out.push(sentence.to_owned());
            }
            current.clear();
        }
    }
    let rest = current.trim();
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

/// A sentence in lines of at most [`CAPTION_CHARS`], broken between words;
/// a word longer than a line, or a run of CJK with no spaces, is broken
/// where it must be.
fn wrap_caption(sentence: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_chars = 0;
    for word in sentence.split_whitespace() {
        let word_chars = word.chars().count();
        if line_chars > 0 && line_chars + 1 + word_chars > CAPTION_CHARS {
            lines.push(std::mem::take(&mut line));
            line_chars = 0;
        }
        if word_chars > CAPTION_CHARS {
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if piece.chars().count() == CAPTION_CHARS {
                    lines.push(std::mem::take(&mut piece));
                }
            }
            line = piece;
            line_chars = line.chars().count();
            continue;
        }
        if line_chars > 0 {
            line.push(' ');
            line_chars += 1;
        }
        line.push_str(word);
        line_chars += word_chars;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clip in the voice list is its file and where it starts, in
    /// minutes and seconds, with hours once there are any.
    #[test]
    fn a_voice_is_named_by_its_file_and_where_it_starts() {
        assert_eq!(source_label("Interview.mp4", 0.0), "Interview.mp4 · 0:00");
        assert_eq!(source_label("Interview.mp4", 72.9), "Interview.mp4 · 1:12");
        assert_eq!(source_label("Talk.wav", 3_725.0), "Talk.wav · 1:02:05");
        assert_eq!(source_label("Talk.wav", -1.0), "Talk.wav · 0:00");
    }

    /// A script becomes one caption per sentence, a hand line break is
    /// kept, a long sentence wraps at its words, and each line is held for
    /// its reading time within one to seven seconds.
    #[test]
    fn a_script_is_cut_into_readable_lines() {
        let lines = script_captions(
            "Hello there. This is version 3.5, mind!\n\nA sentence that runs on for far \
             longer than a caption line has any business running on for. Ok?",
        );
        let text: Vec<&str> = lines.iter().map(|(line, _)| line.as_str()).collect();
        assert_eq!(
            text,
            [
                "Hello there.",
                "This is version 3.5, mind!",
                "A sentence that runs on for far longer",
                "than a caption line has any business",
                "running on for.",
                "Ok?",
            ]
        );
        assert!(
            lines
                .iter()
                .all(|(_, seconds)| (1.0..=7.0).contains(seconds))
        );
        assert_eq!(lines[0].1, 1.0);
        assert!(lines[2].1 > lines[0].1);
        assert!(script_captions("  \n ").is_empty());
        assert_eq!(script_captions("你好。再见！").len(), 2);
    }

    /// TetalCut: word by word makes one popping title per word, each held
    /// until the next is said (or a moment past it across a pause); pop in
    /// keys each phrase; still is Concat's plain title.
    #[test]
    fn caption_styles_pop_and_go_word_by_word() {
        let w = |t: &str, a: f64, b: f64| Word {
            start: a,
            end: b,
            text: t.to_owned(),
        };
        let segments = vec![Segment {
            start: 0.0,
            end: 3.0,
            text: "Hola a todos".to_owned(),
            words: vec![w("Hola", 0.0, 0.4), w("a", 0.5, 0.6), w("todos", 2.0, 2.5)],
        }];
        let look = (0.35, 0.05);
        let words = segment_captions(&segments, 10.0, 1.0, look, 2, None, None);
        assert_eq!(words.len(), 3);
        let spans: Vec<(f64, f64)> = words
            .iter()
            .map(|c| match c {
                Command::AddKeyedTextClip {
                    start,
                    duration,
                    keys,
                    ..
                } => {
                    assert!(keys.iter().any(|k| k.property == KeyProperty::Scale));
                    (*start, duration.unwrap_or(0.0))
                }
                other => panic!("not a keyed caption: {other:?}"),
            })
            .collect();
        // "Hola" until "a" starts; "a" lingers across the pause before "todos"
        assert!((spans[0].0 - 10.0).abs() < 1e-9 && (spans[0].1 - 0.5).abs() < 1e-9);
        assert!((spans[1].1 - (0.1 + WORD_LINGER)).abs() < 1e-9);
        assert!((spans[2].0 - 12.0).abs() < 1e-9);
        let pop = segment_captions(&segments, 0.0, 2.0, look, 1, None, None);
        assert!(matches!(pop.as_slice(), [Command::AddKeyedTextClip { duration: Some(d), .. }] if (*d - 1.5).abs() < 1e-9));
        let still = segment_captions(&segments, 0.0, 1.0, look, 0, None, None);
        assert!(matches!(still.as_slice(), [Command::AddTextClip { .. }]));
        // a script's words are spread by length
        let plain = Segment {
            start: 0.0,
            end: 2.0,
            text: "uno dos".to_owned(),
            words: Vec::new(),
        };
        let spread = timed_words(&plain);
        assert_eq!(spread.len(), 2);
        assert!((spread[1].start - 1.0).abs() < 1e-9 && (spread[1].end - 2.0).abs() < 1e-9);
    }

    /// TetalCut: a template colours every caption, and word by word puts
    /// each phrase's longest word in the accent, a little larger; the kept
    /// look is the title style as it was.
    #[test]
    fn templates_colour_captions_and_accent_the_key_word() {
        let ferre = TEMPLATES.iter().position(|t| t.name == "La Ferre").unwrap();
        let (base, accent) = template_styles(ferre, None);
        let (base, accent) = (base.unwrap(), accent.unwrap());
        assert_eq!((base.color.as_str(), base.stroke_color.as_str()), ("#ffffff", "#0b2a5b"));
        assert_eq!((accent.color.as_str(), accent.stroke_color.as_str()), ("#ffd21f", "#0b2a5b"));
        let kept = TextStyle { color: "#123456".to_owned(), ..TextStyle::default() };
        let (same, none) = template_styles(TEMPLATES.len() - 1, Some(&kept));
        assert_eq!(same.unwrap().color, "#123456");
        assert!(none.is_none());
        let segments = vec![Segment {
            start: 0.0,
            end: 2.0,
            text: "la mejor ferretería".to_owned(),
            words: Vec::new(),
        }];
        let out = segment_captions(&segments, 0.0, 1.0, (0.35, 0.05), 2, Some(&base), Some(&accent));
        let looks: Vec<(String, f64)> = out
            .iter()
            .map(|c| match c {
                Command::AddKeyedTextClip { style: Some(style), .. } => (style.color.clone(), style.font_size),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(looks[0].0, "#ffffff");
        assert_eq!(looks[2].0, "#ffd21f", "the longest word is the key");
        assert!(looks[2].1 > looks[0].1);
    }
}
