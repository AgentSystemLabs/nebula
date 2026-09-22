//! Files dragged onto a prompt box, kept where the agent can find them.
//!
//! A terminal hands a drop over as a paste of the file's path, escaped for
//! a shell: Ghostty sends a macOS screenshot's floating thumbnail as
//!
//! ```text
//! /var/folders/…/T/TemporaryItems/NSIRD_screencaptureui_LySI4r/Screenshot\ 2026-09-21\ at\ 11.13.58 PM.png
//! ```
//!
//! and that text used to go to the agent as it came, which lost the
//! screenshot twice over. The file behind the thumbnail is macOS's to
//! delete, and it does, soon after the drop, long before a prompt typed
//! around it is sent. And the name holds a U+202F NARROW NO-BREAK SPACE
//! before `PM`, which the agent types back as a plain space, so its `cp`
//! or `Read` misses a file that is still there.
//!
//! So a paste into a box bound for an agent that is nothing but dropped
//! paths is staged the moment it lands: every file that is about to
//! vanish (anything under a `TemporaryItems` folder), and every image
//! whose path the agent can't type back as-is, is copied into the DATA
//! DIR's `attachments/` under a plain ASCII name, and the box gets that
//! path in place of the one dropped. Any other path — a source file in the
//! repo, a Desktop image with a tidy name — is left exactly as pasted: the
//! agent should work on the file itself, not a copy of it.
//!
//! Copies older than [`KEEP`] are pruned whenever another one is made.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Directory under the DATA DIR the copies go to.
pub const DIR: &str = "attachments";

/// How long a copy outlives its drop. A prompt is sent within minutes; a
/// week covers a session resumed days later that looks at it again.
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A paste longer than this is not a drop: no terminal sends a wall of
/// paths, and the check stays off everyday pastes.
const MAX_DROP_LEN: usize = 16 * 1024;

/// The largest copy compared byte for byte with a new drop of the same
/// name; a bigger one just gets a numbered copy beside it.
const MAX_COMPARE: u64 = 64 * 1024 * 1024;

/// Extensions of files the agent is dropped to look at.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "tif", "tiff", "bmp",
];

/// The default home of the copies: `attachments/` in the DATA DIR.
pub fn default_dir() -> PathBuf {
    nebula_core::paths::data_dir().join(DIR)
}

/// A drop, staged.
#[derive(Debug, PartialEq, Eq)]
pub struct Staged {
    /// The paste as it should land in the box.
    pub text: String,
    /// Files that had to be copied and couldn't be, each with why. Their
    /// paths stay in `text` as they were pasted.
    pub failed: Vec<(String, String)>,
}

/// `text` staged for an agent-bound box, copying into `dir` — or None when
/// the paste is not a drop (anything but whitespace-separated paths of
/// files that exist), or is one with nothing to copy: the caller pastes it
/// as it came.
pub fn stage(text: &str, dir: &Path) -> Option<Staged> {
    let body = text.trim();
    if body.is_empty() || body.len() > MAX_DROP_LEN || !body.starts_with(['/', '~', '\'', '"', 'f'])
    {
        return None;
    }
    let tokens = split_words(body)?;
    let files: Vec<PathBuf> = tokens
        .iter()
        .map(|(_, word)| dropped_file(word))
        .collect::<Option<_>>()?;
    if !files.iter().any(|f| needs_copy(f)) {
        return None;
    }
    prune(dir);
    let mut failed = Vec::new();
    let mut staged = String::new();
    for ((raw, _), file) in tokens.iter().zip(&files) {
        if !staged.is_empty() {
            staged.push(' ');
        }
        if !needs_copy(file) {
            staged.push_str(raw);
            continue;
        }
        match copy_in(file, dir) {
            Ok(copy) => staged.push_str(&quoted(&copy.to_string_lossy())),
            Err(err) => {
                staged.push_str(raw);
                failed.push((file_name(file), err.to_string()));
            }
        }
    }
    // Whatever whitespace framed the paste still frames it.
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    Some(Staged {
        text: format!("{lead}{staged}{trail}"),
        failed,
    })
}

/// The paste split the way a shell reads words — `\` escapes the next
/// character, `'…'` and `"…"` quote — on ASCII whitespace alone: the
/// U+202F in a screenshot's name is part of the name. Each word comes back
/// raw, as pasted, and unescaped. None for a dangling quote or escape.
fn split_words(s: &str) -> Option<Vec<(&str, String)>> {
    let mut words = Vec::new();
    let mut start = None;
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => word.push(chars.next()?.1),
            (Some(_), c) => word.push(c),
            (None, c) if c.is_ascii_whitespace() => {
                if let Some(from) = start.take() {
                    words.push((&s[from..i], std::mem::take(&mut word)));
                }
                continue;
            }
            (None, '\'' | '"') => quote = Some(c),
            (None, c) => word.push(c),
        }
        start.get_or_insert(i);
    }
    if quote.is_some() {
        return None;
    }
    if let Some(from) = start {
        words.push((&s[from..], word));
    }
    Some(words)
}

/// The file one dropped word names — an absolute path, `~/…`, or a
/// `file://` URL — when it is a file that exists.
fn dropped_file(word: &str) -> Option<PathBuf> {
    let path = if let Some(url) = word.strip_prefix("file://") {
        PathBuf::from(percent_decode(
            url.strip_prefix("localhost").unwrap_or(url),
        )?)
    } else if let Some(rest) = word.strip_prefix("~/") {
        nebula_core::env::home_dir()?.join(rest)
    } else {
        PathBuf::from(word)
    };
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// `%XX` escapes decoded, for a `file://` URL's path.
fn percent_decode(s: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        if b == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(b);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

/// Does `file` have to be copied before the agent can use it: it sits in
/// a folder macOS clears behind a drag (a screenshot's floating
/// thumbnail), or it is an image whose path the agent won't type back
/// right.
fn needs_copy(file: &Path) -> bool {
    let vanishes = file
        .components()
        .any(|c| c == Component::Normal("TemporaryItems".as_ref()));
    vanishes || (is_image(file) && !plain(&file.to_string_lossy()))
}

fn is_image(file: &Path) -> bool {
    file.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|i| e.eq_ignore_ascii_case(i)))
}

/// Can `path` be written into a prompt as it is — nothing a shell would
/// want escaped, nothing outside ASCII?
fn plain(path: &str) -> bool {
    path.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@,:=~%".contains(c))
}

/// `path` as it goes into the prompt: bare when plain, single-quoted when
/// not — the DATA DIR is under `Application Support` on macOS.
fn quoted(path: &str) -> String {
    if plain(path) {
        path.to_string()
    } else {
        format!("'{path}'")
    }
}

/// Copy `file` into `dir` under its name made plain, and return the copy.
/// A copy already there with the same bytes is reused, so dropping one
/// screenshot twice leaves one file; a different file with that name gets
/// a numbered one beside it.
fn copy_in(file: &Path, dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let (stem, ext) = plain_name(&file_name(file));
    let len = fs::metadata(file)?.len();
    for n in 1.. {
        let dest = match n {
            1 => dir.join(format!("{stem}{ext}")),
            n => dir.join(format!("{stem}-{n}{ext}")),
        };
        match fs::metadata(&dest) {
            Ok(there) if there.len() == len && same_bytes(file, &dest) => return touched(dest),
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // A clone on APFS, so a big drop costs no time; it keeps
                // the source's mtime, which `touched` resets.
                fs::copy(file, &dest)?;
                return touched(dest);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("an unbounded range never runs out")
}

/// Do two files of one length hold the same bytes? Past [`MAX_COMPARE`]
/// they are taken to differ rather than read into memory.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let small = fs::metadata(a).is_ok_and(|m| m.len() <= MAX_COMPARE);
    small && matches!((fs::read(a), fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

/// `dest` with its mtime set to now, so a copy reused today isn't pruned
/// tomorrow for the day it was first made.
fn touched(dest: PathBuf) -> io::Result<PathBuf> {
    fs::File::options()
        .write(true)
        .open(&dest)?
        .set_modified(SystemTime::now())?;
    Ok(dest)
}

fn file_name(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A file name made plain, as `(stem, ".ext")`: every run of anything but
/// ASCII letters, digits, `.`, `_` and `-` becomes one `-`, so
/// `Screenshot 2026-09-21 at 11.13.58 PM.png` (U+202F before `PM`) is
/// `Screenshot-2026-09-21-at-11.13.58-PM.png`.
fn plain_name(name: &str) -> (String, String) {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    let stem = plain_run(stem);
    let stem = if stem.is_empty() {
        "dropped".to_string()
    } else {
        stem
    };
    let ext = ext.map(plain_run).filter(|e| !e.is_empty());
    (stem, ext.map(|e| format!(".{e}")).unwrap_or_default())
}

fn plain_run(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Delete the copies in `dir` older than [`KEEP`]. Best effort: a copy
/// that can't be read or removed is left for the next time.
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > KEEP);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell-escaped path, the way Ghostty pastes a drop.
    fn escaped(path: &Path) -> String {
        let mut out = String::new();
        for c in path.to_string_lossy().chars() {
            if " ()'\"\\".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    /// A macOS screenshot behind its floating thumbnail, in `root`.
    fn thumbnail(root: &Path) -> PathBuf {
        let dir = root.join("T/TemporaryItems/NSIRD_screencaptureui_LySI4r");
        fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("Screenshot 2026-09-21 at 11.13.58\u{202f}PM.png");
        fs::write(&shot, b"\x89PNG pixels").unwrap();
        shot
    }

    #[test]
    fn a_dropped_screenshot_is_copied_under_a_plain_name_before_macos_deletes_it() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());

        let staged = stage(&escaped(&shot), &attachments).expect("a drop");

        let copy = attachments.join("Screenshot-2026-09-21-at-11.13.58-PM.png");
        assert_eq!(staged.text, quoted(&copy.to_string_lossy()));
        assert!(staged.failed.is_empty());
        // macOS clears the thumbnail's file; the copy is what the agent reads.
        fs::remove_file(&shot).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"\x89PNG pixels");
    }

    #[test]
    fn the_whitespace_around_a_drop_survives_staging() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());

        let staged = stage(&format!("{} ", escaped(&shot)), &attachments).unwrap();

        assert!(staged.text.ends_with(".png "), "{:?}", staged.text);
    }

    #[test]
    fn an_image_with_an_awkward_name_outside_temp_is_copied_too() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let desktop = root.path().join("Desktop");
        fs::create_dir_all(&desktop).unwrap();
        let shot = desktop.join("Screenshot 2026-09-21 at 9.19.29\u{202f}PM.png");
        fs::write(&shot, b"png").unwrap();

        let staged = stage(&escaped(&shot), &attachments).unwrap();

        assert!(
            staged
                .text
                .contains("Screenshot-2026-09-21-at-9.19.29-PM.png"),
            "{:?}",
            staged.text
        );
        // The original stays where it was: a copy, never a move.
        assert!(shot.is_file());
    }

    #[test]
    fn a_file_the_agent_can_use_where_it_lies_is_pasted_as_it_came() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let source = root.path().join("src main.rs");
        fs::write(&source, "fn main() {}").unwrap();
        let tidy = root.path().join("logo.png");
        fs::write(&tidy, b"png").unwrap();

        // A source file, even one with a space: the agent works on it, not a copy.
        assert_eq!(stage(&escaped(&source), &attachments), None);
        // An image whose path is already plain.
        if plain(&tidy.to_string_lossy()) {
            assert_eq!(stage(&escaped(&tidy), &attachments), None);
        }
        assert!(!attachments.exists(), "nothing copied");
    }

    #[test]
    fn several_dropped_files_stage_one_by_one() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());
        let source = root.path().join("notes.txt");
        fs::write(&source, "notes").unwrap();

        let paste = format!("{} {}", escaped(&source), escaped(&shot));
        let staged = stage(&paste, &attachments).unwrap();

        let copy = attachments.join("Screenshot-2026-09-21-at-11.13.58-PM.png");
        assert_eq!(
            staged.text,
            format!("{} {}", escaped(&source), quoted(&copy.to_string_lossy()))
        );
    }

    #[test]
    fn text_that_is_not_only_paths_of_existing_files_is_no_drop() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());

        for paste in [
            "fix the login redirect".to_string(),
            format!("{} what is this", escaped(&shot)),
            root.path().join("gone.png").display().to_string(),
            "/".to_string(),
            "'/unterminated".to_string(),
            String::new(),
        ] {
            assert_eq!(stage(&paste, &attachments), None, "{paste:?}");
        }
    }

    #[test]
    fn quoted_and_file_url_drops_are_read_too() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());
        let path = shot.to_string_lossy().into_owned();
        let url = format!(
            "file://{}",
            path.replace(' ', "%20").replace('\u{202f}', "%E2%80%AF")
        );

        for paste in [format!("'{path}'"), format!("\"{path}\""), url] {
            let staged = stage(&paste, &attachments).expect(&paste);
            assert!(
                staged.text.contains("-PM.png"),
                "{paste:?} → {:?}",
                staged.text
            );
        }
        // Three drops of the same screenshot, one copy.
        assert_eq!(fs::read_dir(&attachments).unwrap().count(), 1);
    }

    #[test]
    fn a_different_file_under_a_taken_name_gets_a_numbered_copy() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        let shot = thumbnail(root.path());
        stage(&escaped(&shot), &attachments).unwrap();
        fs::write(&shot, b"other pixels").unwrap();

        let staged = stage(&escaped(&shot), &attachments).unwrap();

        assert!(staged
            .text
            .contains("Screenshot-2026-09-21-at-11.13.58-PM-2.png"));
    }

    #[test]
    fn copies_older_than_a_week_are_pruned_by_the_next_drop() {
        let root = tempfile::tempdir().unwrap();
        let attachments = root.path().join("attachments");
        fs::create_dir_all(&attachments).unwrap();
        let old = attachments.join("old.png");
        fs::write(&old, b"old").unwrap();
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - KEEP - Duration::from_secs(60))
            .unwrap();
        let fresh = attachments.join("fresh.png");
        fs::write(&fresh, b"fresh").unwrap();

        stage(&escaped(&thumbnail(root.path())), &attachments).unwrap();

        assert!(!old.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn plain_name_keeps_the_readable_parts() {
        let name = |n: &str| {
            let (stem, ext) = plain_name(n);
            stem + &ext
        };
        assert_eq!(
            name("Screenshot 2026-09-21 at 11.13.58\u{202f}PM.png"),
            "Screenshot-2026-09-21-at-11.13.58-PM.png"
        );
        assert_eq!(name("shot (1).png"), "shot-1.png");
        assert_eq!(name("截图.png"), "dropped.png");
        assert_eq!(name(".env"), ".env");
        assert_eq!(name("README"), "README");
    }
}
