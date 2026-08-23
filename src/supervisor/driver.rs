//! The one place a live session handle exists.
//!
//! The driver executes the supervisor's instructions in order and holds the
//! engine they operate on. It makes no decisions of its own: every safety
//! question was already settled by the pure reconciler that produced the ops.

use std::sync::Arc;

use crate::engine::{TorrentEngine, TorrentSnapshot, TorrentState};
use crate::supervisor::factory::SessionFactory;
use crate::supervisor::state::{FailedOp, Input, SessionOp, Supervisor};

/// How long one engine call gets before we stop waiting on it.
///
/// The bound is per call, not per batch. A tunnel coming back reconciles into
/// a build plus one add per wanted row, and several slow-but-working adds
/// sharing a single budget would mean the budget shrinks as the queue grows,
/// and one slow magnet takes every other transfer down with it. Failing closed
/// is for the cases where we cannot tell what the engine is doing, not for
/// ordinary slowness.
mod limits {
    use std::time::Duration;

    /// Magnet resolution walks the DHT and waits on peers; a thinly-seeded
    /// torrent legitimately takes tens of seconds. Generous, because firing
    /// early drops a download that was about to work, while firing late costs
    /// only that one torrent's place in the session.
    pub const ADD: Duration = Duration::from_secs(60);

    /// Building a session is local work: binding a socket and opening the
    /// session directory. A half-built session is not something to keep, so
    /// this one still escalates.
    pub const BUILD: Duration = Duration::from_secs(30);

    /// Pausing, resuming and removing act on a torrent the session already
    /// holds. Short, because these are the calls whose failure means we can no
    /// longer be sure traffic has stopped.
    pub const CONTROL: Duration = Duration::from_secs(10);
}

/// Await one engine call with a deadline, turning a miss into the same error
/// the engine itself would have reported. That routes it into the `OpFailed`
/// feedback the supervisor already understands: a slow add becomes that
/// torrent's problem alone, while a pause or remove that does not answer still
/// fails closed and kills the session, because failing to stop traffic is the
/// one case where teardown is the only answer we trust.
async fn bounded<T, F>(limit: std::time::Duration, call: F) -> Result<T, crate::engine::EngineError>
where
    F: std::future::Future<Output = Result<T, crate::engine::EngineError>>,
{
    match tokio::time::timeout(limit, call).await {
        Ok(result) => result,
        Err(_) => Err(crate::engine::EngineError::Backend(format!(
            "no answer within {}s",
            limit.as_secs()
        ))),
    }
}

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

    /// Tear the session down because an engine call did not answer.
    ///
    /// Dropping the handle is what actually stops it. `Input::SessionFailed`
    /// on its own would not: it sets the supervisor to `Down` and produces a
    /// notice, but leaves this engine field `Some`, so the session would keep
    /// running while the supervisor believed it dead — worse than either state
    /// on its own. So the drop comes first, and the supervisor is told after,
    /// when its view already matches reality.
    pub async fn force_kill(&mut self, reason: &str) -> Vec<String> {
        // Same load-bearing assumption as the `Kill` op below: the transfer
        // stops because this was the last `Arc<dyn TorrentEngine>` and
        // librqbit ends its own tasks when the session handle goes. Untestable
        // under the hard rule; see the longer note at that site.
        self.engine = None;
        self.handle(Input::SessionFailed(reason.to_string())).await
    }

    /// Which torrents the engine says have finished. A read and nothing else:
    /// `&self`, no supervisor call, no engine instruction.
    ///
    /// The split is deliberate and it is safety-critical. Detection and
    /// mutation used to happen together here, under whatever bound the caller
    /// put around the pair. `Supervisor::handle(Input::Completed)` forgets a
    /// torrent synchronously, before the removal it asks for has been carried
    /// out, so a bound that cancelled the pair mid-way consumed the completion
    /// and left the engine seeding a torrent nobody believed was there — and
    /// the next tick would report it finished again to a supervisor that no
    /// longer knew it, emitting no removal at all. Handing each infohash back
    /// to the caller means the completion travels the same fail-closed path as
    /// every other input: a removal that does not answer becomes
    /// `FailedOp::Remove`, which kills the session.
    pub async fn finished(&self) -> Vec<String> {
        self.snapshots()
            .await
            .into_iter()
            .filter(|s| s.state == TorrentState::Seeding)
            .map(|s| s.infohash)
            .collect()
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
                    //
                    // LOAD-BEARING ASSUMPTION, and the one thing here that no
                    // test can check: this stops the transfer only if dropping
                    // the last `Arc<dyn TorrentEngine>` really does end
                    // librqbit's internal tasks. Verifying it would mean
                    // running a live session, which the project's hard rule
                    // forbids, so it rests on librqbit's ownership model and
                    // on a maintainer confirming it by hand. If that ever
                    // stops holding, every kill switch in this crate stops
                    // working and nothing here will say so.
                    self.engine = None;
                }
                SessionOp::Build { device } => {
                    match bounded(limits::BUILD, self.factory.build(device.as_deref())).await {
                        Ok(engine) => self.engine = Some(engine),
                        Err(e) => {
                            self.engine = None;
                            // Re-entering the supervisor here is safe: `SessionFailed`
                            // produces only a notice and never another build.
                            let more =
                                Box::pin(self.handle(Input::SessionFailed(e.to_string()))).await;
                            notices.extend(more);
                            return notices;
                        }
                    }
                }
                SessionOp::AddTorrent {
                    infohash,
                    magnet,
                    dir,
                    paused,
                } => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) =
                        bounded(limits::ADD, engine.add_magnet(&magnet, &dir, paused)).await
                    {
                        // Named, not "that download": a batch can carry several
                        // adds, and a user cannot act on a failure they cannot
                        // attribute to a row.
                        notices.push(format!("Could not start {infohash} ({e})"));
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
                    if let Err(e) = bounded(limits::CONTROL, engine.pause(&infohash)).await {
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
                    if let Err(e) = bounded(limits::CONTROL, engine.resume(&infohash)).await {
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
                    if let Err(e) =
                        bounded(limits::CONTROL, engine.remove(&infohash, delete_files)).await
                    {
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
    async fn a_forced_kill_drops_the_engine_before_telling_the_supervisor() {
        // `SessionFailed` alone would leave the handle alive: the supervisor
        // would believe the session dead while it kept transferring, which is
        // worse than either state on its own.
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

        let notices = d.force_kill("the backend stopped answering").await;
        assert!(!d.session_is_live(), "the handle must be dropped");
        assert!(
            notices.iter().any(|n| n.contains("stopped answering")),
            "the reason must reach the user: {notices:?}"
        );
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
        for infohash in d.finished().await {
            d.handle(Input::Completed(infohash)).await;
        }
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

    /// An engine whose `add_magnet` never answers for one particular magnet
    /// and behaves normally for every other call. Magnet resolution over DHT
    /// on a thinly-seeded torrent really is slow, so this is the ordinary case
    /// going long, not a broken backend. Opens nothing.
    struct SlowAddEngine {
        slow: String,
        added: std::sync::Mutex<Vec<String>>,
    }

    impl SlowAddEngine {
        fn new(slow: &str) -> Self {
            Self {
                slow: slow.to_string(),
                added: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn added(&self) -> Vec<String> {
            self.added.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    #[async_trait::async_trait]
    impl TorrentEngine for SlowAddEngine {
        async fn add_magnet(
            &self,
            magnet: &str,
            _output_dir: &std::path::Path,
            _paused: bool,
        ) -> Result<crate::engine::InfoHash, crate::engine::EngineError> {
            if magnet.contains(&self.slow) {
                std::future::pending::<()>().await;
            }
            self.added
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(magnet.to_string());
            Ok(magnet.to_string())
        }
        async fn pause(&self, _infohash: &str) -> Result<(), crate::engine::EngineError> {
            Ok(())
        }
        async fn resume(&self, _infohash: &str) -> Result<(), crate::engine::EngineError> {
            Ok(())
        }
        async fn remove(
            &self,
            _infohash: &str,
            _delete_files: bool,
        ) -> Result<(), crate::engine::EngineError> {
            Ok(())
        }
        async fn snapshot(
            &self,
            _infohash: &str,
        ) -> Result<TorrentSnapshot, crate::engine::EngineError> {
            Err(crate::engine::EngineError::NotFound("none".into()))
        }
        async fn snapshots(&self) -> Vec<TorrentSnapshot> {
            Vec::new()
        }
    }

    struct SlowAddFactory {
        engine: Arc<SlowAddEngine>,
    }

    #[async_trait::async_trait]
    impl SessionFactory for SlowAddFactory {
        async fn build(
            &self,
            _device: Option<&str>,
        ) -> Result<Arc<dyn TorrentEngine>, crate::engine::EngineError> {
            Ok(Arc::clone(&self.engine) as Arc<dyn TorrentEngine>)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn one_slow_add_does_not_take_the_other_transfers_down_with_it() {
        // A tunnel coming back reconciles into a build plus one add per wanted
        // row, all in one batch. Bounding the batch rather than the call meant
        // several slow-but-working adds shared one budget, and one slow magnet
        // tore down every other transfer — collateral damage dressed up as
        // failing closed. Only the torrent that was slow may suffer for it.
        const CC: &str = "cccccccccccccccccccccccccccccccccccccccc";
        let engine = Arc::new(SlowAddEngine::new(BB));
        let mut d = Driver::new(
            Arc::new(SlowAddFactory {
                engine: Arc::clone(&engine),
            }) as Arc<dyn SessionFactory>,
            Supervisor::new(BindSupport::Supported),
        );

        // Wanted while unprotected, so nothing is added yet; the bind below is
        // the single batch that builds and adds all three.
        for infohash in [AA, BB, CC] {
            d.handle(Input::User(Intent::Add(wanted(infohash)))).await;
        }
        let notices = tokio::time::timeout(
            std::time::Duration::from_secs(3600),
            d.handle(Input::Vpn(GuardAction::Bind {
                device: "utun4".into(),
            })),
        )
        .await
        .expect("a slow add must not hold the caller indefinitely");

        assert!(
            d.session_is_live(),
            "one slow magnet must not tear the session down: {notices:?}"
        );
        assert_eq!(
            engine.added(),
            vec![
                format!("magnet:?xt=urn:btih:{AA}"),
                format!("magnet:?xt=urn:btih:{CC}"),
            ],
            "the torrents that answered must still be in the session"
        );
        assert!(
            notices.iter().any(|n| n.contains(BB)),
            "the torrent that did not answer must be reported: {notices:?}"
        );
    }

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
