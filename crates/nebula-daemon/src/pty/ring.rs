//! Per-PTY scrollback: a fixed-capacity byte ring with monotonic sequence
//! numbers. `seq` of a byte = total bytes ever written before it, so clients
//! can re-attach with `from_seq` and get a gap-free delta.

use std::collections::VecDeque;

pub struct ScrollbackRing {
    buf: VecDeque<u8>,
    cap: usize,
    /// Seq of the oldest byte still retained.
    start_seq: u64,
    /// Seq the next written byte will get.
    end_seq: u64,
}

impl ScrollbackRing {
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap), cap, start_seq: 0, end_seq: 0 }
    }

    /// Append output; returns the seq of the chunk's first byte.
    pub fn append(&mut self, data: &[u8]) -> u64 {
        let chunk_seq = self.end_seq;
        if data.len() >= self.cap {
            // Chunk alone overflows the ring: keep only its tail.
            self.buf.clear();
            self.buf.extend(&data[data.len() - self.cap..]);
        } else {
            let overflow = (self.buf.len() + data.len()).saturating_sub(self.cap);
            self.buf.drain(..overflow);
            self.buf.extend(data);
        }
        self.end_seq += data.len() as u64;
        self.start_seq = self.end_seq - self.buf.len() as u64;
        chunk_seq
    }

    pub fn end_seq(&self) -> u64 {
        self.end_seq
    }

    /// Everything retained from `from_seq` onward. If `from_seq` has fallen
    /// off the ring (or is None), returns the whole ring — the client resets
    /// its parser before applying a replay whose base != its requested seq.
    pub fn snapshot_from(&self, from_seq: Option<u64>) -> (u64, Vec<u8>) {
        let from = from_seq
            .filter(|s| *s >= self.start_seq && *s <= self.end_seq)
            .unwrap_or(self.start_seq);
        let skip = (from - self.start_seq) as usize;
        let (a, b) = self.buf.as_slices();
        let mut out = Vec::with_capacity(self.buf.len() - skip);
        if skip < a.len() {
            out.extend_from_slice(&a[skip..]);
            out.extend_from_slice(b);
        } else {
            out.extend_from_slice(&b[skip - a.len()..]);
        }
        (from, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_snapshot() {
        let mut r = ScrollbackRing::new(8);
        assert_eq!(r.append(b"abc"), 0);
        assert_eq!(r.append(b"def"), 3);
        let (base, data) = r.snapshot_from(None);
        assert_eq!(base, 0);
        assert_eq!(data, b"abcdef");
    }

    #[test]
    fn overflow_drops_oldest() {
        let mut r = ScrollbackRing::new(4);
        r.append(b"abc");
        assert_eq!(r.append(b"de"), 3); // "bcde" retained
        let (base, data) = r.snapshot_from(None);
        assert_eq!(base, 1);
        assert_eq!(data, b"bcde");
    }

    #[test]
    fn oversized_chunk_keeps_tail() {
        let mut r = ScrollbackRing::new(4);
        r.append(b"0123456789");
        let (base, data) = r.snapshot_from(None);
        assert_eq!(base, 6);
        assert_eq!(data, b"6789");
        assert_eq!(r.end_seq(), 10);
    }

    #[test]
    fn from_seq_delta() {
        let mut r = ScrollbackRing::new(16);
        r.append(b"hello ");
        r.append(b"world");
        let (base, data) = r.snapshot_from(Some(6));
        assert_eq!(base, 6);
        assert_eq!(data, b"world");
    }

    #[test]
    fn from_seq_fell_off_ring_replays_all() {
        let mut r = ScrollbackRing::new(4);
        r.append(b"abcdefgh"); // start_seq = 4
        let (base, data) = r.snapshot_from(Some(2));
        assert_eq!(base, 4);
        assert_eq!(data, b"efgh");
    }

    #[test]
    fn from_seq_at_end_is_empty() {
        let mut r = ScrollbackRing::new(8);
        r.append(b"abcd");
        let (base, data) = r.snapshot_from(Some(4));
        assert_eq!(base, 4);
        assert!(data.is_empty());
    }
}
