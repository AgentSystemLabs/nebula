//! Claude Cloud scanner — the one thing a `claude --cloud=<task>` child
//! says on its output that nebula must act on, read straight off the PTY
//! stream.
//!
//! The dispatch creates the session, prints where it lives, and exits:
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
//! The agent then runs in the cloud sandbox: nebula never attaches to or
//! teleports the session, it shows the row's pane as a panel linking to
//! the session's page.
//!
//! The `Created cloud session:` line before them carries the title Claude
//! Cloud gave the session — its own summary of the task — and that is
//! read too (issue #92): the agent runs where no hook ever reaches
//! nebula, so nothing else would ever name the row, and a grid of cloud
//! cards all called `agent` tells nobody which is which. It is reported
//! ahead of the id, the order the CLI prints them in.

/// Byte sequences that immediately precede a session id.
const ID_MARKERS: [&[u8]; 2] = [b"claude.ai/code/session_", b"--teleport session_"];

/// What precedes the session's title on the create's first line.
const TITLE_MARKER: &[u8] = b"Created cloud session: ";
/// A title run longer than this is taken as it stands rather than waiting
/// for its line end; the CLI's own titles are a short sentence.
const MAX_TITLE_LEN: usize = 256;

/// An id longer than this is accepted as-is rather than waiting for its
/// terminator; real ids are ~28 characters.
const MAX_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudSighting {
    /// The title the child printed for the session — Claude Cloud's own
    /// name for it, raw: the row's name is sanitized where it is applied.
    Title(String),
    /// The session id the child printed, `session_` prefix included.
    SessionId(String),
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

/// Tracks the sightings across chunk boundaries. Each is reported once.
#[derive(Debug)]
pub struct CloudScanner {
    /// Unconsumed tail of prior chunks: enough to complete a marker that
    /// straddles chunks, or a marker plus a title or id still being
    /// printed.
    tail: Vec<u8>,
    title_found: bool,
    id_found: bool,
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
            title_found: false,
            id_found: false,
        }
    }

    /// Scan a chunk of child output. Markers split across chunks are fine —
    /// the retained tail bridges them. Returns the sightings this chunk
    /// completed (at most two, ever: the title, then the id). The id ends
    /// the scan: the CLI prints the title first, so a title not seen by
    /// then was never printed.
    pub fn feed(&mut self, data: &[u8]) -> Vec<CloudSighting> {
        let mut out = Vec::new();
        if self.id_found {
            return out;
        }
        self.tail.extend_from_slice(data);

        let mut keep_from = None;
        if !self.title_found {
            match self.scan_title() {
                IdScan::Found(title) => {
                    self.title_found = true;
                    out.push(CloudSighting::Title(title));
                }
                IdScan::Pending { start } => keep_from = Some(start),
                IdScan::None => {}
            }
        }
        match self.scan_id() {
            IdScan::Found(id) => {
                self.id_found = true;
                out.push(CloudSighting::SessionId(id));
            }
            IdScan::Pending { start } => {
                keep_from = Some(keep_from.map_or(start, |t| t.min(start)));
            }
            IdScan::None => {}
        }

        if self.id_found {
            self.tail.clear();
        } else {
            // Keep a pending title or id whole; otherwise only what a
            // marker that straddles the boundary could need.
            let keep = keep_from.unwrap_or_else(|| {
                let longest = ID_MARKERS
                    .iter()
                    .map(|m| m.len())
                    .chain([TITLE_MARKER.len()])
                    .max()
                    .unwrap_or(0);
                self.tail.len().saturating_sub(longest - 1)
            });
            self.tail.drain(..keep);
        }
        out
    }

    /// The title line: from its marker to the end of its line. A run
    /// past [`MAX_TITLE_LEN`] with no line end yet is taken as it stands,
    /// like an overlong id.
    fn scan_title(&self) -> IdScan {
        let buf = &self.tail;
        let Some(start) = find(buf, TITLE_MARKER) else {
            return IdScan::None;
        };
        let title_bytes = &buf[start + TITLE_MARKER.len()..];
        let len = title_bytes
            .iter()
            .take_while(|b| **b != b'\r' && **b != b'\n')
            .count();
        if len == title_bytes.len() && len < MAX_TITLE_LEN {
            return IdScan::Pending { start };
        }
        IdScan::Found(String::from_utf8_lossy(&title_bytes[..len]).into_owned())
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
            CloudSighting::Title(title) => super::PtyEvent::CloudTitle { title },
            CloudSighting::SessionId(id) => super::PtyEvent::CloudSession { id },
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

    /// What the whole create yields, in the order the CLI prints it.
    fn both() -> Vec<CloudSighting> {
        vec![
            CloudSighting::Title("Hello world".into()),
            CloudSighting::SessionId(ID.into()),
        ]
    }

    #[test]
    fn create_output_yields_the_title_then_the_id_once() {
        let mut s = CloudScanner::new();
        assert_eq!(s.feed(CREATE), both());
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
            assert_eq!(got, both(), "cut at {cut}");
        }
    }

    #[test]
    fn byte_at_a_time_matches_whole_chunk() {
        let mut s = CloudScanner::new();
        let mut got = Vec::new();
        for b in CREATE {
            got.extend(s.feed(std::slice::from_ref(b)));
        }
        assert_eq!(got, both());
    }

    /// An older CLI, or a JSON-mode one, that never prints the title line:
    /// the id alone is reported, and the scan ends on it all the same.
    #[test]
    fn id_without_a_title_line_ends_the_scan() {
        let mut s = CloudScanner::new();
        assert_eq!(
            s.feed(b"View: https://claude.ai/code/session_abc?x\r\n"),
            vec![CloudSighting::SessionId("session_abc".into())]
        );
        assert!(s.feed(b"Created cloud session: Late\r\n").is_empty());
    }

    /// The title is whatever sits between its marker and the line end —
    /// spaces, punctuation and a `session_` in it included — and an
    /// unterminated one waits for its line end rather than reporting a
    /// prefix.
    #[test]
    fn title_runs_to_its_line_end() {
        let mut s = CloudScanner::new();
        assert!(s.feed(b"Created cloud session: Fix the").is_empty());
        assert_eq!(
            s.feed(b" session_ handoff: part 2\nView: https://claude.ai/code/session_q1?m=0\n"),
            vec![
                CloudSighting::Title("Fix the session_ handoff: part 2".into()),
                CloudSighting::SessionId("session_q1".into()),
            ]
        );
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
    fn unrelated_output_keeps_a_bounded_tail() {
        let mut s = CloudScanner::new();
        for _ in 0..1000 {
            assert!(s.feed(&[b'x'; 100]).is_empty());
        }
        assert!(s.tail.len() < 128, "tail grew to {}", s.tail.len());
    }
}
