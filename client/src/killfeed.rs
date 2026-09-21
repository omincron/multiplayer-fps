//! Kill feed (Milestone 13): a short-lived, bounded list of recent kill
//! messages, driven by `EventKind::Killed`. Pure display-list logic —
//! text formatting (who's "you", name lookups) is the caller's job, since
//! that needs `RemotePlayers`/the local player's own name and doesn't
//! belong in a module that otherwise has no notion of players at all.

use std::collections::VecDeque;

/// How long a line stays on screen before it ages out.
const DISPLAY_DURATION_MS: f64 = 6000.0;
/// Newest-first cap so a flurry of kills can't grow the feed unbounded.
const CAPACITY: usize = 5;

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    text: String,
    expires_at_ms: f64,
}

#[derive(Debug, Default)]
pub struct KillFeed {
    entries: VecDeque<Entry>,
}

impl KillFeed {
    pub fn push(&mut self, text: String, now_ms: f64) {
        self.entries.push_back(Entry {
            text,
            expires_at_ms: now_ms + DISPLAY_DURATION_MS,
        });
        while self.entries.len() > CAPACITY {
            self.entries.pop_front();
        }
    }

    /// Currently-visible lines, newest first, dropping anything whose
    /// display window has elapsed. Takes `&mut self` because expiry is
    /// swept as part of the call rather than needing a separate `tick()`
    /// the caller has to remember to invoke every frame.
    pub fn visible(&mut self, now_ms: f64) -> impl Iterator<Item = &str> {
        self.entries.retain(|entry| entry.expires_at_ms > now_ms);
        self.entries.iter().rev().map(|entry| entry.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_lines_are_newest_first() {
        let mut feed = KillFeed::default();
        feed.push("a killed b".into(), 0.0);
        feed.push("c killed d".into(), 100.0);

        let lines: Vec<&str> = feed.visible(200.0).collect();
        assert_eq!(lines, vec!["c killed d", "a killed b"]);
    }

    #[test]
    fn entries_disappear_after_their_display_window() {
        let mut feed = KillFeed::default();
        feed.push("a killed b".into(), 0.0);

        assert_eq!(feed.visible(DISPLAY_DURATION_MS - 1.0).count(), 1);
        assert_eq!(feed.visible(DISPLAY_DURATION_MS + 1.0).count(), 0);
    }

    #[test]
    fn capacity_drops_the_oldest_entry_first() {
        let mut feed = KillFeed::default();
        for i in 0..CAPACITY + 2 {
            feed.push(format!("kill {i}"), 0.0);
        }

        let lines: Vec<&str> = feed.visible(0.0).collect();
        assert_eq!(lines.len(), CAPACITY);
        // Newest-first, and the two oldest ("kill 0", "kill 1") are gone.
        assert_eq!(lines.first(), Some(&"kill 6"));
        assert!(!lines.contains(&"kill 0"));
        assert!(!lines.contains(&"kill 1"));
    }
}
