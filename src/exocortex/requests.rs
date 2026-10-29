//! Requests in flight (PAR-025–027): every command whose answer the app
//! waits for gets a reqId here, with the area that owns it and a deadline.
//! An answer settles it; one that never comes expires into an actionable
//! error for its owner; an answer nobody waits for any more (expired,
//! superseded, from before a reconnection) is stale and dropped by the
//! owner's check (`is_pending`, or its own kept id).
//!
//! Pure bookkeeping, no I/O: `Model` holds one and routes by it.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Most commands are answered within this.
pub const DEFAULT_WITHIN: Duration = Duration::from_secs(20);
/// A conversation's window and older pages (the chat says so after it).
pub const LOAD_WITHIN: Duration = Duration::from_secs(15);
/// Stop.
pub const ABORT_WITHIN: Duration = Duration::from_secs(12);
/// A new folder.
pub const FOLDER_CREATE_WITHIN: Duration = Duration::from_secs(12);
/// Dictation's transcription.
pub const TRANSCRIPTION_WITHIN: Duration = Duration::from_secs(90);

/// The area a request belongs to, whose handler gets its answer, error or
/// timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    /// The connection itself (the bootstrap ping).
    App,
    /// The conversation list (B).
    Sidebar,
    /// The open conversation (C1, C2).
    Chat,
    /// Sending and the queue (D).
    Composer,
    /// Models, settings and the operations panels (E).
    Ops,
}

/// A request waiting for its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub owner: Owner,
    /// What it is (`load`, `older`, `send`, `abort`, …): for its owner and
    /// for messages.
    pub kind: &'static str,
    /// The conversation it is about, if one.
    pub conv: Option<String>,
    pub sent: Instant,
    pub deadline: Instant,
}

#[derive(Debug, Default)]
pub struct Requests {
    next: u64,
    pending: HashMap<String, Pending>,
}

impl Requests {
    /// A fresh reqId nobody waits on (fire-and-forget commands).
    pub fn id(&mut self) -> String {
        self.next += 1;
        format!("app-{}", self.next)
    }

    /// A fresh reqId, waited on by `owner` until `now + within`.
    pub fn issue(
        &mut self,
        owner: Owner,
        kind: &'static str,
        conv: Option<&str>,
        within: Duration,
        now: Instant,
    ) -> String {
        let id = self.id();
        self.pending.insert(
            id.clone(),
            Pending {
                owner,
                kind,
                conv: conv.map(str::to_owned),
                sent: now,
                deadline: now + within,
            },
        );
        id
    }

    /// Waits on `id` (one made elsewhere, as the bootstrap's ping).
    pub fn track(
        &mut self,
        id: &str,
        owner: Owner,
        kind: &'static str,
        within: Duration,
        now: Instant,
    ) {
        self.pending.insert(
            id.to_owned(),
            Pending {
                owner,
                kind,
                conv: None,
                sent: now,
                deadline: now + within,
            },
        );
    }

    /// Its answer arrived: no longer waited on. None when nobody waited
    /// (stale, or not ours).
    pub fn settle(&mut self, id: &str) -> Option<Pending> {
        self.pending.remove(id)
    }

    pub fn is_pending(&self, id: &str) -> bool {
        self.pending.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<&Pending> {
        self.pending.get(id)
    }

    /// The requests of `owner` and `kind` still waited on.
    pub fn of(&self, owner: Owner, kind: &str) -> impl Iterator<Item = (&String, &Pending)> {
        self.pending
            .iter()
            .filter(move |(_, p)| p.owner == owner && p.kind == kind)
    }

    /// Stops waiting on `id` (superseded) without an answer.
    pub fn forget(&mut self, id: &str) {
        self.pending.remove(id);
    }

    /// Those past their deadline, no longer waited on, oldest first.
    pub fn expire(&mut self, now: Instant) -> Vec<(String, Pending)> {
        let late: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(id, _)| id.clone())
            .collect();
        let mut expired: Vec<(String, Pending)> = late
            .into_iter()
            .filter_map(|id| self.pending.remove(&id).map(|p| (id, p)))
            .collect();
        expired.sort_by_key(|(_, p)| p.sent);
        expired
    }

    /// The soonest deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    /// The connection dropped: nothing in flight will be answered.
    pub fn clear(&mut self) -> Vec<(String, Pending)> {
        self.pending.drain().collect()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_settle_deadlines_expire_and_late_answers_are_stale() {
        let now = Instant::now();
        let mut requests = Requests::default();
        let send = requests.issue(Owner::Composer, "send", Some("c"), DEFAULT_WITHIN, now);
        let abort = requests.issue(Owner::Composer, "abort", Some("c"), ABORT_WITHIN, now);
        assert_ne!(send, abort);
        assert_eq!(requests.deadline(), Some(now + ABORT_WITHIN));
        assert!(requests.expire(now + Duration::from_secs(11)).is_empty());
        // Stop's deadline passes first.
        let expired = requests.expire(now + ABORT_WITHIN);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, abort);
        assert_eq!(expired[0].1.kind, "abort");
        // Its answer, arriving late, is stale.
        assert!(requests.settle(&abort).is_none());
        assert!(requests.is_pending(&send));
        let settled = requests.settle(&send).expect("waited on");
        assert_eq!(settled.owner, Owner::Composer);
        assert_eq!(settled.conv.as_deref(), Some("c"));
        assert!(requests.is_empty() && requests.deadline().is_none());
    }

    #[test]
    fn a_dropped_connection_clears_everything() {
        let now = Instant::now();
        let mut requests = Requests::default();
        requests.track("ping-1", Owner::App, "ping", DEFAULT_WITHIN, now);
        requests.issue(Owner::Chat, "load", Some("c"), LOAD_WITHIN, now);
        assert_eq!(requests.of(Owner::Chat, "load").count(), 1);
        assert_eq!(requests.clear().len(), 2);
        assert!(!requests.is_pending("ping-1"));
    }
}
