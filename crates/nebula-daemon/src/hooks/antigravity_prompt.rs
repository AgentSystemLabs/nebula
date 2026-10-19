//! agy omits prompt text from hooks; its transcript holds the user request.

use tokio::io::{AsyncReadExt, AsyncSeekExt};

const TAIL_BYTES: u64 = 256 * 1024;

/// Best effort only: a missing, changing or unfamiliar transcript must
/// never hold up agy's hook reply.
pub async fn read(path: Option<&str>) -> Option<String> {
    let path = path?;
    tokio::time::timeout(std::time::Duration::from_millis(50), async {
        // Reject pipes and devices before opening them.
        if !tokio::fs::metadata(path).await.ok()?.is_file() {
            return None;
        }
        let mut file = tokio::fs::File::open(path).await.ok()?;
        let len = file.metadata().await.ok()?.len();
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
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(&path, TRANSCRIPT).unwrap();
        assert_eq!(
            read(path.to_str()).await.as_deref(),
            Some("Reply with exactly: nebula prompt probe")
        );
        assert!(read(None).await.is_none());
        assert!(read(Some("/nonexistent/nebula-transcript")).await.is_none());
        assert!(read(dir.path().to_str()).await.is_none());
        let mut large = vec![b'x'; TAIL_BYTES as usize];
        large.push(b'\n');
        large.extend_from_slice(TRANSCRIPT);
        std::fs::write(&path, large).unwrap();
        assert!(read(path.to_str()).await.is_some());
        std::fs::write(&path, vec![b'x'; TAIL_BYTES as usize + 1]).unwrap();
        assert!(read(path.to_str()).await.is_none());
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
