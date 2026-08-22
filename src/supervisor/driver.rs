//! The one place a live session handle exists.
//!
//! The driver executes the supervisor's instructions in order and holds the
//! engine they operate on. It makes no decisions of its own: every safety
//! question was already settled by the pure reconciler that produced the ops.

use std::sync::Arc;

use crate::engine::{TorrentEngine, TorrentSnapshot, TorrentState};
use crate::supervisor::factory::SessionFactory;
use crate::supervisor::state::{FailedOp, Input, SessionOp, Supervisor};

pub struct Driver {
    factory: Arc<dyn SessionFactory>,
    supervisor: Supervisor,
    engine: Option<Arc<dyn TorrentEngine>>,
}

impl Driver {
    pub fn new(factory: Arc<dyn SessionFactory>, supervisor: Supervisor) -> Self {
        Self {
            factory,
            supervisor,
            engine: None,
        }
    }

    pub fn session_is_live(&self) -> bool {
        self.engine.is_some()
    }

    /// Whether the supervisor already has this torrent among the ones it is
    /// asked to carry. The caller needs it to decide between an `Add` and a
    /// bare `Start`; keeping its own list beside this one would go stale the
    /// moment a torrent finished and left `wanted` on its own.
    pub fn knows(&self, infohash: &str) -> bool {
        self.supervisor
            .wanted()
            .iter()
            .any(|w| w.infohash == infohash)
    }

    pub async fn snapshots(&self) -> Vec<TorrentSnapshot> {
        match &self.engine {
            Some(engine) => engine.snapshots().await,
            None => Vec::new(),
        }
    }

    /// Feed one input to the supervisor and carry out what it asks for.
    /// Returns the notices the user should see.
    pub async fn handle(&mut self, input: Input) -> Vec<String> {
        let ops = self.supervisor.handle(input);
        self.execute(ops).await
    }

    /// Ask the engine what finished, and tell the supervisor. This is what
    /// enforces "never seed": a torrent that reaches `Seeding` is on its way
    /// out of the session.
    pub async fn poll_completions(&mut self) -> Vec<String> {
        let finished: Vec<String> = self
            .snapshots()
            .await
            .into_iter()
            .filter(|s| s.state == TorrentState::Seeding)
            .map(|s| s.infohash)
            .collect();
        let mut notices = Vec::new();
        for infohash in finished {
            notices.extend(self.handle(Input::Completed(infohash)).await);
        }
        notices
    }

    async fn execute(&mut self, ops: Vec<SessionOp>) -> Vec<String> {
        let mut notices = Vec::new();
        for op in ops {
            match op {
                SessionOp::Notice(message) => notices.push(message),
                SessionOp::Kill { .. } => {
                    // Dropping the last handle (rather than calling
                    // `Session::stop()`) is deliberate: it discards any
                    // fastresume progress the engine might otherwise have
                    // saved, but a kill switch has to be the one thing we
                    // can be sure works, so we never wait on a graceful stop.
                    self.engine = None;
                }
                SessionOp::Build { device } => match self.factory.build(device.as_deref()).await {
                    Ok(engine) => self.engine = Some(engine),
                    Err(e) => {
                        self.engine = None;
                        // Re-entering the supervisor here is safe: `SessionFailed`
                        // produces only a notice and never another build.
                        let more = Box::pin(self.handle(Input::SessionFailed(e.to_string()))).await;
                        notices.extend(more);
                        return notices;
                    }
                },
                SessionOp::AddTorrent {
                    infohash,
                    magnet,
                    dir,
                    paused,
                } => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.add_magnet(&magnet, &dir, paused).await {
                        notices.push(format!("Could not start that download ({e})"));
                        // Tell the supervisor the engine does not actually
                        // have it, so it stops believing otherwise. `Add`
                        // never emits another op that can itself fail, so
                        // this cannot recurse further.
                        let more = Box::pin(self.handle(Input::OpFailed {
                            infohash,
                            kind: FailedOp::Add,
                        }))
                        .await;
                        notices.extend(more);
                    }
                }
                SessionOp::PauseTorrent(infohash) => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.pause(&infohash).await {
                        notices.push(format!("Could not pause that download ({e})"));
                        // A pause we cannot confirm took effect must fail
                        // closed: this feeds back to a `Kill`, which cannot
                        // itself fail, so the recursion is bounded.
                        let more = Box::pin(self.handle(Input::OpFailed {
                            infohash,
                            kind: FailedOp::Pause,
                        }))
                        .await;
                        notices.extend(more);
                    }
                }
                SessionOp::ResumeTorrent(infohash) => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.resume(&infohash).await {
                        notices.push(format!("Could not resume that download ({e})"));
                        // `Resume` only ever updates our own record and emits
                        // no ops, so this cannot recurse further either.
                        let more = Box::pin(self.handle(Input::OpFailed {
                            infohash,
                            kind: FailedOp::Resume,
                        }))
                        .await;
                        notices.extend(more);
                    }
                }
                SessionOp::RemoveTorrent {
                    infohash,
                    delete_files,
                } => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.remove(&infohash, delete_files).await {
                        notices.push(format!("Could not remove that download ({e})"));
                        // Same reasoning as the failed pause: fail closed via
                        // `Kill`, which cannot itself fail.
                        let more = Box::pin(self.handle(Input::OpFailed {
                            infohash,
                            kind: FailedOp::Remove,
                        }))
                        .await;
                        notices.extend(more);
                    }
                }
            }
        }
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::factory::FakeFactory;
    use crate::supervisor::state::{Intent, Wanted};
    use crate::vpn::guard::GuardAction;
    use crate::vpn::policy::BindSupport;

    fn wanted(infohash: &str) -> Wanted {
        Wanted {
            infohash: infohash.to_string(),
            magnet: format!("magnet:?xt=urn:btih:{infohash}"),
            dir: std::path::PathBuf::from("/downloads"),
            paused: false,
        }
    }

    fn driver(factory: Arc<FakeFactory>) -> Driver {
        Driver::new(factory, Supervisor::new(BindSupport::Supported))
    }

    #[tokio::test]
    async fn a_protected_add_reaches_the_engine() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        d.handle(Input::User(Intent::Add(wanted(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))))
        .await;
        assert_eq!(f.built_devices(), vec![Some("utun4".to_string())]);
        assert_eq!(
            f.engine().added_magnets(),
            vec!["magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()]
        );
        assert!(d.session_is_live());
    }

    #[tokio::test]
    async fn an_unprotected_add_never_builds_a_session() {
        // The machine-checked form of the hard rule at the driver layer.
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::User(Intent::Add(wanted(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))))
        .await;
        assert!(
            f.built_devices().is_empty(),
            "no session may be built without a tunnel"
        );
        assert!(f.engine().added_magnets().is_empty());
        assert!(!d.session_is_live());
    }

    #[tokio::test]
    async fn a_dropped_tunnel_drops_the_engine() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        d.handle(Input::User(Intent::Add(wanted(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))))
        .await;
        assert!(d.session_is_live());
        d.handle(Input::Vpn(GuardAction::PauseAll {
            reason: "tunnel gone".into(),
        }))
        .await;
        assert!(
            !d.session_is_live(),
            "the engine must be dropped, not merely paused"
        );
        assert!(
            d.snapshots().await.is_empty(),
            "a dead session reports nothing"
        );
    }

    #[tokio::test]
    async fn a_failed_build_is_reported_and_leaves_no_session() {
        let f = Arc::new(FakeFactory::new());
        f.fail_next("could not open a session");
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        let notices = d
            .handle(Input::User(Intent::Add(wanted(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ))))
            .await;
        assert!(
            notices
                .iter()
                .any(|n| n.contains("could not open a session")),
            "the failure must reach the user: {notices:?}"
        );
        assert!(!d.session_is_live());
    }

    #[tokio::test]
    async fn a_finished_torrent_is_removed_from_the_session() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        d.handle(Input::User(Intent::Add(wanted(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))))
        .await;
        let infohash = f.engine().snapshots().await[0].infohash.clone();
        f.engine().finish(&infohash);
        d.poll_completions().await;
        assert!(
            f.engine().snapshots().await.is_empty(),
            "a completed torrent must leave the session rather than seed"
        );
        assert!(
            !d.session_is_live(),
            "with nothing left, the session closes"
        );
    }

    #[tokio::test]
    async fn snapshots_come_from_the_live_engine() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        d.handle(Input::User(Intent::Add(wanted(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))))
        .await;
        let infohash = f.engine().snapshots().await[0].infohash.clone();
        f.engine().set_progress(&infohash, 512, 1024);
        let snaps = d.snapshots().await;
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].progress_bytes, 512);
        assert_eq!(snaps[0].total_bytes, 1024);
    }

    const AA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BB: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[tokio::test]
    async fn a_failed_add_is_retried_once_the_supervisor_reconciles_again() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        d.handle(Input::User(Intent::Add(wanted(AA)))).await;

        f.engine().fail_next_add("disk on fire");
        let notices = d.handle(Input::User(Intent::Add(wanted(BB)))).await;
        assert!(
            notices.iter().any(|n| n.contains("disk on fire")),
            "the failure must reach the user: {notices:?}"
        );
        assert_eq!(
            f.engine().added_magnets(),
            vec![format!("magnet:?xt=urn:btih:{AA}")],
            "bb's add failed and must not appear as added"
        );

        // An unrelated intent (pausing aa, which stays active because bb is
        // still wanted) forces a reconcile with no kill involved. If the
        // supervisor still believed bb was in the session, this would add
        // nothing further; the fix is what makes it retry bb.
        d.handle(Input::User(Intent::Pause(AA.to_string()))).await;
        assert_eq!(
            f.engine().added_magnets(),
            vec![
                format!("magnet:?xt=urn:btih:{AA}"),
                format!("magnet:?xt=urn:btih:{BB}"),
            ],
            "the supervisor must retry the add it thinks never landed"
        );
    }

    #[tokio::test]
    async fn a_failed_pause_kills_the_session() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        // Two active torrents, so pausing one does not, by itself, empty the
        // "wanted" set and trigger the ordinary nothing-left-to-download
        // kill; any kill we see must come from the failed-pause handling.
        d.handle(Input::User(Intent::Add(wanted(AA)))).await;
        d.handle(Input::User(Intent::Add(wanted(BB)))).await;
        assert!(d.session_is_live());

        f.engine().fail_next_pause("backend wedged");
        let notices = d.handle(Input::User(Intent::Pause(AA.to_string()))).await;
        assert!(
            notices.iter().any(|n| n.contains("backend wedged")),
            "the failure must reach the user: {notices:?}"
        );
        assert!(
            !d.session_is_live(),
            "a pause we cannot confirm must fail closed by killing the session"
        );
    }

    #[tokio::test]
    async fn a_failed_remove_kills_the_session() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        // Two active torrents, same reasoning as the failed-pause test: bb
        // stays wanted, so removing aa alone would not naturally kill the
        // session unless the failed-remove handling does it explicitly.
        d.handle(Input::User(Intent::Add(wanted(AA)))).await;
        d.handle(Input::User(Intent::Add(wanted(BB)))).await;
        assert!(d.session_is_live());

        f.engine().fail_next_remove("backend wedged");
        let notices = d
            .handle(Input::User(Intent::Remove {
                infohash: AA.to_string(),
                delete_files: false,
            }))
            .await;
        assert!(
            notices.iter().any(|n| n.contains("backend wedged")),
            "the failure must reach the user: {notices:?}"
        );
        assert!(
            !d.session_is_live(),
            "a remove we cannot confirm must fail closed by killing the session"
        );
    }

    #[tokio::test]
    async fn a_failed_resume_marks_the_entry_paused_again() {
        let f = Arc::new(FakeFactory::new());
        let mut d = driver(Arc::clone(&f));
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        // A second, unrelated active torrent keeps the session alive while
        // aa is paused, so the resume path (not a rebuild from scratch) is
        // what actually gets exercised.
        d.handle(Input::User(Intent::Add(wanted(AA)))).await;
        d.handle(Input::User(Intent::Add(wanted(BB)))).await;
        d.handle(Input::User(Intent::Pause(AA.to_string()))).await;

        f.engine().fail_next_resume("backend wedged");
        let notices = d.handle(Input::User(Intent::Start(AA.to_string()))).await;
        assert!(
            notices.iter().any(|n| n.contains("backend wedged")),
            "the failure must reach the user: {notices:?}"
        );

        // Kill and rebuild the session; if the record still says paused (as
        // it must, since the resume never actually took effect), the
        // rebuild re-adds it paused rather than downloading.
        d.handle(Input::Vpn(GuardAction::PauseAll {
            reason: "gone".into(),
        }))
        .await;
        d.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }))
        .await;
        let snaps = d.snapshots().await;
        let aa_snap = snaps
            .iter()
            .find(|s| s.infohash == AA)
            .expect("aa must still be wanted and rebuilt");
        assert_eq!(
            aa_snap.state,
            TorrentState::Paused,
            "a failed resume must leave the entry paused so a rebuild does not resume it"
        );
    }
}
