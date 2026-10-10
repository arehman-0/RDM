//! Download queue: a concurrency cap, a waiting list and scheduled starts.
//!
//! Pure logic, no Tauri. Side effects (spawning a download, telling the UI,
//! arming a wake-up timer) go through the [`QueueHost`] trait — the same
//! pattern as the engine's `ProgressSink` — so promote / cancel / schedule
//! behaviour is unit-testable with a recording host.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// Wall-clock epoch seconds (0 if the clock is before 1970).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Default number of downloads allowed to run at once.
pub const DEFAULT_MAX_CONCURRENT: usize = 3;

/// Queue priority. The waiting list is kept sorted High → Normal → Low and is
/// FIFO within a band, so a higher-priority entry always starts first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

/// A unit of queued work — either a plain file or a stream-site download.
#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub id: String,
    pub url: String,
    /// Exact destination (Save-As, or the path a resumed download already
    /// uses); otherwise the engine names the file inside `dir`.
    pub save_path: Option<PathBuf>,
    pub dir: PathBuf,
    pub file_name: String,
    pub connections: usize,
    pub speed_limit: u64,
    pub verify: bool,
    pub expected_checksum: Option<String>,
    pub headers: Vec<(String, String)>,
    pub mirrors: Vec<String>,
    pub is_stream: bool,
    /// Epoch seconds to start at; `None` = as soon as a slot frees.
    pub start_at: Option<u64>,
    pub priority: Priority,
    /// Paused while waiting: kept in place, never started until released.
    pub held: bool,
    /// Browser-suggested file name (see `DownloadOptions::name_hint`).
    pub name_hint: Option<String>,
    /// `url` is an HLS/DASH manifest downloaded by yt-dlp (needs `is_stream`).
    pub manifest: bool,
    /// Stream quality (`best` / `audio` / a height); `None` = best.
    pub quality: Option<String>,
}

/// `queue-status` event payload: where a not-yet-running entry stands.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStatus {
    pub id: String,
    pub file_name: String,
    /// `queued` | `scheduled` | `held` | `starting` | `canceled`
    pub status: String,
    /// 1-based position among waiting downloads (0 if not waiting).
    pub position: usize,
    /// Epoch seconds a scheduled entry will start at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<u64>,
    pub priority: Priority,
    pub held: bool,
}

impl QueueStatus {
    /// Status for an entry that has left the waiting list.
    pub fn left(e: &QueueEntry, status: &str) -> Self {
        Self {
            id: e.id.clone(),
            file_name: e.file_name.clone(),
            status: status.into(),
            position: 0,
            start_at: None,
            priority: e.priority,
            held: false,
        }
    }
}

/// Everything the queue needs from its environment.
pub trait QueueHost: Send + Sync {
    /// Start running `entry` (typically on a spawned task). When it ends, the
    /// host must call [`QueueManager::finished`] to release its slot.
    fn launch(&self, entry: QueueEntry);
    /// Report a waiting entry's state.
    fn queue_status(&self, status: QueueStatus);
    /// Call [`QueueManager::promote`] at (or shortly after) epoch second `at`.
    fn arm_timer(&self, at: u64);
}

pub struct QueueManager {
    max_concurrent: AtomicUsize,
    running: AtomicUsize,
    pending: Mutex<VecDeque<QueueEntry>>,
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT)
    }
}

impl QueueManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent: AtomicUsize::new(max_concurrent.clamp(1, 10)),
            running: AtomicUsize::new(0),
            pending: Mutex::new(VecDeque::new()),
        }
    }

    fn pending(&self) -> MutexGuard<'_, VecDeque<QueueEntry>> {
        // A panic while holding this lock can't leave the deque inconsistent
        // (every mutation is a single push/remove), so recover from poison.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_max_concurrent(&self, max: usize) {
        self.max_concurrent
            .store(max.clamp(1, 10), Ordering::Release);
    }

    #[cfg(test)]
    pub fn running(&self) -> usize {
        self.running.load(Ordering::Acquire)
    }

    /// Ids of every waiting entry (tray "Pause all" / "Resume all").
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending().iter().map(|e| e.id.clone()).collect()
    }

    #[cfg(test)]
    pub fn is_pending(&self, id: &str) -> bool {
        self.pending().iter().any(|e| e.id == id)
    }

    /// Insert keeping the list sorted by priority (FIFO within a band).
    fn insert_sorted(pending: &mut VecDeque<QueueEntry>, entry: QueueEntry) {
        let at = pending
            .iter()
            .position(|e| e.priority < entry.priority)
            .unwrap_or(pending.len());
        pending.insert(at, entry);
    }

    /// Add an entry (arming its timer if it is scheduled for later), then
    /// start whatever fits.
    pub fn enqueue(&self, host: &dyn QueueHost, entry: QueueEntry, now: u64) {
        if let Some(t) = entry.start_at.filter(|t| *t > now) {
            host.arm_timer(t);
        }
        Self::insert_sorted(&mut self.pending(), entry);
        self.promote(host, now);
    }

    /// Hold (pause) or release a waiting entry. `false` if it isn't waiting.
    pub fn set_held(&self, host: &dyn QueueHost, id: &str, held: bool, now: u64) -> bool {
        let found = {
            let mut pending = self.pending();
            match pending.iter_mut().find(|e| e.id == id) {
                Some(e) => {
                    e.held = held;
                    true
                }
                None => false,
            }
        };
        if found {
            // Releasing may let it start right away.
            self.promote(host, now);
        }
        found
    }

    /// Change a waiting entry's priority; it moves to the end of its new band.
    pub fn set_priority(
        &self,
        host: &dyn QueueHost,
        id: &str,
        priority: Priority,
        now: u64,
    ) -> bool {
        let found = {
            let mut pending = self.pending();
            match pending.iter().position(|e| e.id == id) {
                Some(i) => {
                    if let Some(mut e) = pending.remove(i) {
                        e.priority = priority;
                        Self::insert_sorted(&mut pending, e);
                    }
                    true
                }
                None => false,
            }
        };
        if found {
            self.emit_positions(host, now);
        }
        found
    }

    /// Move a waiting entry `delta` places (negative = earlier). It stays
    /// inside its priority band; use [`set_priority`](Self::set_priority) to
    /// cross bands. Returns whether it is waiting at all.
    pub fn move_by(&self, host: &dyn QueueHost, id: &str, delta: i32, now: u64) -> bool {
        let found = {
            let mut pending = self.pending();
            match pending.iter().position(|e| e.id == id) {
                Some(mut i) => {
                    let band = pending[i].priority;
                    for _ in 0..delta.unsigned_abs() {
                        let j = if delta < 0 {
                            i.checked_sub(1)
                        } else {
                            Some(i + 1).filter(|j| *j < pending.len())
                        };
                        match j {
                            Some(j) if pending[j].priority == band => {
                                pending.swap(i, j);
                                i = j;
                            }
                            _ => break,
                        }
                    }
                    true
                }
                None => false,
            }
        };
        if found {
            self.emit_positions(host, now);
        }
        found
    }

    /// Start as many *ready* entries as the cap allows (in list order —
    /// priority, then FIFO — skipping held ones and ones scheduled for the
    /// future), then re-broadcast the waiting list.
    pub fn promote(&self, host: &dyn QueueHost, now: u64) {
        let mut ready = Vec::new();
        {
            // Check the cap, pick, and reserve the slot under one lock so
            // concurrent promotes never over-subscribe.
            let mut pending = self.pending();
            while self.running.load(Ordering::Acquire) < self.max_concurrent.load(Ordering::Acquire)
            {
                let Some(i) = pending
                    .iter()
                    .position(|e| !e.held && e.start_at.is_none_or(|t| t <= now))
                else {
                    break;
                };
                if let Some(e) = pending.remove(i) {
                    self.running.fetch_add(1, Ordering::SeqCst);
                    ready.push(e);
                }
            }
        }
        // Launch outside the lock: a host may call back into the queue.
        for e in ready {
            host.launch(e);
        }
        self.emit_positions(host, now);
    }

    /// A launched entry ended (any outcome): free its slot and promote.
    pub fn finished(&self, host: &dyn QueueHost, now: u64) {
        // Saturating decrement (a stray extra call must not wrap to usize::MAX).
        let mut n = self.running.load(Ordering::Acquire);
        while n > 0 {
            match self
                .running
                .compare_exchange_weak(n, n - 1, Ordering::SeqCst, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => n = actual,
            }
        }
        self.promote(host, now);
    }

    /// Remove a still-waiting entry. Returns it if it was waiting (the host is
    /// told it was canceled); `None` means it is running or unknown.
    pub fn cancel_pending(&self, host: &dyn QueueHost, id: &str, now: u64) -> Option<QueueEntry> {
        let removed = {
            let mut pending = self.pending();
            let i = pending.iter().position(|e| e.id == id)?;
            pending.remove(i)
        }?;
        host.queue_status(QueueStatus::left(&removed, "canceled"));
        self.emit_positions(host, now);
        Some(removed)
    }

    fn emit_positions(&self, host: &dyn QueueHost, now: u64) {
        let items: Vec<QueueStatus> = self
            .pending()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let future = e.start_at.filter(|t| *t > now);
                let status = if e.held {
                    "held"
                } else if future.is_some() {
                    "scheduled"
                } else {
                    "queued"
                };
                QueueStatus {
                    id: e.id.clone(),
                    file_name: e.file_name.clone(),
                    status: status.into(),
                    position: i + 1,
                    start_at: future,
                    priority: e.priority,
                    held: e.held,
                }
            })
            .collect();
        for s in items {
            host.queue_status(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingHost {
        launched: Mutex<Vec<String>>,
        statuses: Mutex<Vec<QueueStatus>>,
        timers: Mutex<Vec<u64>>,
    }

    impl QueueHost for RecordingHost {
        fn launch(&self, entry: QueueEntry) {
            self.launched.lock().unwrap().push(entry.id);
        }
        fn queue_status(&self, status: QueueStatus) {
            self.statuses.lock().unwrap().push(status);
        }
        fn arm_timer(&self, at: u64) {
            self.timers.lock().unwrap().push(at);
        }
    }

    impl RecordingHost {
        fn launched(&self) -> Vec<String> {
            self.launched.lock().unwrap().clone()
        }
        fn last_status(&self, id: &str) -> Option<QueueStatus> {
            self.statuses
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|s| s.id == id)
                .cloned()
        }
    }

    fn entry(id: &str, start_at: Option<u64>) -> QueueEntry {
        QueueEntry {
            id: id.into(),
            url: format!("http://h/{id}"),
            save_path: None,
            dir: PathBuf::from("."),
            file_name: format!("{id}.bin"),
            connections: 4,
            speed_limit: 0,
            verify: false,
            expected_checksum: None,
            headers: vec![],
            mirrors: vec![],
            is_stream: false,
            start_at,
            priority: Priority::Normal,
            held: false,
            name_hint: None,
            manifest: false,
            quality: None,
        }
    }

    fn with_priority(id: &str, p: Priority) -> QueueEntry {
        QueueEntry {
            priority: p,
            ..entry(id, None)
        }
    }

    fn order(h: &RecordingHost, ids: &[&str]) -> Vec<String> {
        let mut v: Vec<(usize, String)> = ids
            .iter()
            .map(|id| (h.last_status(id).unwrap().position, id.to_string()))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    const NOW: u64 = 1_000_000;

    #[test]
    fn respects_cap_and_frees_slots_in_fifo_order() {
        let (q, h) = (QueueManager::new(2), RecordingHost::default());
        for id in ["a", "b", "c", "d"] {
            q.enqueue(&h, entry(id, None), NOW);
        }
        assert_eq!(h.launched(), ["a", "b"]);
        assert_eq!(q.running(), 2);
        let c = h.last_status("c").unwrap();
        assert_eq!((c.status.as_str(), c.position), ("queued", 1));
        assert_eq!(h.last_status("d").unwrap().position, 2);

        q.finished(&h, NOW);
        assert_eq!(h.launched(), ["a", "b", "c"]);
        assert_eq!(
            h.last_status("d").unwrap().position,
            1,
            "positions renumbered"
        );
    }

    #[test]
    fn raising_the_cap_starts_more() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("a", None), NOW);
        q.enqueue(&h, entry("b", None), NOW);
        assert_eq!(h.launched(), ["a"]);
        q.set_max_concurrent(5);
        q.promote(&h, NOW);
        assert_eq!(h.launched(), ["a", "b"]);
    }

    #[test]
    fn future_schedule_waits_arms_timer_and_does_not_block_others() {
        let (q, h) = (QueueManager::new(3), RecordingHost::default());
        q.enqueue(&h, entry("later", Some(NOW + 60)), NOW);
        q.enqueue(&h, entry("now", None), NOW);
        assert_eq!(h.launched(), ["now"], "a scheduled entry must not block");
        assert_eq!(*h.timers.lock().unwrap(), [NOW + 60]);
        let s = h.last_status("later").unwrap();
        assert_eq!(
            (s.status.as_str(), s.start_at),
            ("scheduled", Some(NOW + 60))
        );

        q.promote(&h, NOW + 59);
        assert_eq!(h.launched(), ["now"], "not before its time");
        q.promote(&h, NOW + 60); // the timer fires
        assert_eq!(h.launched(), ["now", "later"]);
    }

    #[test]
    fn past_due_schedule_starts_per_queue_rules() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("busy", None), NOW);
        q.enqueue(&h, entry("overdue", Some(NOW - 3600)), NOW);
        assert!(
            h.timers.lock().unwrap().is_empty(),
            "no timer for past times"
        );
        assert_eq!(h.launched(), ["busy"], "still subject to the cap");
        assert_eq!(h.last_status("overdue").unwrap().status, "queued");
        q.finished(&h, NOW);
        assert_eq!(h.launched(), ["busy", "overdue"]);
    }

    #[test]
    fn cancel_removes_only_waiting_entries() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("run", None), NOW);
        q.enqueue(&h, entry("wait1", None), NOW);
        q.enqueue(&h, entry("wait2", None), NOW);

        assert!(
            q.cancel_pending(&h, "run", NOW).is_none(),
            "running is not pending"
        );
        let removed = q.cancel_pending(&h, "wait1", NOW).expect("was waiting");
        assert_eq!(removed.id, "wait1");
        assert_eq!(h.last_status("wait1").unwrap().status, "canceled");
        assert_eq!(h.last_status("wait2").unwrap().position, 1);
        assert!(!q.is_pending("wait1"));

        q.finished(&h, NOW);
        assert_eq!(
            h.launched(),
            ["run", "wait2"],
            "canceled entry never launches"
        );
    }

    #[test]
    fn higher_priority_starts_first_fifo_within_band() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("busy", None), NOW);
        q.enqueue(&h, with_priority("n1", Priority::Normal), NOW);
        q.enqueue(&h, with_priority("low", Priority::Low), NOW);
        q.enqueue(&h, with_priority("hi", Priority::High), NOW);
        q.enqueue(&h, with_priority("n2", Priority::Normal), NOW);
        assert_eq!(
            order(&h, &["n1", "low", "hi", "n2"]),
            ["hi", "n1", "n2", "low"]
        );
        let hi = h.last_status("hi").unwrap();
        assert_eq!((hi.priority, hi.position), (Priority::High, 1));
        for _ in 0..4 {
            q.finished(&h, NOW);
        }
        assert_eq!(h.launched(), ["busy", "hi", "n1", "n2", "low"]);
    }

    #[test]
    fn held_entries_keep_their_place_but_never_start() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("busy", None), NOW);
        q.enqueue(&h, entry("a", None), NOW);
        q.enqueue(&h, entry("b", None), NOW);
        assert!(q.set_held(&h, "a", true, NOW));
        let a = h.last_status("a").unwrap();
        assert_eq!((a.status.as_str(), a.held, a.position), ("held", true, 1));

        q.finished(&h, NOW);
        assert_eq!(
            h.launched(),
            ["busy", "b"],
            "held entry skipped, not removed"
        );
        assert!(q.is_pending("a"));

        assert!(q.set_held(&h, "a", false, NOW));
        assert_eq!(h.launched(), ["busy", "b"], "still capped");
        q.finished(&h, NOW);
        assert_eq!(h.launched(), ["busy", "b", "a"]);
        assert!(
            !q.set_held(&h, "a", true, NOW),
            "running entries aren't held"
        );
    }

    #[test]
    fn releasing_a_hold_starts_it_if_a_slot_is_free() {
        let (q, h) = (QueueManager::new(2), RecordingHost::default());
        q.enqueue(&h, entry("busy", None), NOW);
        q.set_max_concurrent(1);
        q.enqueue(&h, entry("a", None), NOW);
        q.set_held(&h, "a", true, NOW);
        q.set_max_concurrent(2);
        q.promote(&h, NOW);
        assert_eq!(h.launched(), ["busy"]);
        q.set_held(&h, "a", false, NOW);
        assert_eq!(h.launched(), ["busy", "a"]);
    }

    #[test]
    fn move_up_down_stays_inside_the_priority_band() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.enqueue(&h, entry("busy", None), NOW);
        q.enqueue(&h, with_priority("hi", Priority::High), NOW);
        for id in ["a", "b", "c"] {
            q.enqueue(&h, entry(id, None), NOW);
        }
        let ids = ["hi", "a", "b", "c"];
        assert!(q.move_by(&h, "c", -1, NOW));
        assert_eq!(order(&h, &ids), ["hi", "a", "c", "b"]);
        q.move_by(&h, "c", -10, NOW); // clamps at the top of its band
        assert_eq!(order(&h, &ids), ["hi", "c", "a", "b"]);
        q.move_by(&h, "c", 1, NOW);
        assert_eq!(order(&h, &ids), ["hi", "a", "c", "b"]);
        assert!(!q.move_by(&h, "busy", 1, NOW), "running entries can't move");

        // Crossing bands goes through priority: it joins the end of the band.
        assert!(q.set_priority(&h, "b", Priority::High, NOW));
        assert_eq!(order(&h, &ids), ["hi", "b", "a", "c"]);
        q.set_priority(&h, "hi", Priority::Low, NOW);
        assert_eq!(order(&h, &ids), ["b", "a", "c", "hi"]);
    }

    #[test]
    fn finished_never_underflows() {
        let (q, h) = (QueueManager::new(1), RecordingHost::default());
        q.finished(&h, NOW);
        assert_eq!(q.running(), 0);
    }
}
