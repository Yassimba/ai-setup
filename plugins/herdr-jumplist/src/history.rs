use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 50;
/// Bounds the pending-jump markers so a lost pane.focused event can't leave
/// suppressions accumulating forever.
const MAX_PENDING_SUPPRESS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    cursor: usize,
    /// Panes we jumped to whose pane.focused events haven't arrived yet, so
    /// they are not recorded as fresh visits. A list, not a slot: rapid
    /// repeated jumps can outrun event delivery, leaving several pending.
    #[serde(default)]
    suppress: Vec<String>,
}

impl History {
    /// Clamp state loaded from disk so a corrupt or hand-edited file cannot
    /// put the cursor out of bounds.
    pub fn sanitize(&mut self) {
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
    }

    /// Record a focus change coming in from the pane.focused event hook.
    pub fn record(&mut self, pane: &str) {
        if let Some(pending) = self.suppress.iter().position(|p| p == pane) {
            // One of our own jump landings; the cursor already covers it.
            self.suppress.remove(pending);
            return;
        }
        // A focus we didn't cause makes any still-pending markers stale.
        self.suppress.clear();
        if self.entries.get(self.cursor).map(String::as_str) == Some(pane) {
            return;
        }
        // A fresh visit discards the forward tail, like an editor jumplist.
        self.entries.truncate(self.cursor + 1);
        self.entries.push(pane.to_string());
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        self.cursor = self.entries.len() - 1;
    }

    /// Walk the history in `direction`, focusing the first pane that still
    /// exists. `focus` returns whether the pane could be focused; entries it
    /// rejects are pruned. Returns the pane that was focused, if any.
    pub fn jump(
        &mut self,
        direction: Direction,
        mut focus: impl FnMut(&str) -> bool,
    ) -> Option<String> {
        loop {
            let index = match direction {
                Direction::Back => self.cursor.checked_sub(1)?,
                Direction::Forward => {
                    let next = self.cursor + 1;
                    if next >= self.entries.len() {
                        return None;
                    }
                    next
                }
            };
            let pane = self.entries[index].clone();
            let duplicate = self.entries.get(self.cursor) == Some(&pane);
            if !duplicate && focus(&pane) {
                self.cursor = index;
                self.suppress.push(pane.clone());
                if self.suppress.len() > MAX_PENDING_SUPPRESS {
                    self.suppress.remove(0);
                }
                return Some(pane);
            }
            // Dead pane or a duplicate of the current entry: drop it and keep
            // walking. Removing below the cursor shifts everything left.
            self.entries.remove(index);
            if direction == Direction::Back {
                self.cursor -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_all(history: &mut History, panes: &[&str]) {
        for pane in panes {
            history.record(pane);
        }
    }

    fn always(_: &str) -> bool {
        true
    }

    #[test]
    fn back_walks_history_and_forward_returns() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "c"]);
        assert_eq!(history.jump(Direction::Back, always), Some("b".into()));
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
        assert_eq!(history.jump(Direction::Back, always), None);
        assert_eq!(history.jump(Direction::Forward, always), Some("b".into()));
        assert_eq!(history.jump(Direction::Forward, always), Some("c".into()));
        assert_eq!(history.jump(Direction::Forward, always), None);
    }

    #[test]
    fn consecutive_refocus_is_not_recorded_twice() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "b", "b"]);
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
        assert_eq!(history.jump(Direction::Back, always), None);
    }

    #[test]
    fn jump_landing_is_suppressed_not_recorded() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b"]);
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
        // herdr fires pane.focused for the pane we just jumped to.
        history.record("a");
        // Forward history must survive: "b" is still reachable.
        assert_eq!(history.jump(Direction::Forward, always), Some("b".into()));
    }

    #[test]
    fn suppress_marker_only_covers_one_event() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "c"]);
        history.jump(Direction::Back, always); // to b, suppress b
        history.record("b"); // consumed
        history.record("a"); // genuine visit: truncates forward tail
        assert_eq!(history.jump(Direction::Forward, always), None);
        assert_eq!(history.jump(Direction::Back, always), Some("b".into()));
    }

    #[test]
    fn rapid_double_jump_suppresses_both_late_events() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "c"]);
        // Two back-jumps land before either pane.focused event arrives.
        assert_eq!(history.jump(Direction::Back, always), Some("b".into()));
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
        history.record("b");
        history.record("a");
        // History is untouched: forward still walks b then c.
        assert_eq!(history.jump(Direction::Forward, always), Some("b".into()));
        assert_eq!(history.jump(Direction::Forward, always), Some("c".into()));
    }

    #[test]
    fn mismatched_suppress_marker_is_cleared_and_recorded() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b"]);
        history.jump(Direction::Back, always);
        // User clicked "c" before our jump's event arrived.
        history.record("c");
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
    }

    #[test]
    fn fresh_visit_discards_forward_tail() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "c"]);
        history.jump(Direction::Back, always);
        history.jump(Direction::Back, always);
        history.record("d");
        assert_eq!(history.jump(Direction::Forward, always), None);
        assert_eq!(history.jump(Direction::Back, always), Some("a".into()));
    }

    #[test]
    fn dead_panes_are_pruned_while_walking() {
        let mut history = History::default();
        record_all(&mut history, &["a", "b", "c", "d"]);
        let alive = |pane: &str| pane == "a" || pane == "d";
        assert_eq!(history.jump(Direction::Back, alive), Some("a".into()));
        // b and c were pruned; forward goes straight back to d.
        assert_eq!(history.jump(Direction::Forward, alive), Some("d".into()));
    }

    #[test]
    fn history_is_capped() {
        let mut history = History::default();
        for i in 0..(MAX_ENTRIES + 10) {
            history.record(&format!("pane-{i}"));
        }
        for _ in 0..MAX_ENTRIES {
            history.jump(Direction::Back, always);
        }
        assert_eq!(history.jump(Direction::Back, always), None);
        assert_eq!(history.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn sanitize_clamps_out_of_bounds_cursor() {
        let mut history: History = serde_json::from_str(r#"{"entries":["a"],"cursor":9}"#).unwrap();
        history.sanitize();
        assert_eq!(history.cursor, 0);
    }

    #[test]
    fn empty_history_jumps_nowhere() {
        let mut history = History::default();
        assert_eq!(history.jump(Direction::Back, always), None);
        assert_eq!(history.jump(Direction::Forward, always), None);
    }
}
