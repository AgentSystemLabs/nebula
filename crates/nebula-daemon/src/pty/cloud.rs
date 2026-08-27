//! Claude Cloud scanner — the two things a `claude --cloud` child says on
//! its output that nebula must act on, read straight off the PTY stream.
//!
//! On an account without the live-attach rollout (Claude's
//! `tengu_remote_backend` flag, 2.1.247), `claude --cloud <task>` creates
//! the session, prints where it lives, and exits:
//!
//! ```text
//! Created cloud session: Hello world
//! View: https://claude.ai/code/session_016SiQW5Lem2LbnUf1A3undt?from=cli&m=0
//! Resume with: claude --teleport session_016SiQW5Lem2LbnUf1A3undt
//! ```
//!
//! That id is the only handle nebula ever gets on the session, and the
//! process is gone milliseconds after printing it, so it is captured here
//! rather than asked for. Both lines carry it; the first sighting wins.
//!
//! The same accounts have `claude --cloud <id>` refuse with
//! `Error: Attaching to an existing cloud session is not enabled for your
//! account.` and exit 1. Seeing that line is what tells the daemon to fall
//! back to a teleport — a deliberate kill of an attach that *did* work looks
//! identical from the exit code alone, so the refusal has to be read, not
//! inferred.

/// Byte sequences that immediately precede a session id.
const ID_MARKERS: [&[u8]; 2] = [b"claude.ai/code/session_", b"--teleport session_"];

/// Fragments of the CLI's attach-refusal messages. The account-gate wording
/// is the one observed; the other is the generic attach failure that
/// precedes any reason text.
const REJECT_MARKERS: [&[u8]; 2] = [
    b"cloud session is not enabled for your account",
    b"Couldn't attach to cloud session",
];

/// An id longer than this is accepted as-is rather than waiting for its
/// terminator; real ids are ~28 characters.
const MAX_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudSighting {
    /// The session id the child printed, `session_` prefix included.
    SessionId(String),
    /// The child refused to attach; it exits right after.
    AttachRejected,
}

/// Where an id search left off within the retained tail.
enum IdScan {
    Found(String),
    /// A marker (or a marker and part of an id) sits at `start` and the
    /// chunk ended before the id did — keep from there and wait for more.
    Pending {
        start: usize,
    },
    None,
}

/// Tracks sightings across chunk boundaries. Each sighting is reported once.
#[derive(Debug)]
pub struct CloudScanner {
    /// Unconsumed tail of prior chunks: enough to complete a marker that
    /// straddles chunks, or a marker plus an id still being printed.
    tail: Vec<u8>,
    id_found: bool,
    rejected: bool,
}

impl Default for CloudScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudScanner {
    pub fn new() -> Self {
        Self {
            tail: Vec::new(),
            id_found: false,
            rejected: false,
        }
    }

    /// Scan a chunk of child output. Markers split across chunks are fine —
    /// the retained tail bridges them. Returns the sightings this chunk
    /// completed, in the order they were completed.
    pub fn feed(&mut self, data: &[u8]) -> Vec<CloudSighting> {
        let mut out = Vec::new();
        if self.id_found && self.rejected {
            return out;
        }
        self.tail.extend_from_slice(data);

        if !self.rejected && REJECT_MARKERS.iter().any(|m| find(&self.tail, m).is_some()) {
            self.rejected = true;
            out.push(CloudSighting::AttachRejected);
        }

        let mut keep_from = None;
        if !self.id_found {
            match self.scan_id() {
                IdScan::Found(id) => {
                    self.id_found = true;
                    out.push(CloudSighting::SessionId(id));
                }
                IdScan::Pending { start } => keep_from = Some(start),
                IdScan::None => {}
            }
        }

        if self.id_found && self.rejected {
            self.tail.clear();
        } else {
            // Keep a pending id whole; otherwise only what a marker that
            // straddles the boundary could need.
            let keep = keep_from.unwrap_or_else(|| {
                let longest = ID_MARKERS
                    .iter()
                    .chain(REJECT_MARKERS.iter())
                    .map(|m| m.len())
                    .max()
                    .unwrap_or(0);
                self.tail.len().saturating_sub(longest - 1)
            });
            self.tail.drain(..keep);
        }
        out
    }

    fn scan_id(&self) -> IdScan {
        let buf = &self.tail;
        let mut from = 0;
        loop {
            // Earliest marker at or after `from`.
            let mut best: Option<(usize, usize)> = None;
            for marker in ID_MARKERS {
                if let Some(pos) = find(&buf[from..], marker) {
                    let start = from + pos;
                    if best.is_none_or(|(s, _)| start < s) {
                        best = Some((start, start + marker.len()));
                    }
                }
            }
            let Some((start, id_start)) = best else {
                return IdScan::None;
            };
            let id_bytes = &buf[id_start..];
            let len = id_bytes
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            if len == id_bytes.len() && len < MAX_ID_LEN {
                // Still being printed (or the marker was the last thing in
                // the chunk) — wait for the terminator.
                return IdScan::Pending { start };
            }
            if len == 0 {
                // Marker followed by something that is not an id; look past it.
                from = id_start;
                continue;
            }
            let id = format!(
                "session_{}",
                std::str::from_utf8(&id_bytes[..len]).expect("ascii alphanumerics")
            );
            return IdScan::Found(id);
        }
    }
}

impl From<CloudSighting> for super::PtyEvent {
    fn from(sighting: CloudSighting) -> Self {
        match sighting {
            CloudSighting::SessionId(id) => super::PtyEvent::CloudSession { id },
            CloudSighting::AttachRejected => super::PtyEvent::CloudAttachRejected,
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE: &[u8] = b"No .nvmrc file found\r\n\
Please see `nvm --help` or https://github.com/nvm-sh/nvm#nvmrc for more information.\r\n\
Created cloud session: Hello world\r\n\
View: https://claude.ai/code/session_016SiQW5Lem2LbnUf1A3undt?from=cli&m=0\r\n\
Resume with: claude --teleport session_016SiQW5Lem2LbnUf1A3undt\r\n";

    const ID: &str = "session_016SiQW5Lem2LbnUf1A3undt";

    #[test]
    fn create_output_yields_the_id_once() {
        let mut s = CloudScanner::new();
        assert_eq!(s.feed(CREATE), vec![CloudSighting::SessionId(ID.into())]);
        // The teleport line repeats the id; it is not reported again.
        assert!(s
            .feed(b"Resume with: claude --teleport session_016SiQW5Lem2LbnUf1A3undt\r\n")
            .is_empty());
    }

    #[test]
    fn id_split_across_chunks_is_bridged() {
        for cut in 1..CREATE.len() {
            let mut s = CloudScanner::new();
            let mut got = s.feed(&CREATE[..cut]);
            got.extend(s.feed(&CREATE[cut..]));
            assert_eq!(
                got,
                vec![CloudSighting::SessionId(ID.into())],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn byte_at_a_time_matches_whole_chunk() {
        let mut s = CloudScanner::new();
        let mut got = Vec::new();
        for b in CREATE {
            got.extend(s.feed(std::slice::from_ref(b)));
        }
        assert_eq!(got, vec![CloudSighting::SessionId(ID.into())]);
    }

    #[test]
    fn teleport_line_alone_is_enough() {
        let mut s = CloudScanner::new();
        assert_eq!(
            s.feed(b"Resume with: claude --teleport session_abc123\r\n"),
            vec![CloudSighting::SessionId("session_abc123".into())]
        );
    }

    #[test]
    fn marker_without_an_id_is_skipped_not_stuck() {
        let mut s = CloudScanner::new();
        assert!(s
            .feed(b"see claude.ai/code/session_ (none) and ")
            .is_empty());
        assert_eq!(
            s.feed(b"https://claude.ai/code/session_zz9?x\r\n"),
            vec![CloudSighting::SessionId("session_zz9".into())]
        );
    }

    #[test]
    fn overlong_run_is_accepted_without_a_terminator() {
        let mut s = CloudScanner::new();
        let long = "a".repeat(MAX_ID_LEN);
        let line = format!("--teleport session_{long}");
        assert_eq!(
            s.feed(line.as_bytes()),
            vec![CloudSighting::SessionId(format!("session_{long}"))]
        );
    }

    #[test]
    fn attach_refusal_is_reported_once_and_split_safe() {
        let msg =
            b"Error: Attaching to an existing cloud session is not enabled for your account.\r\n";
        for cut in 1..msg.len() {
            let mut s = CloudScanner::new();
            let mut got = s.feed(&msg[..cut]);
            got.extend(s.feed(&msg[cut..]));
            assert_eq!(got, vec![CloudSighting::AttachRejected], "cut at {cut}");
            assert!(s.feed(msg).is_empty());
        }
    }

    #[test]
    fn unrelated_output_keeps_a_bounded_tail() {
        let mut s = CloudScanner::new();
        for _ in 0..1000 {
            assert!(s.feed(&[b'x'; 100]).is_empty());
        }
        assert!(s.tail.len() < 128, "tail grew to {}", s.tail.len());
    }
}
