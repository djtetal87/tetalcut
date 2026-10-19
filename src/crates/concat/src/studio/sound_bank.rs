// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut's sound bank, as CapCut's Audio ▸ Sounds: music and sound
//! effects by category, heard with a tap and put at the playhead with +.
//!
//! The bank is a folder - on a Mac and an iPhone, Documents/TetalCut
//! Sonidos, where Finder and Files see it - whose subfolders are the
//! categories. TetalCut's own sounds (`crate::sounds`, synthesised, free to
//! use) are written into it the first time; anything else dropped in a
//! subfolder (a DJ's own tracks, a pack of effects) shows up beside them.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use concat_host::freesound::{self, Hit};

use super::*;

/// The file kinds the bank lists.
const AUDIO: [&str; 10] = ["m4a", "mp3", "wav", "aac", "flac", "ogg", "aif", "aiff", "caf", "opus"];
/// The bank's first category, which shows everything.
const ALL: &str = "Todos";
/// Files the user brought in through the bank's import go here.
const MINE: &str = "Mis sonidos";
/// The category pill that searches Freesound.
const SEARCH: &str = "Buscar en Freesound";
/// Where Freesound's sounds are kept once heard or placed.
const FREESOUND: &str = "Freesound";
/// A search result's row is named by this and its index.
const HIT: &str = "fs:";
/// The pack of the effects TikTok and CapCut use most, from Freesound.
const POPULAR: &str = "Populares";
/// Video files the import takes the sound out of.
const VIDEO: [&str; 5] = ["mp4", "mov", "m4v", "webm", "mkv"];

/// The Populares pack: each sound's name in the bank and its Freesound id
/// (free licences only - CC0, or CC BY, credited in the pack's Créditos.txt).
const POPULARES: &[(&str, u64)] = &[
    ("Swish whoosh grande", 237980),
    ("Noise whoosh", 425706),
    ("Disparo con eco", 384252),
    ("Whoosh", 60013),
    ("Obturador Canon", 270435),
    ("Fuwa (viento corto)", 719560),
    ("Click botón", 342200),
    ("Sci-fi whoosh", 274210),
    ("Impacto", 376817),
    ("Scratch de disco", 43404),
    ("Mal presentimiento", 146434),
    ("Glitch", 573189),
    ("Cámara (click)", 170229),
    ("Robot - máquina", 363251),
    ("Subida (build up)", 261415),
    ("Boom", 802074),
    ("Pop", 202230),
    ("Ding", 335908),
    ("Risas", 124028),
    ("Swoosh rápido", 389590),
    ("Whoosh + golpe", 322250),
];

/// Written once the built-in sounds have been put in the folder, so one the
/// user deletes stays deleted.
const SEEDED: &str = ".tetalcut-sonidos-v1";

/// One sound in the bank.
#[derive(Clone, Debug)]
struct Sound {
    category: String,
    name: String,
    path: String,
}

struct Bank {
    open: bool,
    category: String,
    sounds: Vec<Sound>,
    /// What the user has asked to hear, while it is being brought in.
    pending_preview: Option<String>,
    /// Freesound: the last search, its results, whether one is running,
    /// only sounds a paid video may use, and why the last one failed.
    query: String,
    hits: Vec<Hit>,
    searching: bool,
    commercial: bool,
    status: String,
    /// The sound being renamed, and the one a delete waits to be confirmed on.
    renaming: Option<String>,
    deleting: Option<String>,
    /// The Populares pack is being fetched.
    fetching: bool,
    /// A compilation is being cut, or a video's sound taken out.
    working: bool,
}

impl Default for Bank {
    fn default() -> Bank {
        Bank {
            open: false,
            category: String::new(),
            sounds: Vec::new(),
            pending_preview: None,
            query: String::new(),
            hits: Vec::new(),
            searching: false,
            commercial: true,
            status: String::new(),
            renaming: None,
            deleting: None,
            fetching: false,
            working: false,
        }
    }
}

thread_local! {
    static BANK: RefCell<Bank> = RefCell::new(Bank::default());
}

/// Where the bank lives.
fn bank_root(config: &Path) -> PathBuf {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Documents").join("TetalCut Sonidos");
    }
    config.join("sounds")
}

/// The built-in sounds written into the bank, the first time.
fn seed(root: &Path) {
    if root.join(SEEDED).exists() {
        return;
    }
    for (category, name, _, _, bytes) in crate::sounds::BUILT_IN {
        let folder = root.join(category);
        if std::fs::create_dir_all(&folder).is_err() {
            return;
        }
        let path = folder.join(format!("{name}.m4a"));
        if !path.exists() && std::fs::write(&path, bytes).is_err() {
            return;
        }
    }
    let _ = std::fs::write(root.join(SEEDED), b"");
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Every sound in the bank: a subfolder a category (one level below it
/// too, for a pack in a folder of its own), loose files under "Otros".
fn scan(root: &Path) -> Vec<Sound> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let mut files = Vec::new();
            collect(&path, 2, &mut files);
            files.sort();
            for file in files {
                out.push(sound(&name, &file));
            }
        } else if is_audio(&path) {
            out.push(sound("Otros", &path));
        }
    }
    // TetalCut's own categories first, in the bank's order
    let order = [POPULAR, "Música", "Transiciones", "Impactos", "Interfaz", "Divertidos", MINE];
    out.sort_by_key(|sound| {
        order
            .iter()
            .position(|category| *category == sound.category)
            .unwrap_or(order.len())
    });
    out
}

fn collect(folder: &Path, depth: u32, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for path in entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()) {
        if path.is_dir() && depth > 1 {
            collect(&path, depth - 1, files);
        } else if is_audio(&path) && files.len() < 2000 {
            files.push(path);
        }
    }
}

fn sound(category: &str, path: &Path) -> Sound {
    Sound {
        category: category.to_owned(),
        name: path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
    }
}

impl Studio {
    /// The bank opened (read from its folder again) or closed.
    pub fn sounds_open(&mut self, open: bool) {
        if open {
            self.sounds_refresh();
            self.populares_fetch();
        } else {
            self.close_source();
        }
        BANK.with(|bank| bank.borrow_mut().open = open);
    }

    /// Reads the bank's folder again, putting TetalCut's own sounds in it
    /// the first time.
    pub fn sounds_refresh(&mut self) {
        let root = bank_root(&self.host.dirs.config);
        let _ = std::fs::create_dir_all(&root);
        seed(&root);
        let sounds = scan(&root);
        BANK.with(|bank| {
            let mut bank = bank.borrow_mut();
            if bank.category.is_empty() {
                bank.category = "Música".to_owned();
            }
            bank.sounds = sounds;
        });
    }

    /// A category pill.
    pub fn sound_category(&mut self, category: &str) {
        BANK.with(|bank| bank.borrow_mut().category = category.to_owned());
    }

    /// A sound tapped: heard (brought into the project's media first, as
    /// the bin's own preview needs); tapped again, paused.
    pub fn sound_preview(&mut self, path: &str) {
        if path.starts_with(HIT) {
            self.with_hit(path, |studio, file| studio.sound_preview(&file));
            return;
        }
        self.with_sound_media(path, |studio, media_id| studio.preview_media(&media_id, -1));
    }

    /// A sound's +: put at the playhead, on a sound lane.
    pub fn sound_add(&mut self, path: &str) {
        if path.starts_with(HIT) {
            self.with_hit(path, |studio, file| studio.sound_add(&file));
            return;
        }
        self.with_sound_media(path, |studio, media_id| {
            studio.close_source();
            let start = f64::from(studio.playhead.max(0.0));
            if let Some(id) = studio.apply(Command::AddClipAtFirstFree { media_id, start }) {
                studio.selection = vec![id];
            }
            studio.notify(&t("sounds.added"), false);
        });
    }

    /// The Freesound key pasted in, and kept.
    pub fn freesound_key(&mut self, key: &str) {
        let key = key.trim();
        self.prefs.freesound_key = (!key.is_empty()).then(|| key.to_owned());
        self.prefs.save(&self.host.dirs);
        BANK.with(|bank| bank.borrow_mut().status.clear());
        self.populares_fetch();
    }

    /// Only sounds a paid video may use, or every licence.
    pub fn freesound_commercial(&mut self, on: bool) {
        let query = BANK.with(|bank| {
            let mut bank = bank.borrow_mut();
            bank.commercial = on;
            bank.query.clone()
        });
        if !query.is_empty() {
            self.freesound_search(&query);
        }
    }

    /// Searches Freesound on a worker.
    pub fn freesound_search(&mut self, query: &str) {
        let key = self.prefs.freesound_key.clone().unwrap_or_default();
        let commercial = BANK.with(|bank| {
            let mut bank = bank.borrow_mut();
            bank.query = query.trim().to_owned();
            bank.searching = !bank.query.is_empty();
            bank.status.clear();
            bank.commercial
        });
        let query = query.to_owned();
        spawn(
            move || freesound::search(&key, &query, commercial),
            |_, _, _, result| {
                BANK.with(|bank| {
                    let mut bank = bank.borrow_mut();
                    bank.searching = false;
                    match result {
                        Ok(hits) => {
                            bank.status = if hits.is_empty() && !bank.query.is_empty() {
                                t("sounds.noResults")
                            } else {
                                String::new()
                            };
                            bank.hits = hits;
                        }
                        Err(error) => {
                            bank.hits.clear();
                            bank.status = error;
                        }
                    }
                });
            },
        );
    }

    /// Runs `then` with the file of search result `row` ("fs:<index>"),
    /// downloading it into the bank's Freesound folder first.
    fn with_hit(&mut self, row: &str, then: impl FnOnce(&mut Studio, String) + Send + 'static) {
        let Some(hit) = row
            .strip_prefix(HIT)
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| BANK.with(|bank| bank.borrow().hits.get(index).cloned()))
        else {
            return;
        };
        let folder = bank_root(&self.host.dirs.config).join(FREESOUND);
        let done = freesound::path_of(&folder, &hit);
        if done.exists() {
            then(self, done.to_string_lossy().into_owned());
            return;
        }
        BANK.with(|bank| bank.borrow_mut().pending_preview = Some(row.to_owned()));
        spawn(
            move || freesound::download(&folder, &hit),
            move |studio, _, _, result| {
                BANK.with(|bank| bank.borrow_mut().pending_preview = None);
                match result {
                    Ok(path) => {
                        studio.sounds_refresh();
                        then(studio, path.to_string_lossy().into_owned());
                    }
                    Err(error) => studio.notify(&error, true),
                }
            },
        );
    }

    /// The user's own files brought into the bank's "Mis sonidos".
    pub fn sounds_import(&mut self) {
        let root = bank_root(&self.host.dirs.config).join(MINE);
        let kinds: &'static [&'static str] = &[
            "m4a", "mp3", "wav", "aac", "flac", "ogg", "aif", "aiff", "caf", "opus", "mp4", "mov",
            "m4v", "webm", "mkv",
        ];
        crate::host::pick_files_then(
            t("sounds.importTitle"),
            Some((t("sounds.audioFiles"), kinds)),
            move |studio, _, _, paths| {
                BANK.with(|bank| bank.borrow_mut().working = true);
                spawn(
                    move || {
                        let _ = std::fs::create_dir_all(&root);
                        let mut added = 0;
                        for path in paths {
                            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned())
                            else {
                                continue;
                            };
                            let video = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(|e| VIDEO.contains(&e.to_ascii_lowercase().as_str()));
                            let done = if video {
                                // a video: its sound, as a sound file
                                concat_host::sound_files::extract(&path, &root.join(format!("{stem}.m4a")))
                                    .is_ok()
                            } else {
                                path.file_name()
                                    .is_some_and(|name| std::fs::copy(&path, root.join(name)).is_ok())
                            };
                            if done {
                                added += 1;
                            }
                        }
                        added
                    },
                    |studio, _, _, added: usize| {
                        BANK.with(|bank| bank.borrow_mut().working = false);
                        studio.sounds_refresh();
                        studio.sound_category(MINE);
                        studio.notify(&tf("sounds.imported", &[&added]), false);
                    },
                );
            },
        );
    }

    /// A compilation (or a video of one) cut at its silences into a
    /// category of its own, one sound a file.
    pub fn sounds_split(&mut self) {
        let root = bank_root(&self.host.dirs.config);
        let kinds: &'static [&'static str] = &[
            "m4a", "mp3", "wav", "aac", "flac", "ogg", "aif", "aiff", "caf", "opus", "mp4", "mov",
            "m4v", "webm", "mkv",
        ];
        crate::host::pick_files_then(
            t("sounds.splitTitle"),
            Some((t("sounds.audioFiles"), kinds)),
            move |studio, _, _, paths| {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Compilado".into());
                let category = format!("Compilado {stem}");
                let folder = root.join(&category);
                BANK.with(|bank| bank.borrow_mut().working = true);
                studio.notify(&t("sounds.splitting"), false);
                spawn(
                    move || concat_host::sound_files::split(&path, &folder, &stem),
                    move |studio, _, _, result| {
                        BANK.with(|bank| bank.borrow_mut().working = false);
                        match result {
                            Ok(files) => {
                                studio.sounds_refresh();
                                studio.sound_category(&category);
                                studio.notify(&tf("sounds.splitDone", &[&files.len()]), false);
                            }
                            Err(error) => studio.notify(&error, true),
                        }
                    },
                );
            },
        );
    }

    /// The rename field opened on a sound, or closed (`path` empty).
    pub fn sound_rename_begin(&mut self, path: &str) {
        BANK.with(|bank| {
            let mut bank = bank.borrow_mut();
            bank.renaming = (!path.is_empty()).then(|| path.to_owned());
            bank.deleting = None;
        });
    }

    /// A sound given a new name (its file renamed beside it).
    pub fn sound_rename(&mut self, path: &str, name: &str) {
        BANK.with(|bank| bank.borrow_mut().renaming = None);
        let clean: String = name
            .trim()
            .chars()
            .map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c })
            .take(80)
            .collect();
        if clean.is_empty() || path.starts_with(HIT) {
            return;
        }
        let from = Path::new(path);
        let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let to = from.with_file_name(format!("{clean}.{ext}"));
        if to.exists() {
            self.notify(&t("sounds.nameTaken"), true);
            return;
        }
        if let Err(error) = std::fs::rename(from, &to) {
            self.notify(&error.to_string(), true);
        }
        self.sounds_refresh();
    }

    /// A sound's trash: the first tap asks, the second deletes the file.
    pub fn sound_delete(&mut self, path: &str) {
        if path.starts_with(HIT) {
            return;
        }
        let confirmed = BANK.with(|bank| {
            let mut bank = bank.borrow_mut();
            bank.renaming = None;
            if bank.deleting.as_deref() == Some(path) {
                bank.deleting = None;
                true
            } else {
                bank.deleting = Some(path.to_owned());
                false
            }
        });
        if !confirmed {
            return;
        }
        if let Err(error) = std::fs::remove_file(path) {
            self.notify(&error.to_string(), true);
        }
        self.sounds_refresh();
    }

    /// The Populares pack fetched from Freesound, the sounds it lacks (once
    /// a key is kept; quietly otherwise).
    pub fn populares_fetch(&mut self) {
        let Some(key) = self.prefs.freesound_key.clone().filter(|key| !key.is_empty()) else {
            return;
        };
        let folder = bank_root(&self.host.dirs.config).join(POPULAR);
        let missing: Vec<(String, u64)> = POPULARES
            .iter()
            .filter(|(name, _)| !folder.join(format!("{name}.mp3")).exists())
            .map(|(name, id)| ((*name).to_owned(), *id))
            .collect();
        if missing.is_empty() || BANK.with(|bank| bank.borrow().fetching) {
            return;
        }
        BANK.with(|bank| bank.borrow_mut().fetching = true);
        spawn(
            move || {
                let mut credits = Vec::new();
                let mut failed = None;
                for (name, id) in missing {
                    match freesound::sound(&key, id)
                        .and_then(|hit| freesound::download_to(&folder.join(format!("{name}.mp3")), &hit).map(|()| hit))
                    {
                        Ok(hit) => credits.push(format!(
                            "{name}: \"{}\" de {} ({}) - https://freesound.org/s/{}/",
                            hit.name, hit.author, hit.license, hit.id
                        )),
                        Err(error) => failed = Some(error),
                    }
                }
                if !credits.is_empty() {
                    let file = folder.join("Créditos.txt");
                    let mut text = std::fs::read_to_string(&file).unwrap_or_else(|_| {
                        "Sonidos de Freesound (freesound.org). Los CC BY piden nombrar a su autor.\n".into()
                    });
                    for line in credits {
                        text.push_str(&line);
                        text.push('\n');
                    }
                    let _ = std::fs::write(file, text);
                }
                failed
            },
            |studio, _, _, failed: Option<String>| {
                BANK.with(|bank| bank.borrow_mut().fetching = false);
                studio.sounds_refresh();
                if let Some(error) = failed {
                    studio.notify(&error, true);
                }
            },
        );
    }

    /// Runs `then` with the media id of the file at `path` in the open
    /// project, adding the file to the project's media first if it is not
    /// there yet.
    fn with_sound_media(
        &mut self,
        path: &str,
        then: impl FnOnce(&mut Studio, String) + Send + 'static,
    ) {
        if self.session.is_none() {
            return;
        }
        if let Some(id) = self.project().media.iter().find(|item| item.path == path).map(|item| item.id.clone()) {
            then(self, id);
            return;
        }
        let path = path.to_owned();
        BANK.with(|bank| bank.borrow_mut().pending_preview = Some(path.clone()));
        spawn_in_project(
            {
                let path = path.clone();
                move || media::probe(&path)
            },
            move |studio, _, _, result| {
                BANK.with(|bank| bank.borrow_mut().pending_preview = None);
                match result {
                    Ok(summary) => {
                        studio.apply(Command::AddMedia {
                            item: summary.to_new_media(),
                        });
                        if let Some(id) = studio
                            .project()
                            .media
                            .iter()
                            .find(|item| item.path == path)
                            .map(|item| item.id.clone())
                        {
                            then(studio, id);
                        }
                    }
                    Err(error) => studio.notify(&error, true),
                }
            },
        );
    }

    /// The bank as the window shows it.
    pub fn publish_sounds(&self, editor: &Editor, models: &Models) {
        let playing = self
            .source
            .as_ref()
            .filter(|source| source.playing)
            .and_then(|source| self.project().media_by_id(&source.media_id))
            .map(|item| item.path.clone());
        BANK.with(|bank| {
            let bank = bank.borrow();
            editor.set_sounds_shown(bank.open);
            let mut categories: Vec<SharedString> = vec![ALL.into(), POPULAR.into(), SEARCH.into()];
            for sound in &bank.sounds {
                if !categories.iter().any(|c| c.as_str() == sound.category) {
                    categories.push(sound.category.as_str().into());
                }
            }
            sync(&models.sound_categories, categories);
            editor.set_sound_category(bank.category.as_str().into());
            let has_key = self.prefs.freesound_key.as_ref().is_some_and(|k| !k.is_empty());
            editor.set_sounds_note(
                if bank.working {
                    t("sounds.working")
                } else if bank.fetching {
                    t("sounds.fetchingPopular")
                } else if bank.category == POPULAR && !has_key {
                    t("sounds.popularNeedsKey")
                } else {
                    String::new()
                }
                .into(),
            );
            let searching = bank.category == SEARCH;
            editor.set_freesound_mode(searching);
            editor.set_freesound_key_set(self.prefs.freesound_key.as_ref().is_some_and(|k| !k.is_empty()));
            editor.set_freesound_commercial(bank.commercial);
            editor.set_freesound_status(
                if bank.searching { t("sounds.searching") } else { bank.status.clone() }.into(),
            );
            if searching {
                let rows: Vec<SoundRowData> = bank
                    .hits
                    .iter()
                    .enumerate()
                    .map(|(index, hit)| {
                        let row = format!("{}{}", HIT, index);
                        SoundRowData {
                            name: hit.name.as_str().into(),
                            category: format!("{} · {} · {:.1} s", hit.license, hit.author, hit.seconds)
                                .into(),
                            playing: playing.as_deref().is_some_and(|path| {
                                path.ends_with(&format!("({}).mp3", hit.id))
                            }),
                            loading: bank.pending_preview.as_deref() == Some(row.as_str()),
                            editable: false,
                            renaming: false,
                            deleting: false,
                            path: row.into(),
                        }
                    })
                    .collect();
                sync(&models.sounds, rows);
                return;
            }
            let rows: Vec<SoundRowData> = bank
                .sounds
                .iter()
                .filter(|sound| bank.category == ALL || sound.category == bank.category)
                .map(|sound| SoundRowData {
                    name: sound.name.as_str().into(),
                    category: sound.category.as_str().into(),
                    path: sound.path.as_str().into(),
                    playing: playing.as_deref() == Some(sound.path.as_str()),
                    loading: bank.pending_preview.as_deref() == Some(sound.path.as_str()),
                    editable: true,
                    renaming: bank.renaming.as_deref() == Some(sound.path.as_str()),
                    deleting: bank.deleting.as_deref() == Some(sound.path.as_str()),
                })
                .collect();
            sync(&models.sounds, rows);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bank_is_seeded_once_and_lists_by_category() {
        let root = std::env::temp_dir().join(format!("tetalcut-bank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        seed(&root);
        let sounds = scan(&root);
        assert_eq!(sounds.len(), crate::sounds::BUILT_IN.len());
        assert_eq!(sounds[0].category, "Música");
        // a deleted sound stays deleted; a dropped-in folder is a category
        let whoosh = root.join("Transiciones").join("Whoosh.m4a");
        std::fs::remove_file(&whoosh).unwrap();
        std::fs::create_dir_all(root.join("Reggaeton")).unwrap();
        std::fs::write(root.join("Reggaeton").join("Tema.mp3"), b"x").unwrap();
        std::fs::write(root.join("Reggaeton").join("notas.txt"), b"x").unwrap();
        seed(&root);
        let sounds = scan(&root);
        assert!(!whoosh.exists());
        assert!(sounds.iter().any(|s| s.category == "Reggaeton" && s.name == "Tema"));
        assert!(!sounds.iter().any(|s| s.name == "notas"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
