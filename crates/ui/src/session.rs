//! The desktop layout's client half of a design session. This crate starts with the part the
//! first M4 attempt got wrong: which keyed changes are still unsettled, and what to do about
//! them across a reload or a lost connection. The window that uses it comes next.
//!
//! The rules are the spec's, not the browser's (spec §7, §13):
//!
//! - A change is sent once under a key the host minted from real randomness; this model refuses
//!   to record the same key twice, so a counter that restarts at zero cannot replay an accepted
//!   key as a different change.
//! - A reply — applied or refused — settles its key for good. Refusals are durable answers
//!   (M3's refused Start results), so they are kept exactly like applied ones.
//! - After a reconnect or a reload, nothing is sent again blindly: every still-pending change is
//!   looked up by its key first, and only a lookup that says *not applied* may be re-sent, with
//!   the same key and the same body.
//! - A new incarnation voids everything pending: the project was reopened, and the worker would
//!   refuse those changes anyway.

use std::collections::VecDeque;

use cutcraft_web_protocol::{CommandReply, CommandRequest, KeyLookup, SessionSnapshot};

/// How many settled answers are kept, newest last, for restoring a dialog after a reload.
const KEPT_ANSWERS: usize = 32;

/// The client's record of one design session: what was sent, what answered, what is still
/// uncertain. Pure state and rules — transport, keys and timing belong to the host.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    incarnation: String,
    revision: u64,
    /// Changes sent and not yet answered, oldest first.
    pending: Vec<CommandRequest>,
    /// The most recent answers, oldest first, with the request that asked them.
    answers: VecDeque<(CommandRequest, CommandReply)>,
}

/// What a lookup said about one pending change (spec §13).
#[derive(Clone, Debug, PartialEq)]
pub enum Reconciled {
    /// It was applied after all; the answer is now held like any other.
    Applied(CommandReply),
    /// It was refused after all; the answer is now held like any other.
    Refused(CommandReply),
    /// It never landed: the same key and body may be sent again.
    NotApplied,
    /// No such change was pending — it was answered before, or belongs to another session.
    Unknown,
}

/// The host tried to record a key that is already in flight or already answered. This is the
/// key-collision bug class: the key is not random, or it was minted twice. Show it, and do not
/// send the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCollision;

impl Session {
    /// A session as a fresh snapshot finds it: nothing sent, nothing pending.
    pub fn new(incarnation: impl Into<String>, revision: u64) -> Self {
        Self {
            incarnation: incarnation.into(),
            revision,
            pending: Vec::new(),
            answers: VecDeque::new(),
        }
    }

    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Adopt a snapshot the host received (pushed or fetched). The revision always moves; a new
    /// incarnation additionally voids everything pending, because the project was reopened
    /// (spec §7). Settled answers survive it: they did happen.
    pub fn adopt(&mut self, snapshot: &SessionSnapshot) {
        self.revision = snapshot.revision;
        if self.incarnation != snapshot.incarnation {
            self.incarnation = snapshot.incarnation.clone();
            self.pending.clear();
        }
    }

    /// Record a change as sent. The host minted the key; recording the same key twice — pending
    /// or answered — is refused, so a replayed key can never be mistaken for a new change.
    pub fn sent(&mut self, request: CommandRequest) -> Result<(), KeyCollision> {
        let key = request.key.as_str();
        if self.pending.iter().any(|p| p.key == key)
            || self.answers.iter().any(|(r, _)| r.key == key)
        {
            return Err(KeyCollision);
        }
        self.pending.push(request);
        Ok(())
    }

    /// The changes still uncertain, oldest first: what to look up by key after a reconnect or a
    /// reload — never to be sent again blindly (spec §13).
    pub fn uncertain(&self) -> &[CommandRequest] {
        &self.pending
    }

    /// The answer a key already holds, if any: what a reloaded dialog shows instead of asking
    /// again.
    pub fn answer(&self, key: &str) -> Option<&CommandReply> {
        self.answers
            .iter()
            .find(|(request, _)| request.key == key)
            .map(|(_, reply)| reply)
    }

    /// A reply arrived. Settles the key — applied or refused, both are final — and returns the
    /// answer now held; `None` when nothing was pending under that key (a duplicate delivery, or
    /// an answer this session already holds).
    pub fn settled(&mut self, key: &str, reply: CommandReply) -> Option<CommandReply> {
        let Some(at) = self.pending.iter().position(|p| p.key == key) else {
            return self
                .answers
                .iter()
                .find(|(r, _)| r.key == key)
                .map(|(_, a)| a.clone());
        };
        let request = self.pending.remove(at);
        self.hold(request, reply)
    }

    /// Fold one lookup result into the session, after a reconnect or a reload. `NotApplied`
    /// leaves the change pending, free to be sent again under the same key; an applied or
    /// refused result settles it; a key nothing is pending under is `Unknown`.
    pub fn reconciled(&mut self, key: &str, found: KeyLookup) -> Reconciled {
        let reply = match found {
            KeyLookup::Applied { revision, value } => CommandReply::Applied { revision, value },
            KeyLookup::Refused { code, message } => CommandReply::Refused { code, message },
            KeyLookup::NotApplied => {
                // Still pending, free to be sent again; a key nothing is pending under is
                // unknown, whatever the lookup said.
                return if self.pending.iter().any(|p| p.key == key) {
                    Reconciled::NotApplied
                } else {
                    Reconciled::Unknown
                };
            }
        };
        let Some(at) = self.pending.iter().position(|p| p.key == key) else {
            return Reconciled::Unknown;
        };
        let request = self.pending.remove(at);
        match self.hold(request, reply) {
            Some(reply @ CommandReply::Applied { .. }) => Reconciled::Applied(reply),
            Some(reply @ CommandReply::Refused { .. }) => Reconciled::Refused(reply),
            _ => unreachable!("an answer is always held"),
        }
    }

    /// Move the session into a checkpoint the host stores (localStorage in the browser), so a
    /// reload looks its pending changes up instead of minting keys again.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            schema_version: 1,
            incarnation: self.incarnation.clone(),
            revision: self.revision,
            pending: self.pending.clone(),
            answers: self.answers.iter().cloned().collect(),
        }
    }

    /// Restore a checkpoint against the snapshot the reloaded host just fetched. A checkpoint
    /// from the same incarnation resumes exactly where it left off, pending and answers intact;
    /// one from any other incarnation — a reopen happened in between — keeps only its answers.
    pub fn restore(checkpoint: Checkpoint, snapshot: &SessionSnapshot) -> Self {
        let same = checkpoint.incarnation == snapshot.incarnation;
        let mut session = Self::new(snapshot.incarnation.clone(), snapshot.revision);
        if same {
            session.pending = checkpoint.pending;
            // The snapshot may be newer than the checkpoint; the pending keys are looked up
            // either way (spec §13).
            session.revision = snapshot.revision;
        }
        for (request, reply) in checkpoint.answers {
            session.answers.push_back((request, reply));
        }
        session
    }

    /// Hold an answer, bounded, newest last.
    fn hold(&mut self, request: CommandRequest, reply: CommandReply) -> Option<CommandReply> {
        let held = reply.clone();
        self.answers.push_back((request, reply));
        while self.answers.len() > KEPT_ANSWERS {
            self.answers.pop_front();
        }
        Some(held)
    }
}

/// What a reload needs to continue a session honestly: the incarnation and revision it stopped
/// at, the changes still uncertain, and the most recent answers.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    schema_version: u32,
    incarnation: String,
    revision: u64,
    pending: Vec<CommandRequest>,
    answers: Vec<(CommandRequest, CommandReply)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_web_protocol::PROTOCOL_VERSION;
    use serde_json::json;

    fn request(key: &str, revision: u64) -> CommandRequest {
        CommandRequest {
            id: "sheet.place".into(),
            params: json!({"x": 12.0, "y": 30.0}),
            key: key.into(),
            incarnation: "i1".into(),
            revision: Some(revision),
        }
    }

    fn snapshot(incarnation: &str, revision: u64) -> SessionSnapshot {
        SessionSnapshot {
            protocol: PROTOCOL_VERSION,
            incarnation: incarnation.into(),
            revision,
            ..SessionSnapshot::default()
        }
    }

    #[test]
    fn a_change_settles_once_and_twice_more_changes_nothing() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k1", 3)).unwrap();
        assert_eq!(s.uncertain().len(), 1);
        let reply = CommandReply::Applied {
            revision: 4,
            value: json!({"placed": true}),
        };
        assert_eq!(s.settled("k1", reply.clone()), Some(reply.clone()));
        assert!(s.uncertain().is_empty());
        // A duplicate delivery of the same answer holds; it does not re-settle.
        assert_eq!(s.settled("k1", reply.clone()), Some(reply));
        assert_eq!(
            s.answer("k1"),
            Some(&CommandReply::Applied {
                revision: 4,
                value: json!({"placed": true}),
            })
        );
    }

    #[test]
    fn the_same_key_twice_is_refused_whatever_the_body() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k1", 3)).unwrap();
        assert_eq!(s.sent(request("k1", 3)), Err(KeyCollision));
        s.settled(
            "k1",
            CommandReply::Refused {
                code: "stale-revision".into(),
                message: "This project changed since.".into(),
            },
        );
        // After the answer, too: an accepted key is never reused as a new change.
        assert_eq!(s.sent(request("k1", 5)), Err(KeyCollision));
        // A fresh key, meanwhile, is fine.
        assert!(s.sent(request("k2", 5)).is_ok());
    }

    #[test]
    fn a_refusal_is_an_answer_not_a_loss() {
        let mut s = Session::new("i1", 2);
        s.sent(request("save", 2)).unwrap();
        let refused = CommandReply::Refused {
            code: "not-saved-yet".into(),
            message: "Choose a folder and a name (Save As).".into(),
        };
        assert_eq!(s.settled("save", refused.clone()), Some(refused.clone()));
        assert_eq!(s.answer("save"), Some(&refused));
        assert!(s.uncertain().is_empty());
    }

    #[test]
    fn a_new_incarnation_voids_pending_and_keeps_answers() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k0", 2)).unwrap();
        s.settled(
            "k0",
            CommandReply::Applied {
                revision: 3,
                value: json!({}),
            },
        )
        .unwrap();
        s.sent(request("k1", 3)).unwrap();
        s.adopt(&snapshot("i2", 1));
        assert_eq!(s.incarnation(), "i2");
        assert_eq!(s.revision(), 1);
        assert!(s.uncertain().is_empty());
        assert!(s.answer("k0").is_some());
    }

    #[test]
    fn the_same_incarnation_keeps_pending_through_a_snapshot() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k1", 3)).unwrap();
        s.adopt(&snapshot("i1", 7));
        assert_eq!(s.revision(), 7);
        assert_eq!(s.uncertain().len(), 1);
    }

    #[test]
    fn a_lookup_decides_and_only_not_applied_may_be_sent_again() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k1", 3)).unwrap();
        s.sent(request("k2", 3)).unwrap();
        // One landed after all.
        assert_eq!(
            s.reconciled(
                "k1",
                KeyLookup::Applied {
                    revision: 4,
                    value: json!({"placed": true}),
                }
            ),
            Reconciled::Applied(CommandReply::Applied {
                revision: 4,
                value: json!({"placed": true}),
            })
        );
        // One was refused elsewhere (a durable refusal).
        assert_eq!(
            s.reconciled(
                "k2",
                KeyLookup::Refused {
                    code: "stale-revision".into(),
                    message: "This project changed since.".into(),
                }
            ),
            Reconciled::Refused(CommandReply::Refused {
                code: "stale-revision".into(),
                message: "This project changed since.".into(),
            })
        );
        assert!(s.uncertain().is_empty());
        // One never landed: still pending, and the same key may carry it again.
        s.sent(request("k3", 3)).unwrap();
        assert_eq!(
            s.reconciled("k3", KeyLookup::NotApplied),
            Reconciled::NotApplied
        );
        assert_eq!(s.uncertain()[0].key, "k3");
        // A key from nowhere is unknown, and settles nothing.
        assert_eq!(
            s.reconciled("nope", KeyLookup::NotApplied),
            Reconciled::Unknown
        );
    }

    #[test]
    fn a_checkpoint_round_trips_and_resumes_the_same_session() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k0", 2)).unwrap();
        s.settled(
            "k0",
            CommandReply::Applied {
                revision: 3,
                value: json!({}),
            },
        )
        .unwrap();
        s.sent(request("k1", 3)).unwrap();
        let text = serde_json::to_string(&s.checkpoint()).unwrap();
        let back: Checkpoint = serde_json::from_str(&text).unwrap();
        let resumed = Session::restore(back, &snapshot("i1", 5));
        assert_eq!(resumed.incarnation(), "i1");
        assert_eq!(resumed.revision(), 5);
        assert_eq!(resumed.uncertain().len(), 1);
        assert_eq!(resumed.uncertain()[0].key, "k1");
        assert!(resumed.answer("k0").is_some());
    }

    #[test]
    fn a_checkpoint_from_another_incarnation_keeps_only_answers() {
        let mut s = Session::new("i1", 3);
        s.sent(request("k0", 2)).unwrap();
        s.settled(
            "k0",
            CommandReply::Applied {
                revision: 3,
                value: json!({}),
            },
        )
        .unwrap();
        s.sent(request("k1", 3)).unwrap();
        let resumed = Session::restore(s.checkpoint(), &snapshot("i9", 1));
        assert_eq!(resumed.incarnation(), "i9");
        assert!(resumed.uncertain().is_empty());
        assert!(resumed.answer("k0").is_some());
    }

    #[test]
    fn only_the_most_recent_answers_are_kept() {
        let mut s = Session::new("i1", 0);
        for n in 0..(KEPT_ANSWERS + 8) {
            let key = format!("k{n}");
            s.sent(request(&key, 0)).unwrap();
            s.settled(
                &key,
                CommandReply::Applied {
                    revision: 0,
                    value: json!({"n": n}),
                },
            )
            .unwrap();
        }
        assert!(s.answer("k0").is_none());
        assert!(s.answer(&format!("k{}", KEPT_ANSWERS + 7)).is_some());
        let checkpoint = s.checkpoint();
        assert_eq!(checkpoint.answers.len(), KEPT_ANSWERS);
    }
}
