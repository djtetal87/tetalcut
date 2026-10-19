// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut: Freesound (freesound.org) from the sound bank - a text search
//! with the user's own API key, and a result's preview (an MP3) downloaded
//! into the bank so it can be heard, placed and used again offline.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Largest search reply read.
const MAX_JSON: u64 = 4 << 20;
/// Largest preview read.
const MAX_SOUND: u64 = 40 << 20;

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Freesound's id for the sound.
    pub id: u64,
    /// Its title.
    pub name: String,
    /// Its length.
    pub seconds: f64,
    /// Short licence name: "CC0", "CC BY", "CC BY-NC", …
    pub license: String,
    /// Who uploaded it (CC BY asks for their name in the credits).
    pub author: String,
    /// The preview's address (an MP3 on Freesound's servers).
    pub preview: String,
}

impl Hit {
    /// Whether the licence allows use in a client's (paid) video.
    pub fn commercial(&self) -> bool {
        !self.license.contains("NC")
    }
}

fn short_license(url: &str) -> String {
    let url = url.to_ascii_lowercase();
    if url.contains("publicdomain/zero") || url.contains("creative commons 0") {
        "CC0".into()
    } else if url.contains("by-nc") || url.contains("noncommercial") {
        "CC BY-NC".into()
    } else if url.contains("sampling+") {
        "Sampling+".into()
    } else if url.contains("/by/") || url.contains("attribution") {
        "CC BY".into()
    } else {
        "?".into()
    }
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();
    let response = match agent.get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => {
            return Err("Freesound no aceptó la clave: revísala".into());
        }
        Err(ureq::Error::Status(code, _)) => return Err(format!("Freesound respondió {code}")),
        Err(_) => return Err("No hay conexión con Freesound".into()),
    };
    let mut body = Vec::new();
    response
        .into_reader()
        .take(limit)
        .read_to_end(&mut body)
        .map_err(|error| format!("Freesound: {error}"))?;
    Ok(body)
}

/// Reads a search reply.
pub fn parse(body: &[u8]) -> Result<Vec<Hit>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Freesound envió una respuesta ilegible".to_string())?;
    if let Some(detail) = value.get("detail").and_then(Value::as_str) {
        return Err(format!("Freesound: {detail}"));
    }
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| "Freesound no envió resultados".to_string())?;
    Ok(results
        .iter()
        .filter_map(|row| {
            let previews = row.get("previews")?;
            let preview = previews
                .get("preview-hq-mp3")
                .or_else(|| previews.get("preview-lq-mp3"))
                .and_then(Value::as_str)?
                .to_string();
            if !preview.starts_with("https://") {
                return None;
            }
            Some(Hit {
                id: row.get("id").and_then(Value::as_u64)?,
                name: row.get("name").and_then(Value::as_str).unwrap_or("").chars().take(120).collect(),
                seconds: row
                    .get("duration")
                    .and_then(Value::as_f64)
                    .filter(|seconds| seconds.is_finite())
                    .unwrap_or(0.0),
                license: short_license(row.get("license").and_then(Value::as_str).unwrap_or("")),
                author: row.get("username").and_then(Value::as_str).unwrap_or("").chars().take(60).collect(),
                preview,
            })
        })
        .collect())
}

/// Searches Freesound for `query` with the API `key`; `commercial` keeps
/// only sounds whose licence allows use in a paid video (CC0 and CC BY).
pub fn search(key: &str, query: &str, commercial: bool) -> Result<Vec<Hit>, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Pega tu clave de Freesound para buscar".into());
    }
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let filter = if commercial {
        "&filter=license:(%22Creative+Commons+0%22+OR+%22Attribution%22)"
    } else {
        ""
    };
    let url = format!(
        "https://freesound.org/apiv2/search/text/?query={}&page_size=40&fields=id,name,duration,license,username,previews{filter}&token={}",
        encode(query),
        encode(key)
    );
    parse(&get(&url, MAX_JSON)?)
}

/// One sound by its Freesound id, with the API `key`.
pub fn sound(key: &str, id: u64) -> Result<Hit, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Pega tu clave de Freesound".into());
    }
    let url = format!(
        "https://freesound.org/apiv2/sounds/{id}/?fields=id,name,duration,license,username,previews&token={}",
        encode(key)
    );
    let body = get(&url, MAX_JSON)?;
    // the same shape as a search result, one of them
    let mut wrapped = b"{\"results\":[".to_vec();
    wrapped.extend_from_slice(&body);
    wrapped.extend_from_slice(b"]}");
    parse(&wrapped)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("Freesound no tiene el sonido {id}"))
}

/// `hit`'s preview downloaded to exactly `path` (once).
pub fn download_to(path: &Path, hit: &Hit) -> Result<(), String> {
    if !(hit.preview.starts_with("https://") && hit.preview.contains("freesound.org/")) {
        return Err("no es un enlace de Freesound".into());
    }
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > 0) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = get(&hit.preview, MAX_SOUND)?;
    if bytes.is_empty() {
        return Err("Freesound envió un archivo vacío".into());
    }
    let part = path.with_extension("part");
    std::fs::write(&part, &bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&part, path).map_err(|error| error.to_string())
}

/// A name safe as a file name everywhere.
fn safe(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '_' })
        .take(80)
        .collect();
    let name = name.trim().trim_matches('.').to_string();
    if name.is_empty() { "sonido".into() } else { name }
}

/// Where `hit` is kept in `folder`, downloaded or not.
pub fn path_of(folder: &Path, hit: &Hit) -> PathBuf {
    folder.join(format!("{} ({}).mp3", safe(&hit.name), hit.id))
}

/// `hit`'s preview downloaded into `folder` (once); its path.
pub fn download(folder: &Path, hit: &Hit) -> Result<PathBuf, String> {
    if !(hit.preview.starts_with("https://") && hit.preview.contains("freesound.org/")) {
        return Err("no es un enlace de Freesound".into());
    }
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let path = path_of(folder, hit);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 0) {
        return Ok(path);
    }
    let bytes = get(&hit.preview, MAX_SOUND)?;
    if bytes.is_empty() {
        return Err("Freesound envió un archivo vacío".into());
    }
    let part = folder.join(format!("{}.part", hit.id));
    std::fs::write(&part, &bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&part, &path).map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reads_into_hits_with_short_licences() {
        let body = br#"{"count":2,"results":[
            {"id":1,"name":"Whoosh","duration":1.5,"license":"http://creativecommons.org/publicdomain/zero/1.0/","username":"a","previews":{"preview-hq-mp3":"https://cdn.freesound.org/previews/0/1_hq.mp3"}},
            {"id":2,"name":"Risa","duration":3.0,"license":"https://creativecommons.org/licenses/by-nc/4.0/","username":"b","previews":{"preview-lq-mp3":"https://cdn.freesound.org/previews/0/2_lq.mp3"}},
            {"id":3,"name":"bad","previews":{"preview-hq-mp3":"http://evil/x.mp3"}}
        ]}"#;
        let hits = parse(body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].license, "CC0");
        assert!(hits[0].commercial());
        assert_eq!(hits[1].license, "CC BY-NC");
        assert!(!hits[1].commercial());
        assert!(parse(br#"{"detail":"Invalid token"}"#).unwrap_err().contains("Invalid token"));
    }

    #[test]
    fn names_are_safe_and_queries_encoded() {
        assert_eq!(safe("a/b:c"), "a_b_c");
        assert_eq!(encode("caja registradora"), "caja%20registradora");
        let hit = Hit {
            id: 7,
            name: "Pop!".into(),
            seconds: 1.0,
            license: "CC0".into(),
            author: String::new(),
            preview: String::new(),
        };
        assert!(path_of(Path::new("/x"), &hit).ends_with("Pop_ (7).mp3"));
    }
}
