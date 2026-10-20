//! agy omits prompt text from hooks; its transcript holds the user request.

use tokio::io::{AsyncReadExt, AsyncSeekExt};

const TAIL_BYTES: u64 = 256 * 1024;

/// The file agy names `transcriptPath`, under its conversation's own
/// directory: `…/brain/<conversationId>/.system_generated/logs/transcript.jsonl`.
const TRANSCRIPT_FILE: &str = "transcript.jsonl";

/// Best effort only: a missing, changing or unfamiliar transcript must
/// never hold up agy's hook reply.
pub async fn read(path: Option<&str>, conversation: Option<&str>) -> Option<String> {
    let path = std::path::Path::new(path?);
    if !is_transcript_of(path, conversation?) {
        return None;
    }
    tokio::time::timeout(std::time::Duration::from_millis(50), async {
        // Nonblocking, so a FIFO or device swapped in for the file cannot
        // park the open on a blocking-pool thread past the timeout; no
        // symlink at the leaf. What was opened is what gets checked.
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
            .open(path)
            .await
            .ok()?;
        let meta = file.metadata().await.ok()?;
        if !meta.is_file() {
            return None;
        }
        let len = meta.len();
        let start = len.saturating_sub(TAIL_BYTES);
        file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
        let mut bytes = Vec::new();
        file.take(TAIL_BYTES).read_to_end(&mut bytes).await.ok()?;
        // The first line may start halfway through a JSON record.
        let bytes = if start > 0 {
            &bytes[bytes.iter().position(|b| *b == b'\n')? + 1..]
        } else {
            &bytes[..]
        };
        latest_request(bytes)
    })
    .await
    .ok()
    .flatten()
}

/// Only the transcript of the conversation the hook reports for: an
/// absolute `transcript.jsonl` with that id as one of its directories.
/// The hook token is daemon-wide, so this keeps a payload from steering
/// the read at any other file.
fn is_transcript_of(path: &std::path::Path, conversation: &str) -> bool {
    let conversation = conversation.trim();
    !conversation.is_empty()
        && path.is_absolute()
        && path.file_name() == Some(std::ffi::OsStr::new(TRANSCRIPT_FILE))
        && path
            .parent()
            .is_some_and(|dir| dir.iter().any(|part| part == conversation))
        && !path.iter().any(|part| part == std::ffi::OsStr::new(".."))
}

fn latest_request(bytes: &[u8]) -> Option<String> {
    for line in bytes.split(|b| *b == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if record["type"] != "USER_INPUT" || record["source"] != "USER_EXPLICIT" {
            continue;
        }
        let content = record["content"].as_str()?;
        let request = if let Some((_, rest)) = content.split_once("<USER_REQUEST>") {
            rest.split_once("</USER_REQUEST>")?.0
        } else {
            content
        };
        return Some(request.trim().to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    const TRANSCRIPT: &[u8] = include_bytes!("fixtures/antigravity-transcript.jsonl");

    #[tokio::test]
    async fn reads_real_agy_transcript_without_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("c1").join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join(TRANSCRIPT_FILE);
        std::fs::write(&path, TRANSCRIPT).unwrap();
        let at = path.to_str();
        assert_eq!(
            read(at, Some("c1")).await.as_deref(),
            Some("Reply with exactly: nebula prompt probe")
        );
        assert!(read(None, Some("c1")).await.is_none());
        assert!(read(at, None).await.is_none());
        assert!(
            read(at, Some("other")).await.is_none(),
            "another conversation's id"
        );
        assert!(read(Some("/nonexistent/c1/transcript.jsonl"), Some("c1"))
            .await
            .is_none());
        let mut large = vec![b'x'; TAIL_BYTES as usize];
        large.push(b'\n');
        large.extend_from_slice(TRANSCRIPT);
        std::fs::write(&path, large).unwrap();
        assert!(read(at, Some("c1")).await.is_some());
        std::fs::write(&path, vec![b'x'; TAIL_BYTES as usize + 1]).unwrap();
        assert!(read(at, Some("c1")).await.is_none());
    }

    /// Only a regular `transcript.jsonl` under the conversation's directory
    /// is read: not another file name, not a directory, not a symlink at
    /// the leaf, and not a FIFO — which must not hang the read either.
    #[tokio::test]
    async fn reads_nothing_but_the_conversations_own_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("c1");
        std::fs::create_dir_all(&logs).unwrap();
        let other = logs.join("notes.jsonl");
        std::fs::write(&other, TRANSCRIPT).unwrap();
        assert!(read(other.to_str(), Some("c1")).await.is_none());

        let path = logs.join(TRANSCRIPT_FILE);
        std::fs::create_dir(&path).unwrap();
        assert!(
            read(path.to_str(), Some("c1")).await.is_none(),
            "a directory"
        );
        std::fs::remove_dir(&path).unwrap();

        std::os::unix::fs::symlink(&other, &path).unwrap();
        assert!(read(path.to_str(), Some("c1")).await.is_none(), "a symlink");
        std::fs::remove_file(&path).unwrap();

        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let fifo = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read(path.to_str(), Some("c1")),
        )
        .await
        .expect("a FIFO never blocks the read");
        assert!(fifo.is_none());

        let dotted = format!("{}/c1/../c1/{TRANSCRIPT_FILE}", dir.path().display());
        assert!(!is_transcript_of(std::path::Path::new(&dotted), "c1"));
        assert!(!is_transcript_of(
            std::path::Path::new("c1/transcript.jsonl"),
            "c1"
        ));
    }

    #[test]
    fn latest_user_wins_and_format_drift_is_harmless() {
        let mut bytes = TRANSCRIPT.to_vec();
        bytes.extend_from_slice(b"\n{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"content\":\"next request\"}\n{broken\n");
        assert_eq!(latest_request(&bytes).as_deref(), Some("next request"));
        for bytes in [
            b"garbage".as_slice(),
            b"{}",
            b"{\"type\":\"USER_INPUT\",\"source\":\"USER_EXPLICIT\",\"content\":42}",
        ] {
            assert!(latest_request(bytes).is_none());
        }
    }
}
