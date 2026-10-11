//! Bounded, ordered event stream. The single log source for the logcat-style
//! API: kernel, framework, apps, and the agent plane all push here.

use litebox_json::Json;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl EventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            EventLevel::Debug => "D",
            EventLevel::Info => "I",
            EventLevel::Warn => "W",
            EventLevel::Error => "E",
        }
    }
    pub fn parse(s: &str) -> Option<EventLevel> {
        match s {
            "D" | "debug" => Some(EventLevel::Debug),
            "I" | "info" => Some(EventLevel::Info),
            "W" | "warn" | "warning" => Some(EventLevel::Warn),
            "E" | "error" => Some(EventLevel::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventRecord {
    pub seq: u64,
    /// Virtual microseconds since boot.
    pub ts_us: u64,
    pub level: EventLevel,
    /// android.util.Log style tag, e.g. "dalvik", "pkg/com.acme", "pm".
    pub tag: String,
    pub msg: String,
    /// Structured payload for agents (exception dumps, state diffs, ...).
    pub data: Option<Json>,
}

#[derive(Debug)]
pub struct EventBus {
    ring: VecDeque<EventRecord>,
    next_seq: u64,
    capacity: usize,
    dropped: u64,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        EventBus {
            ring: VecDeque::with_capacity(capacity.max(8)),
            next_seq: 0,
            capacity,
            dropped: 0,
        }
    }

    pub fn push(
        &mut self,
        ts_us: u64,
        level: EventLevel,
        tag: &str,
        msg: &str,
        data: Option<Json>,
    ) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.ring.len() == self.capacity {
            self.ring.pop_front();
            self.dropped += 1;
        }
        self.ring.push_back(EventRecord {
            seq,
            ts_us,
            level,
            tag: tag.to_string(),
            msg: msg.to_string(),
            data,
        });
        seq
    }

    /// All records with seq >= cursor (oldest first). Returns new cursor:
    /// pass the returned cursor back to continue exactly where you left off.
    pub fn take_after(&self, cursor: u64, max: usize) -> (Vec<EventRecord>, u64) {
        let out: Vec<EventRecord> =
            self.ring.iter().filter(|r| r.seq >= cursor).take(max).cloned().collect();
        let new_cursor = out.last().map(|r| r.seq + 1).unwrap_or(cursor);
        (out, new_cursor)
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Force the sequence watermark (snapshot restore).
    pub fn set_next_seq(&mut self, seq: u64) {
        self.next_seq = seq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_cursor() {
        let mut b = EventBus::new(4);
        b.push(1, EventLevel::Info, "t", "one", None);
        b.push(2, EventLevel::Error, "t", "two", None);
        let (got, cur) = b.take_after(0, 10);
        assert_eq!(got.len(), 2);
        assert_eq!(cur, 2, "cursor is one past the last delivered seq");
        let (got2, cur2) = b.take_after(cur, 10);
        assert!(got2.is_empty());
        assert_eq!(cur2, 2);
        // re-fetch from 0 works
        assert_eq!(b.take_after(0, 10).0.len(), 2);
    }

    #[test]
    fn ring_eviction_counts_dropped() {
        let mut b = EventBus::new(2);
        for i in 0..5 {
            b.push(i, EventLevel::Info, "t", &format!("m{i}"), None);
        }
        assert_eq!(b.len(), 2);
        assert_eq!(b.dropped(), 3);
        let (got, _) = b.take_after(0, 10);
        assert_eq!(got[0].msg, "m3");
    }
}
