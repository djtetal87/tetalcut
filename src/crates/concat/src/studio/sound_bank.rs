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

use super::*;

/// The file kinds the bank lists.
const AUDIO: [&str; 10] = ["m4a", "mp3", "wav", "aac", "flac", "ogg", "aif", "aiff", "caf", "opus"];
/// The bank's first category, which shows everything.
const ALL: &str = "Todos";
/// Files the user brought in through the bank's import go here.
const MINE: &str = "Mis sonidos";
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

#[derive(Default)]
struct Bank {
    open: bool,
    category: String,
    sounds: Vec<Sound>,
    /// What the user has asked to hear, while it is being brought in.
    pending_preview: Option<String>,
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
    let order = ["Música", "Transiciones", "Impactos", "Interfaz", "Divertidos", MINE];
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
        self.with_sound_media(path, |studio, media_id| studio.preview_media(&media_id, -1));
    }

    /// A sound's +: put at the playhead, on a sound lane.
    pub fn sound_add(&mut self, path: &str) {
        self.with_sound_media(path, |studio, media_id| {
            studio.close_source();
            let start = f64::from(studio.playhead.max(0.0));
            if let Some(id) = studio.apply(Command::AddClipAtFirstFree { media_id, start }) {
                studio.selection = vec![id];
            }
            studio.notify(&t("sounds.added"), false);
        });
    }

    /// The user's own files brought into the bank's "Mis sonidos".
    pub fn sounds_import(&mut self) {
        let root = bank_root(&self.host.dirs.config).join(MINE);
        crate::host::pick_files_then(
            t("sounds.importTitle"),
            Some((t("sounds.audioFiles"), &AUDIO[..])),
            move |studio, _, _, paths| {
                let _ = std::fs::create_dir_all(&root);
                let mut copied = 0;
                for path in paths {
                    let Some(name) = path.file_name() else { continue };
                    if std::fs::copy(&path, root.join(name)).is_ok() {
                        copied += 1;
                    }
                }
                studio.sounds_refresh();
                studio.sound_category(MINE);
                studio.notify(&tf("sounds.imported", &[&copied]), false);
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
            let mut categories: Vec<SharedString> = vec![ALL.into()];
            for sound in &bank.sounds {
                if !categories.iter().any(|c| c.as_str() == sound.category) {
                    categories.push(sound.category.as_str().into());
                }
            }
            sync(&models.sound_categories, categories);
            editor.set_sound_category(bank.category.as_str().into());
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
