use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::types::{Message, MessageContent};

const IMAGE_REF: &str = "corex-image:";
const LEFT_OUT_PREFIX: &str = "[Earlier image left out of this request to save context. It is saved at ";
const LEFT_OUT_SUFFIX: &str = "; call read_image on that path to look at it again.]";
const MISSING_NOTE: &str = "[An image from earlier in this session is no longer stored on disk.]";

const FORMATS: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

pub(crate) fn images_dir(session_file: &Path) -> Option<PathBuf> {
    Some(session_file.parent()?.join("images").join(session_file.file_stem()?))
}

fn split_data_uri(url: &str) -> Option<(&'static str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    let ext = FORMATS.iter().find(|(m, _)| m.eq_ignore_ascii_case(mime))?.1;
    Some((ext, data))
}

fn mime_for(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?;
    FORMATS.iter().find(|(_, e)| *e == ext).map(|(m, _)| *m)
}

fn store(url: &str, dir: &Path) -> Option<String> {
    let (ext, data) = split_data_uri(url)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let digest = Sha256::digest(&bytes);
    let name = format!("{}.{ext}", digest.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>());
    let path = dir.join(&name);
    let stored = path.is_file()
        || (crate::secure_fs::ensure_private_dir(dir).is_ok() && crate::secure_fs::write_private_atomic(&path, &bytes).is_ok());
    stored.then_some(name)
}

// An image that cannot be written stays inline, so a full disk never loses it.
pub(crate) fn externalize(messages: &mut [Message], dir: &Path) {
    for message in messages {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else { continue };
        for image in parts.iter_mut().filter_map(|p| p.image_url.as_mut()) {
            if let Some(name) = store(&image.url, dir) {
                image.url = format!("{IMAGE_REF}{name}");
            }
        }
    }
}

pub fn keep_recent_images(messages: &mut [Message], keep: usize, dir: Option<&Path>) {
    if keep == 0 {
        return;
    }
    let mut seen = 0;
    for message in messages.iter_mut().rev() {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else { continue };
        for part in parts.iter_mut().rev() {
            let Some(url) = part.image_url.as_ref().map(|i| i.url.clone()) else { continue };
            seen += 1;
            if seen <= keep {
                continue;
            }
            let saved = dir.and_then(|d| store(&url, d).map(|name| d.join(name)));
            part.part_type = "text".to_string();
            part.image_url = None;
            part.text = Some(match saved {
                Some(path) => format!("{LEFT_OUT_PREFIX}{}{LEFT_OUT_SUFFIX}", path.display()),
                None => "[Earlier image left out of this request to save context.]".to_string(),
            });
        }
    }
}

pub fn restore_left_out_images(messages: &mut [Message]) {
    for message in messages {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else { continue };
        for part in parts.iter_mut().filter(|p| p.image_url.is_none()) {
            let Some(path) = part
                .text
                .as_deref()
                .and_then(|t| t.strip_prefix(LEFT_OUT_PREFIX))
                .and_then(|t| t.strip_suffix(LEFT_OUT_SUFFIX))
                .map(PathBuf::from)
            else {
                continue;
            };
            let Some(mime) = path.file_name().and_then(|n| n.to_str()).and_then(mime_for) else { continue };
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            *part = crate::types::ContentPart::image_url(format!("data:{mime};base64,{data}"));
        }
    }
}

pub fn is_session_image(path: &Path) -> bool {
    is_image_under(path, &[crate::session::Session::sessions_dir(), crate::session::Session::fleet_sessions_dir()])
}

fn is_image_under(path: &Path, session_dirs: &[PathBuf]) -> bool {
    let Ok(target) = path.canonicalize() else { return false };
    if mime_for(&target.to_string_lossy()).is_none() || !target.is_file() {
        return false;
    }
    session_dirs
        .iter()
        .filter_map(|dir| dir.join("images").canonicalize().ok())
        .any(|root| target.starts_with(root))
}

fn referenced(messages: &[Message]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|m| match &m.content {
            Some(MessageContent::Parts(parts)) => Some(parts),
            _ => None,
        })
        .flatten()
        .filter_map(|p| p.image_url.as_ref()?.url.strip_prefix(IMAGE_REF))
}

// Run on load, before `restore`: saves can finish out of order, so pruning there could delete an
// image a newer save still points at. The age guard covers another process writing the session.
pub(crate) fn prune_unreferenced(lists: &[&[Message]], dir: &Path, min_age: std::time::Duration) -> usize {
    let used: std::collections::HashSet<&str> = lists.iter().flat_map(|l| referenced(l)).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if used.contains(name.as_str()) || mime_for(&name).is_none() {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= min_age);
        if old && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub(crate) fn restore(messages: &mut [Message], dir: &Path) {
    for message in messages {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else { continue };
        for part in parts.iter_mut() {
            let Some(name) = part.image_url.as_ref().and_then(|i| i.url.strip_prefix(IMAGE_REF)).map(str::to_string) else {
                continue;
            };
            let safe = !name.contains(['/', '\\']) && !name.contains("..");
            let inline = mime_for(&name)
                .filter(|_| safe)
                .and_then(|mime| std::fs::read(dir.join(&name)).ok().map(|bytes| (mime, bytes)))
                .map(|(mime, bytes)| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
            match (inline, part.image_url.as_mut()) {
                (Some(url), Some(image)) => image.url = url,
                _ => {
                    part.part_type = "text".to_string();
                    part.text = Some(MISSING_NOTE.to_string());
                    part.image_url = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ContentPart;

    fn with_image(url: &str) -> Message {
        let mut message = Message::user("look".to_string());
        message.content = Some(MessageContent::Parts(vec![ContentPart::text("look"), ContentPart::image_url(url)]));
        message
    }

    fn url_of(message: &Message) -> Option<String> {
        match &message.content {
            Some(MessageContent::Parts(parts)) => parts.iter().find_map(|p| p.image_url.as_ref().map(|i| i.url.clone())),
            _ => None,
        }
    }

    #[test]
    fn images_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("corex_session_images_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let original = "data:image/png;base64,iVBORw0KGgo=";
        let mut messages = vec![with_image(original), with_image(original), with_image("data:image/svg+xml;base64,PHN2Zz4=")];

        externalize(&mut messages, &dir);
        let stored = url_of(&messages[0]).unwrap();
        assert!(stored.starts_with(IMAGE_REF) && stored.ends_with(".png"), "{stored}");
        assert_eq!(url_of(&messages[1]), Some(stored.clone()), "the same image is stored once");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(url_of(&messages[2]).unwrap().starts_with("data:image/svg"), "unknown formats stay inline");

        restore(&mut messages, &dir);
        assert_eq!(url_of(&messages[0]).as_deref(), Some(original));
        assert_eq!(url_of(&messages[1]).as_deref(), Some(original));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_becomes_a_note() {
        let dir = std::env::temp_dir().join(format!("corex_session_images_missing_{}", std::process::id()));
        let mut messages = vec![with_image(&format!("{IMAGE_REF}0000.png")), with_image(&format!("{IMAGE_REF}../escape.png"))];
        restore(&mut messages, &dir);
        for message in &messages {
            assert_eq!(url_of(message), None);
            let Some(MessageContent::Parts(parts)) = &message.content else { panic!("parts expected") };
            assert!(parts.iter().any(|p| p.text.as_deref() == Some(MISSING_NOTE)));
        }
    }

    #[test]
    fn only_the_newest_images_are_sent() {
        let dir = std::env::temp_dir().join(format!("corex_recent_images_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let urls = ["data:image/png;base64,AAAA", "data:image/png;base64,BBBB", "data:image/png;base64,CCCC"];
        let mut messages: Vec<Message> = urls.iter().map(|u| with_image(u)).collect();

        keep_recent_images(&mut messages, 0, Some(&dir));
        assert!(messages.iter().all(|m| url_of(m).is_some()), "0 keeps every image");

        keep_recent_images(&mut messages, 2, Some(&dir));
        assert_eq!(url_of(&messages[0]), None);
        assert_eq!(url_of(&messages[1]).as_deref(), Some(urls[1]));
        assert_eq!(url_of(&messages[2]).as_deref(), Some(urls[2]));
        let Some(MessageContent::Parts(parts)) = &messages[0].content else { panic!("parts expected") };
        let note = parts.iter().find_map(|p| p.text.clone().filter(|t| t.contains("read_image"))).unwrap();
        let path = note.split("saved at ").nth(1).and_then(|r| r.split(';').next()).unwrap();
        assert!(std::path::Path::new(path).is_file(), "{note}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_image_files_inside_the_session_images_count() {
        let base = std::env::temp_dir().join(format!("corex_session_image_check_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let images = base.join("images").join("abc");
        std::fs::create_dir_all(&images).unwrap();
        std::fs::write(images.join("0011.png"), b"png").unwrap();
        std::fs::write(images.join("notes.txt"), b"text").unwrap();
        std::fs::write(base.join("settings.png"), b"png").unwrap();
        let dirs = [base.clone()];

        assert!(is_image_under(&images.join("0011.png"), &dirs));
        assert!(!is_image_under(&images.join("notes.txt"), &dirs), "only images");
        assert!(!is_image_under(&base.join("settings.png"), &dirs), "only inside images/");
        assert!(!is_image_under(&images.join("../../settings.png"), &dirs));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unreferenced_old_images_are_pruned() {
        let dir = std::env::temp_dir().join(format!("corex_prune_images_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut kept = vec![with_image("data:image/png;base64,AAAA")];
        let mut dropped = vec![with_image("data:image/png;base64,BBBB")];
        externalize(&mut kept, &dir);
        externalize(&mut dropped, &dir);
        std::fs::write(dir.join("notes.txt"), "x").unwrap();

        assert_eq!(prune_unreferenced(&[&kept], &dir, std::time::Duration::from_secs(600)), 0, "fresh files stay");
        assert_eq!(prune_unreferenced(&[&kept], &dir, std::time::Duration::ZERO), 1);
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left.iter().any(|n| url_of(&kept[0]).unwrap().ends_with(n.as_str())) && left.contains(&"notes.txt".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn left_out_images_come_back_after_a_request() {
        let dir = std::env::temp_dir().join(format!("corex_left_out_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let original = "data:image/png;base64,iVBORw0KGgo=";
        let mut messages = vec![with_image(original), with_image("data:image/png;base64,AAAA")];
        keep_recent_images(&mut messages, 1, Some(&dir));
        assert_eq!(url_of(&messages[0]), None);
        restore_left_out_images(&mut messages);
        assert_eq!(url_of(&messages[0]).as_deref(), Some(original));
        assert_eq!(url_of(&messages[1]).as_deref(), Some("data:image/png;base64,AAAA"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
