// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut's additions to the studio, as CapCut has them:
//!
//! - Movement: a clip's entrance, exit and movement while on screen,
//!   written as its keys ([`concat_project::motion`]), on one clip or every
//!   title of the timeline at once.
//! - Auto colour and auto light: the Adjust panel's knobs set from a frame
//!   of the clip ([`concat_project::auto_grade`]).
//! - The keyframe button: a key on every picture property at the playhead,
//!   or every key there taken off.
//! - Speed curves: CapCut's ready-made curves, and a curve to draw.

use std::cell::RefCell;

use concat_host::templates::{SlotFill, TemplateInfo};
use concat_project::model::{KeyEase, KeyProperty, SpeedPoint};
use concat_project::{auto_grade, motion, speed};

use super::*;

/// The picture properties a movement or the keyframe button touches.
const MOVED: [KeyProperty; 5] = [
    KeyProperty::Scale,
    KeyProperty::OffsetX,
    KeyProperty::OffsetY,
    KeyProperty::Rotation,
    KeyProperty::Opacity,
];

/// The most points a drawn speed curve takes.
const MAX_SPEED_POINTS: usize = 10;

thread_local! {
    /// The movement last chosen for each clip this session, by clip id:
    /// (entrance, exit, loop) rows. The keys are what play; this is only
    /// what the panel shows as chosen.
    static MOTIONS: RefCell<HashMap<String, [usize; 3]>> = RefCell::new(HashMap::new());
    /// The template gallery as last read from disk, and the model the
    /// window shows it through (handed over once; see `templates_model`).
    static TEMPLATES: RefCell<Vec<TemplateInfo>> = const { RefCell::new(Vec::new()) };
    static TEMPLATE_CARDS: Rc<VecModel<TemplateCardData>> = Rc::new(VecModel::default());
}

/// The gallery's model, for the window to hold.
pub fn templates_model() -> Rc<VecModel<TemplateCardData>> {
    TEMPLATE_CARDS.with(Rc::clone)
}

impl Studio {
    // ── movement ─────────────────────────────────────────────────────────

    /// The selected clip's movement rows: entrance, exit, loop.
    pub fn motion_choice(&self) -> [usize; 3] {
        self.sole_selection()
            .and_then(|id| MOTIONS.with(|motions| motions.borrow().get(&id).copied()))
            .unwrap_or([0; 3])
    }

    /// One of the selected clip's movement rows changed: `kind` 0 entrance,
    /// 1 exit, 2 loop. The clip's movement keys are written afresh.
    pub fn motion_set(&mut self, kind: i32, index: i32) {
        let Some(id) = self.sole_selection() else {
            return;
        };
        let mut choice = self.motion_choice();
        let limits = [motion::ENTRANCES.len(), motion::EXITS.len(), motion::LOOPS.len()];
        let Some(slot) = usize::try_from(kind).ok().filter(|kind| *kind < 3) else {
            return;
        };
        choice[slot] = usize::try_from(index).unwrap_or(0).min(limits[slot] - 1);
        let Some(commands) = self.clip(&id).map(|clip| motion_commands(clip, choice)) else {
            return;
        };
        MOTIONS.with(|motions| motions.borrow_mut().insert(id, choice));
        self.flush_commit();
        self.apply(Command::Batch { commands });
    }

    /// The selected clip's movement given to every title on the timeline
    /// (captions included), each over its own length and place.
    pub fn motion_apply_all(&mut self) {
        let choice = self.motion_choice();
        let titles: Vec<Clip> = self
            .timeline()
            .clips
            .iter()
            .filter(|clip| clip.kind == model::ClipKind::Text)
            .map(|clip| Clip::clone(clip))
            .collect();
        if titles.is_empty() {
            self.notify(&t("motion.noTitles"), true);
            return;
        }
        let mut commands = Vec::new();
        MOTIONS.with(|motions| {
            let mut motions = motions.borrow_mut();
            for clip in &titles {
                commands.extend(motion_commands(clip, choice));
                motions.insert(clip.id.clone(), choice);
            }
        });
        self.flush_commit();
        self.apply(Command::Batch { commands });
        self.notify(&tf("motion.appliedToTitles", &[&titles.len()]), false);
    }

    // ── auto colour and light ────────────────────────────────────────────

    /// Sets the selected clip's Adjust knobs from a frame of it: `kind` 0
    /// light, 1 colour, 2 both.
    pub fn adjust_auto(&mut self, kind: i32) {
        let Some(clip) = self.sole_selection().and_then(|id| self.clip(&id)).cloned() else {
            return;
        };
        if !clip.kind.is_visual() {
            return;
        }
        let Some(path) = self
            .project()
            .media_by_id(&clip.media_id)
            .map(|media| media.path.clone())
        else {
            return;
        };
        // the frame under the playhead, or the clip's middle
        let playhead = f64::from(self.playhead);
        let local = if (clip.start..=clip.start + clip.duration).contains(&playhead) {
            playhead - clip.start
        } else {
            clip.duration / 2.0
        };
        let seconds = if clip.kind == model::ClipKind::Image {
            0.0
        } else {
            clip.source_start + local * clip.speed
        };
        let frame = match media::still_at(&path, seconds, 480) {
            Ok(frame) => frame,
            Err(error) => {
                self.notify(&error, true);
                return;
            }
        };
        let pixels = eight_bit(frame.pixels(), frame.width(), frame.height());
        let Some(levels) = auto_grade::measure(&pixels) else {
            return;
        };
        let mut knobs = Vec::new();
        if kind != 1 {
            knobs.extend(auto_grade::auto_light(&levels));
        }
        if kind != 0 {
            knobs.extend(auto_grade::auto_color(&levels));
        }
        for (key, value) in knobs {
            self.adjust_set(key, value as f32);
        }
        self.clip_commit();
        self.notify(
            &t(match kind {
                0 => "adjust.autoLightDone",
                1 => "adjust.autoColorDone",
                _ => "adjust.autoDone",
            }),
            false,
        );
    }

    // ── the keyframe button ──────────────────────────────────────────────

    /// Whether the selected clip has a key on any picture property at the
    /// playhead: the button's filled diamond.
    pub fn keyed_here(&self) -> bool {
        self.key_point()
            .is_some_and(|(clip, at)| MOVED.iter().any(|&p| clip.key_at(p, at).is_some()))
    }

    /// A key on every picture property at the playhead, holding what each
    /// is worth there - or, where there are keys already, all of them off.
    /// Afterwards, moving the clip in Edit at another moment keys it there:
    /// CapCut's keyframes.
    pub fn key_all_toggle(&mut self) {
        let Some((clip, at)) = self.key_point() else {
            self.notify(&t("keyframes.parkInsideClip"), true);
            return;
        };
        let clip_id = clip.id.clone();
        let here = MOVED.iter().any(|&p| clip.key_at(p, at).is_some());
        let commands: Vec<Command> = MOVED
            .iter()
            .filter_map(|&property| {
                if here {
                    clip.key_at(property, at).map(|_| Command::ClearClipKey {
                        clip_id: clip_id.clone(),
                        property,
                        at,
                    })
                } else {
                    let ease = clip
                        .keys_on(property)
                        .rfind(|key| key.at < at)
                        .map_or(KeyEase::IN_OUT, |key| key.ease);
                    Some(Command::SetClipKey {
                        clip_id: clip_id.clone(),
                        property,
                        at,
                        value: clip.value_at(property, at),
                        ease,
                    })
                }
            })
            .collect();
        if commands.is_empty() {
            return;
        }
        self.flush_commit();
        self.apply(Command::Batch { commands });
    }

    // ── speed curves ─────────────────────────────────────────────────────

    /// The selected clip's speed preset row, -1 for a drawn curve.
    pub fn speed_preset_current(&self) -> i32 {
        self.sole_selection()
            .and_then(|id| self.clip(&id))
            .and_then(|clip| speed::preset_of(clip.speed_curve.as_deref()))
            .map_or(-1, |row| row as i32)
    }

    /// A ready-made curve on the selected clip; Normal is one times speed.
    pub fn speed_preset(&mut self, index: i32) {
        let Some(clip_id) = self.sole_selection() else {
            return;
        };
        let command = match speed::preset(usize::try_from(index).unwrap_or(0)) {
            Some(curve) => Command::SetClipSpeedCurve {
                clip_id,
                curve: Some(curve),
            },
            None => Command::SetClipSpeed { clip_id, speed: 1.0 },
        };
        self.flush_commit();
        self.apply(command);
    }

    /// The selected clip's speed curve as the curve editor draws it: one
    /// curve, its points at height by speed (0.1x to 10x, 1x in the middle).
    pub fn speed_curve_rows(&self) -> (CurveData, Vec<CurvePointData>) {
        let points = self
            .sole_selection()
            .and_then(|id| self.clip(&id))
            .map(|clip| match &clip.speed_curve {
                Some(curve) => curve.clone(),
                None => vec![
                    SpeedPoint { at: 0.0, speed: clip.speed },
                    SpeedPoint { at: 1.0, speed: clip.speed },
                ],
            })
            .unwrap_or_default();
        let mut path = String::new();
        for (index, point) in points.iter().enumerate() {
            let verb = if index == 0 { 'M' } else { 'L' };
            let y = 1.0 - speed::height_of(point.speed);
            path.push_str(&format!("{verb} {:.4} {y:.4} ", point.at.clamp(0.0, 1.0)));
        }
        let data = CurveData {
            entry: -2,
            key: "speed".into(),
            label: t("common.speed").into(),
            group: SharedString::new(),
            points: ModelRc::default(),
            path: path.trim_end().into(),
        };
        let rows = points
            .iter()
            .enumerate()
            .map(|(index, point)| CurvePointData {
                index: index as i32,
                x: point.at as f32,
                y: speed::height_of(point.speed) as f32,
            })
            .collect();
        (data, rows)
    }

    /// A point of the selected clip's speed curve moved, or put down when
    /// `index` is negative. The two ends stay at the ends.
    pub fn speed_curve_point(&mut self, index: i32, x: f32, y: f32) {
        self.edit_speed_curve(|points| {
            let speed = speed::speed_of(f64::from(y));
            let x = f64::from(x).clamp(0.0, 1.0);
            match usize::try_from(index) {
                Ok(index) if index < points.len() => {
                    let last = points.len() - 1;
                    let at = if index == 0 {
                        0.0
                    } else if index == last {
                        1.0
                    } else {
                        // held between its neighbours
                        let before = points[index - 1].at + 0.02;
                        let after = points[index + 1].at - 0.02;
                        x.clamp(before.min(after), after.max(before))
                    };
                    points[index] = SpeedPoint { at, speed };
                }
                Ok(_) => {}
                Err(_) => {
                    if points.len() < MAX_SPEED_POINTS && x > 0.01 && x < 0.99 {
                        points.push(SpeedPoint { at: x, speed });
                        points.sort_by(|a, b| a.at.total_cmp(&b.at));
                    }
                }
            }
        });
    }

    /// A point of the selected clip's speed curve taken off; not an end.
    pub fn speed_curve_remove(&mut self, index: i32) {
        self.edit_speed_curve(|points| {
            if let Ok(index) = usize::try_from(index)
                && index > 0
                && index + 1 < points.len()
            {
                points.remove(index);
            }
        });
    }

    /// The selected clip's speed curve on the echo, a flat one at its rate
    /// when it has none, handed to `edit`; committed after.
    fn edit_speed_curve(&mut self, edit: impl FnOnce(&mut Vec<SpeedPoint>)) {
        let Some(id) = self.sole_selection() else {
            return;
        };
        self.begin_echo();
        let Some(clip) = self.echo_clip_mut(&id) else {
            return;
        };
        if !matches!(clip.kind, model::ClipKind::Video | model::ClipKind::Audio) {
            return;
        }
        let rate = clip.speed;
        let mut points = clip.speed_curve.clone().unwrap_or_else(|| {
            vec![
                SpeedPoint { at: 0.0, speed: rate },
                SpeedPoint { at: 1.0, speed: rate },
            ]
        });
        edit(&mut points);
        clip.speed_curve = Some(points);
        self.clip_commit();
    }
}

impl Studio {
    // ── templates ────────────────────────────────────────────────────────

    /// Reads the template gallery from disk again.
    pub fn templates_refresh(&mut self) {
        let list = templates::list(&self.host.dirs.config);
        let cards: Vec<TemplateCardData> = list
            .iter()
            .map(|info| TemplateCardData {
                path: info.path.as_str().into(),
                name: info.name.as_str().into(),
                slots: info.slots.len() as i32,
                poster: if info.has_poster {
                    slint::Image::load_from_path(&std::path::Path::new(&info.path).join("poster.jpg"))
                        .unwrap_or_default()
                } else {
                    slint::Image::default()
                },
            })
            .collect();
        TEMPLATES.with(|templates| *templates.borrow_mut() = list);
        TEMPLATE_CARDS.with(|model| sync(model, cards));
    }

    /// Whether the selected clip's file is a template slot.
    pub fn placeholder_here(&self) -> bool {
        self.sole_selection()
            .and_then(|id| self.clip(&id))
            .and_then(|clip| self.project().media_by_id(&clip.media_id))
            .is_some_and(|item| item.placeholder)
    }

    /// The selected clip's file made a template slot, or ordinary again.
    pub fn placeholder_toggle(&mut self) {
        if let Some(id) = self.sole_selection() {
            self.placeholder_toggle_on(&id);
        }
    }

    /// Clip `id`'s file made a template slot, or ordinary again.
    pub fn placeholder_toggle_on(&mut self, id: &str) {
        let Some(item) = self
            .clip(id)
            .filter(|clip| clip.kind.is_visual())
            .and_then(|clip| self.project().media_by_id(&clip.media_id))
        else {
            return;
        };
        let (media_id, placeholder) = (item.id.clone(), !item.placeholder);
        self.flush_commit();
        self.apply(Command::SetMediaPlaceholder {
            media_id,
            placeholder,
        });
        self.notify(
            &t(if placeholder {
                "template.markedReplaceable"
            } else {
                "template.unmarkedReplaceable"
            }),
            false,
        );
    }

    /// The open project saved as a template, and the gallery read again.
    pub fn template_save(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let document = session.document();
        let settings = session.settings();
        let path = session.path().to_owned();
        let config = self.host.dirs.config.clone();
        let base = self.project_name.clone();
        spawn(
            move || {
                // a free name: "Oferta", "Oferta 2", …
                let mut name = base.clone();
                let mut n = 1;
                while templates::templates_dir(&config)
                    .join(projects::folder_name(&name))
                    .join("template.json")
                    .exists()
                {
                    n += 1;
                    name = format!("{base} {n}");
                }
                templates::save(&config, &document, &settings, &path, &name)
            },
            |studio, _, _, result| match result {
                Ok(info) => {
                    studio.templates_refresh();
                    studio.notify(&tf("template.saved", &[&info.name, &info.slots.len()]), false);
                }
                Err(error) => studio.notify(&error, true),
            },
        );
    }

    /// A template taken out of the gallery for good.
    pub fn template_delete(&mut self, path: &str) {
        match templates::delete(&self.host.dirs.config, path) {
            Ok(()) => self.templates_refresh(),
            Err(error) => self.notify(&error, true),
        }
    }

    /// A template used: its clips asked for, in order, then a new project
    /// made from it with them in its slots, and opened.
    pub fn template_use(&mut self, path: &str) {
        let Some(info) = TEMPLATES.with(|templates| {
            templates.borrow().iter().find(|info| info.path == path).cloned()
        }) else {
            self.templates_refresh();
            return;
        };
        if info.slots.is_empty() {
            self.template_fill(info, Vec::new());
            return;
        }
        self.notify(&tf("template.chooseClips", &[&info.slots.len()]), false);
        crate::host::pick_files_then(
            tf("template.chooseClips", &[&info.slots.len()]),
            None,
            move |studio, _, _, paths| studio.template_fill(info, paths),
        );
    }

    /// Makes the project from template `info` with `paths` in its slots,
    /// in the order the slots first play.
    fn template_fill(&mut self, info: TemplateInfo, paths: Vec<std::path::PathBuf>) {
        if paths.len() < info.slots.len() {
            self.notify(
                &tf("template.needsClips", &[&info.slots.len(), &paths.len()]),
                true,
            );
            return;
        }
        let location = crate::panes::start::project_location();
        spawn(
            move || -> Result<ProjectInfo, String> {
                let mut fills = Vec::new();
                for (slot, path) in info.slots.iter().zip(&paths) {
                    let path = path.to_string_lossy().into_owned();
                    let summary = media::probe(&path)?;
                    fills.push(SlotFill {
                        media_id: slot.media_id.clone(),
                        item: summary.to_new_media(),
                    });
                }
                let mut name = info.name.clone();
                let mut n = 1;
                while std::path::Path::new(&location)
                    .join(projects::folder_name(&name))
                    .exists()
                {
                    n += 1;
                    name = format!("{} {n}", info.name);
                }
                templates::instantiate(&info.path, &location, &name, fills)
            },
            |studio, _, _, result| {
                match result.and_then(|project| studio.open_project(project)) {
                    Ok(()) => studio.notify(&t("template.ready"), false),
                    Err(error) => studio.notify(&error, true),
                }
            },
        );
    }
}

/// The commands that give `clip` this movement: its picture properties'
/// keys taken off, and the movement's put on.
fn motion_commands(clip: &Clip, choice: [usize; 3]) -> Vec<Command> {
    let rest = motion::Rest {
        scale: clip.scale,
        offset_x: clip.offset_x,
        offset_y: clip.offset_y,
        rotation: clip.rotation,
        opacity: clip.opacity,
    };
    let keys = motion::motion_keys(choice[0], choice[1], choice[2], clip.duration, &rest);
    let mut commands: Vec<Command> = MOVED
        .iter()
        .filter(|&&property| clip.is_keyed(property))
        .map(|&property| Command::ClearClipKeys {
            clip_id: clip.id.clone(),
            property,
        })
        .collect();
    commands.extend(keys.into_iter().map(|key| Command::SetClipKey {
        clip_id: clip.id.clone(),
        property: key.property,
        at: key.at,
        value: key.value,
        ease: key.ease,
    }));
    commands
}

/// RGBA at eight bits a channel: a deep frame's high bytes.
fn eight_bit(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let count = width as usize * height as usize;
    if pixels.len() >= count * 8 {
        pixels.chunks_exact(2).map(|pair| pair[1]).collect()
    } else {
        pixels.to_vec()
    }
}
