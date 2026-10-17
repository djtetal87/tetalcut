// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut on an iPhone: what the window's crate asks a phone for and an
//! iPhone answers through UIKit.
//!
//! - [`pick`]: the media picker behind the Media page's Import. A sheet
//!   offers Photos (PHPickerViewController, no permission needed) or Files
//!   (UIDocumentPickerViewController); what is chosen is copied into the
//!   app's `Documents/TetalCut Media`, where it stays for the projects that
//!   use it, and those paths come back.
//! - [`publish`]: a finished export saved to Photos (add-only access), and
//!   the copy in the app's folder removed.
//!
//! Classes are reached by name and messaged dynamically: objc2's generated
//! PhotosUI bindings cover the Mac only.

#[cfg(target_os = "ios")]
mod imp;

#[cfg(target_os = "ios")]
pub use imp::{PHOTOS_FOLDER, pick, publish};
