//! The one place a live session handle exists.
//!
//! The driver executes the supervisor's instructions in order and holds the
//! engine they operate on. It makes no decisions of its own: every safety
//! question was already settled by the pure reconciler that produced the ops.

use std::sync::Arc;

use crate::engine::{TorrentEngine, TorrentSnapshot, TorrentState};
use crate::supervisor::factory::SessionFactory;
use crate::supervisor::state::{Input, SessionOp, Supervisor};

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
                    // Dropping the last handle is what tears the session down.
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
                    magnet,
                    dir,
                    paused,
                    ..
                } => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.add_magnet(&magnet, &dir, paused).await {
                        notices.push(format!("Could not start that download ({e})"));
                    }
                }
                SessionOp::PauseTorrent(infohash) => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.pause(&infohash).await {
                        notices.push(format!("Could not pause that download ({e})"));
                    }
                }
                SessionOp::ResumeTorrent(infohash) => {
                    let Some(engine) = self.engine.clone() else {
                        continue;
                    };
                    if let Err(e) = engine.resume(&infohash).await {
                        notices.push(format!("Could not resume that download ({e})"));
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
}
