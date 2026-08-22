use std::collections::HashSet;
use std::path::PathBuf;

use crate::engine::InfoHash;
use crate::vpn::guard::GuardAction;
use crate::vpn::policy::BindSupport;

/// A torrent the user wants the session to be carrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub infohash: InfoHash,
    pub magnet: String,
    pub dir: PathBuf,
    pub paused: bool,
}

/// Whether a session exists, and what it is bound to. `device: None` means the
/// session is running unbound, which only happens where the platform cannot
/// bind at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Down,
    Live { device: Option<String> },
}

/// What the user has asked for. Distinct from `SessionOp`: an intent is a wish,
/// an op is an instruction that has already cleared the safety rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Add(Wanted),
    Start(InfoHash),
    Pause(InfoHash),
    Remove {
        infohash: InfoHash,
        delete_files: bool,
    },
    /// Stop wanting anything at all. The reconcile below tears the session
    /// down on its own once nothing is wanted, so this needs no separate
    /// instruction and no reach into the engine.
    ClearAll,
}

/// Which engine call failed, fed back by the driver so the supervisor's view
/// of the session stays honest about what the engine actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedOp {
    Add,
    Pause,
    Resume,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Vpn(GuardAction),
    User(Intent),
    Completed(InfoHash),
    SessionFailed(String),
    OpFailed { infohash: InfoHash, kind: FailedOp },
}

/// An instruction for the driver. Every variant is safe to execute at the
/// moment it is emitted; the safety decisions were made before it existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOp {
    Build {
        device: Option<String>,
    },
    Kill {
        reason: String,
    },
    AddTorrent {
        infohash: InfoHash,
        magnet: String,
        dir: PathBuf,
        paused: bool,
    },
    PauseTorrent(InfoHash),
    ResumeTorrent(InfoHash),
    RemoveTorrent {
        infohash: InfoHash,
        delete_files: bool,
    },
    Notice(String),
}

/// What the guard has most recently confirmed about the tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Protection {
    Unprotected,
    /// `device: None` means protected, but on a platform that cannot bind.
    Protected {
        device: Option<String>,
    },
}

pub struct Supervisor {
    state: SessionState,
    protection: Protection,
    bind: BindSupport,
    wanted: Vec<Wanted>,
    in_session: HashSet<InfoHash>,
    /// Set when a build or add failed. Blocks automatic retries so a broken
    /// backend cannot spin. Cleared by any fresh guard event or user intent.
    stalled: bool,
}

impl Supervisor {
    pub fn new(bind: BindSupport) -> Self {
        Self {
            state: SessionState::Down,
            protection: Protection::Unprotected,
            bind,
            wanted: Vec::new(),
            in_session: HashSet::new(),
            stalled: false,
        }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// The torrents the user currently wants carried. Exposed for the driver's
    /// snapshot poll, which only asks about these.
    pub fn wanted(&self) -> &[Wanted] {
        &self.wanted
    }

    pub fn handle(&mut self, input: Input) -> Vec<SessionOp> {
        let mut ops = Vec::new();
        match input {
            Input::Vpn(action) => self.apply_vpn(action, &mut ops),
            Input::User(intent) => self.apply_intent(intent, &mut ops),
            Input::Completed(infohash) => {
                self.stalled = false;
                let known = self.wanted.iter().any(|w| w.infohash == infohash);
                // Never seed: a finished torrent leaves the session at once.
                if self.in_session.remove(&infohash) {
                    ops.push(SessionOp::RemoveTorrent {
                        infohash: infohash.clone(),
                        delete_files: false,
                    });
                }
                if known {
                    // Say so, or a download that finished and vanished from
                    // the session is indistinguishable from one that stalled.
                    ops.push(SessionOp::Notice(format!(
                        "{infohash} finished and left the session — swarmling never seeds."
                    )));
                }
                self.wanted.retain(|w| w.infohash != infohash);
            }
            Input::SessionFailed(message) => {
                self.state = SessionState::Down;
                self.in_session.clear();
                self.stalled = true;
                ops.push(SessionOp::Notice(message));
                return ops;
            }
            Input::OpFailed { infohash, kind } => {
                // The driver already told the user what failed; this only
                // keeps our view of the session honest about what the engine
                // actually did. Every arm returns immediately, without a
                // general reconcile, so a failure here can never itself
                // trigger another op that might fail — a `Kill` cannot fail,
                // and the `Add` arm only ever re-tries on a later, unrelated
                // input.
                match kind {
                    FailedOp::Add => {
                        // The engine does not have it after all; the user
                        // still wants it, so leave it in `wanted` but stop
                        // pretending it is in the session, and don't retry in
                        // a loop until something fresh happens.
                        self.in_session.remove(&infohash);
                        self.stalled = true;
                    }
                    FailedOp::Pause | FailedOp::Remove => {
                        // We asked the engine to stop carrying a torrent and
                        // it refused. We can no longer be sure what the
                        // engine is doing, so fail closed: kill the session
                        // rather than risk it carrying on unnoticed.
                        let verb = if matches!(kind, FailedOp::Pause) {
                            "pause"
                        } else {
                            "remove"
                        };
                        self.kill(format!("failed to {verb} torrent {infohash}"), &mut ops);
                        self.stalled = true;
                    }
                    FailedOp::Resume => {
                        // It did not actually resume; say so.
                        if let Some(w) = self.wanted.iter_mut().find(|w| w.infohash == infohash) {
                            w.paused = true;
                        }
                    }
                }
                return ops;
            }
        }
        self.reconcile(&mut ops);
        ops
    }

    fn apply_vpn(&mut self, action: GuardAction, ops: &mut Vec<SessionOp>) {
        match action {
            GuardAction::None => {}
            GuardAction::PauseAll { reason } => {
                self.stalled = false;
                self.protection = Protection::Unprotected;
                self.kill(reason, ops);
            }
            GuardAction::Bind { device } | GuardAction::Rebind { device } => {
                self.stalled = false;
                let new = self.device_for(&device);
                let changed =
                    !matches!(&self.protection, Protection::Protected { device: d } if *d == new);
                self.protection = Protection::Protected { device: new };
                // A different tunnel means the live session is bound to a
                // device that is no longer the protected one. Kill it; the
                // reconcile below builds a correctly bound replacement.
                if changed {
                    self.kill("the VPN moved to a different device".to_string(), ops);
                }
            }
        }
    }

    /// The device a session should bind to, or `None` where the platform
    /// cannot bind at all.
    fn device_for(&self, device: &str) -> Option<String> {
        match self.bind {
            BindSupport::Supported => Some(device.to_string()),
            BindSupport::Unsupported => None,
        }
    }

    fn kill(&mut self, reason: String, ops: &mut Vec<SessionOp>) {
        if matches!(self.state, SessionState::Live { .. }) {
            self.state = SessionState::Down;
            self.in_session.clear();
            ops.push(SessionOp::Kill { reason });
        }
    }

    fn apply_intent(&mut self, intent: Intent, ops: &mut Vec<SessionOp>) {
        self.stalled = false;
        match intent {
            Intent::Add(w) => {
                if self.wanted.iter().any(|x| x.infohash == w.infohash) {
                    return;
                }
                if matches!(self.protection, Protection::Unprotected) {
                    ops.push(SessionOp::Notice(
                        "queued. Nothing will be downloaded until a VPN tunnel is up.".to_string(),
                    ));
                }
                self.wanted.push(w);
            }
            Intent::Start(infohash) => {
                let Some(w) = self.wanted.iter_mut().find(|w| w.infohash == infohash) else {
                    return;
                };
                w.paused = false;
                if matches!(self.protection, Protection::Unprotected) {
                    ops.push(SessionOp::Notice(
                        "queued to start. Nothing will be downloaded until a VPN tunnel is up."
                            .to_string(),
                    ));
                    return;
                }
                if self.in_session.contains(&infohash) {
                    ops.push(SessionOp::ResumeTorrent(infohash));
                }
            }
            Intent::Pause(infohash) => {
                let Some(w) = self.wanted.iter_mut().find(|w| w.infohash == infohash) else {
                    return;
                };
                w.paused = true;
                if self.in_session.contains(&infohash) {
                    ops.push(SessionOp::PauseTorrent(infohash));
                }
            }
            Intent::ClearAll => {
                // Files already on disk are left alone, exactly as a single
                // removal does: clearing a list is not consent to delete them.
                self.wanted.clear();
                for infohash in std::mem::take(&mut self.in_session) {
                    ops.push(SessionOp::RemoveTorrent {
                        infohash,
                        delete_files: false,
                    });
                }
            }
            Intent::Remove {
                infohash,
                delete_files,
            } => {
                if !self.wanted.iter().any(|w| w.infohash == infohash) {
                    return;
                }
                self.wanted.retain(|w| w.infohash != infohash);
                if self.in_session.remove(&infohash) {
                    ops.push(SessionOp::RemoveTorrent {
                        infohash,
                        delete_files,
                    });
                }
            }
        }
    }

    /// Bring the session in line with what is wanted. This is where the
    /// safety rules bite: a build is only ever emitted from a protected state.
    fn reconcile(&mut self, ops: &mut Vec<SessionOp>) {
        let device = match &self.protection {
            Protection::Unprotected => {
                self.kill("no confirmed VPN tunnel".to_string(), ops);
                return;
            }
            Protection::Protected { device } => device.clone(),
        };

        if !self.wanted.iter().any(|w| !w.paused) {
            // Nothing active to carry: hold no session, and therefore no
            // sockets.
            self.kill("nothing left to download".to_string(), ops);
            return;
        }

        if self.stalled {
            return;
        }

        if matches!(self.state, SessionState::Down) {
            ops.push(SessionOp::Build {
                device: device.clone(),
            });
            if device.is_none() {
                ops.push(SessionOp::Notice(
                    "this platform cannot bind torrent traffic to the VPN device: \
                     traffic is not pinned to the tunnel"
                        .to_string(),
                ));
            }
            self.state = SessionState::Live { device };
        }

        for w in &self.wanted {
            if !self.in_session.contains(&w.infohash) {
                ops.push(SessionOp::AddTorrent {
                    infohash: w.infohash.clone(),
                    magnet: w.magnet.clone(),
                    dir: w.dir.clone(),
                    paused: w.paused,
                });
                self.in_session.insert(w.infohash.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::guard::GuardAction;

    fn wanted(infohash: &str) -> Wanted {
        Wanted {
            infohash: infohash.to_string(),
            magnet: format!("magnet:?xt=urn:btih:{infohash}"),
            dir: std::path::PathBuf::from("/downloads"),
            paused: false,
        }
    }

    fn bound() -> Supervisor {
        Supervisor::new(BindSupport::Supported)
    }

    #[test]
    fn nothing_is_built_while_unprotected() {
        let mut s = bound();
        let ops = s.handle(Input::User(Intent::Add(wanted("aa"))));
        assert!(
            !ops.iter().any(|op| matches!(op, SessionOp::Build { .. })),
            "a session must never be built without a confirmed tunnel: {ops:?}"
        );
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn protection_with_something_wanted_builds_a_bound_session() {
        let mut s = bound();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        assert_eq!(
            ops,
            vec![
                SessionOp::Build {
                    device: Some("utun4".to_string())
                },
                SessionOp::AddTorrent {
                    infohash: "aa".to_string(),
                    magnet: "magnet:?xt=urn:btih:aa".to_string(),
                    dir: std::path::PathBuf::from("/downloads"),
                    paused: false,
                },
            ]
        );
    }

    #[test]
    fn protection_with_nothing_wanted_builds_nothing() {
        // An idle app holds no session, so it holds no sockets.
        let mut s = bound();
        let ops = s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        assert_eq!(ops, vec![]);
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn a_dropped_tunnel_kills_the_session() {
        let mut s = bound();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        let ops = s.handle(Input::Vpn(GuardAction::PauseAll {
            reason: "tunnel utun4 went away".into(),
        }));
        assert_eq!(
            ops,
            vec![SessionOp::Kill {
                reason: "tunnel utun4 went away".to_string()
            }]
        );
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn reconnecting_rebuilds_and_re_adds_everything_wanted() {
        let mut s = bound();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        s.handle(Input::Vpn(GuardAction::PauseAll {
            reason: "gone".into(),
        }));
        let ops = s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun9".into(),
        }));
        assert_eq!(
            ops,
            vec![
                SessionOp::Build {
                    device: Some("utun9".to_string())
                },
                SessionOp::AddTorrent {
                    infohash: "aa".to_string(),
                    magnet: "magnet:?xt=urn:btih:aa".to_string(),
                    dir: std::path::PathBuf::from("/downloads"),
                    paused: false,
                },
            ]
        );
    }

    #[test]
    fn a_rebind_kills_before_it_builds() {
        let mut s = bound();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        let ops = s.handle(Input::Vpn(GuardAction::Rebind {
            device: "utun9".into(),
        }));
        let kill = ops
            .iter()
            .position(|op| matches!(op, SessionOp::Kill { .. }));
        let build = ops
            .iter()
            .position(|op| matches!(op, SessionOp::Build { .. }));
        assert!(
            kill.is_some() && build.is_some(),
            "expected both a kill and a build: {ops:?}"
        );
        assert!(
            kill < build,
            "the old session must die before the new one is built: {ops:?}"
        );
    }

    #[test]
    fn an_unbindable_platform_runs_unbound_and_says_so() {
        // Windows cannot pin traffic to the tunnel device. It still refuses to
        // run without one, but the user must be told the traffic is unpinned.
        let mut s = Supervisor::new(BindSupport::Unsupported);
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::Vpn(GuardAction::Bind {
            device: "wg0".into(),
        }));
        assert!(
            ops.contains(&SessionOp::Build { device: None }),
            "an unbindable platform must build an unbound session: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, SessionOp::Notice(m) if m.contains("not pinned"))),
            "the user must be told traffic is not pinned to the device: {ops:?}"
        );
    }

    #[test]
    fn guard_inaction_produces_no_ops() {
        let mut s = bound();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        assert_eq!(s.handle(Input::Vpn(GuardAction::None)), vec![]);
    }

    #[test]
    fn a_kill_while_already_down_does_nothing() {
        let mut s = bound();
        assert_eq!(
            s.handle(Input::Vpn(GuardAction::PauseAll {
                reason: "gone".into()
            })),
            vec![]
        );
    }

    fn protected() -> Supervisor {
        let mut s = Supervisor::new(BindSupport::Supported);
        s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        s
    }

    #[test]
    fn adding_while_protected_builds_and_adds() {
        let mut s = protected();
        let ops = s.handle(Input::User(Intent::Add(wanted("aa"))));
        assert_eq!(
            ops,
            vec![
                SessionOp::Build {
                    device: Some("utun4".to_string())
                },
                SessionOp::AddTorrent {
                    infohash: "aa".to_string(),
                    magnet: "magnet:?xt=urn:btih:aa".to_string(),
                    dir: std::path::PathBuf::from("/downloads"),
                    paused: false,
                },
            ]
        );
    }

    #[test]
    fn adding_the_same_torrent_twice_adds_it_once() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::User(Intent::Add(wanted("aa"))));
        assert!(
            !ops.iter()
                .any(|op| matches!(op, SessionOp::AddTorrent { .. })),
            "a second add of the same infohash must not add it again: {ops:?}"
        );
    }

    #[test]
    fn starting_while_unprotected_refuses_and_builds_nothing() {
        let mut s = Supervisor::new(BindSupport::Supported);
        let mut paused = wanted("aa");
        paused.paused = true;
        s.handle(Input::User(Intent::Add(paused)));
        let ops = s.handle(Input::User(Intent::Start("aa".into())));
        assert!(
            !ops.iter().any(|op| matches!(op, SessionOp::Build { .. })),
            "pressing start without a tunnel must not build a session: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Notice(_))),
            "the refusal must be explained to the user: {ops:?}"
        );
    }

    #[test]
    fn pausing_the_last_active_torrent_kills_the_session() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::User(Intent::Pause("aa".into())));
        assert!(
            ops.contains(&SessionOp::PauseTorrent("aa".to_string())),
            "expected a pause: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Kill { .. })),
            "with nothing active the session must not be left open: {ops:?}"
        );
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn starting_a_paused_torrent_brings_the_session_back() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::User(Intent::Pause("aa".into())));
        let ops = s.handle(Input::User(Intent::Start("aa".into())));
        assert!(
            ops.contains(&SessionOp::Build {
                device: Some("utun4".to_string())
            }),
            "expected a rebuild: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, SessionOp::AddTorrent { infohash, .. } if infohash == "aa")),
            "the restarted torrent must be re-added: {ops:?}"
        );
    }

    #[test]
    fn removing_a_torrent_removes_it_and_forgets_it() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::User(Intent::Add(wanted("bb"))));
        let ops = s.handle(Input::User(Intent::Remove {
            infohash: "aa".into(),
            delete_files: true,
        }));
        assert!(
            ops.contains(&SessionOp::RemoveTorrent {
                infohash: "aa".to_string(),
                delete_files: true
            }),
            "expected a remove: {ops:?}"
        );
        assert_eq!(s.wanted().len(), 1);
        assert_eq!(s.wanted()[0].infohash, "bb");
    }

    #[test]
    fn clearing_everything_empties_the_session_and_keeps_the_files() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::User(Intent::Add(wanted("bb"))));
        let ops = s.handle(Input::User(Intent::ClearAll));

        assert!(s.wanted().is_empty(), "nothing may still be wanted");
        for infohash in ["aa", "bb"] {
            assert!(
                ops.contains(&SessionOp::RemoveTorrent {
                    infohash: infohash.to_string(),
                    delete_files: false
                }),
                "{infohash} must be taken out of the session, files left alone: {ops:?}"
            );
        }
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Kill { .. })),
            "a session carrying nothing must not be held open: {ops:?}"
        );
    }

    #[test]
    fn a_finished_torrent_tells_the_user_it_finished() {
        // A download that completes leaves the session at once, which looks
        // exactly like one that stalled unless we say what happened.
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::Completed("aa".into()));
        assert!(
            ops.iter()
                .any(|op| matches!(op, SessionOp::Notice(message) if message.contains("finished"))),
            "a completed download must say so: {ops:?}"
        );
    }

    #[test]
    fn a_finished_torrent_leaves_the_session_immediately() {
        // Swarmling never seeds. This is the only place that is enforced.
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::Completed("aa".into()));
        assert!(
            ops.contains(&SessionOp::RemoveTorrent {
                infohash: "aa".to_string(),
                delete_files: false
            }),
            "a completed torrent must be removed, never seeded: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Kill { .. })),
            "the last torrent finishing must close the session: {ops:?}"
        );
    }

    #[test]
    fn a_failed_session_does_not_immediately_rebuild() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::SessionFailed("backend exploded".into()));
        assert!(
            !ops.iter().any(|op| matches!(op, SessionOp::Build { .. })),
            "a failed session must not be retried in a loop: {ops:?}"
        );
        assert!(ops
            .iter()
            .any(|op| matches!(op, SessionOp::Notice(m) if m.contains("backend exploded"))));
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn a_fresh_user_intent_clears_a_stall() {
        // aa must be paused before the failure, so the only way reconcile can
        // see something active to rebuild for is if Start's `w.paused = false`
        // actually ran.
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::User(Intent::Pause("aa".into())));
        s.handle(Input::SessionFailed("backend exploded".into()));
        let ops = s.handle(Input::User(Intent::Start("aa".into())));
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Build { .. })),
            "an explicit user retry must be allowed to rebuild: {ops:?}"
        );
    }

    #[test]
    fn refusing_to_start_still_latches_and_starts_once_bound() {
        // Start's unpause happens before the unprotected guard, matching Add.
        // The refusal notice must not be the last word: a later bind with no
        // further user action brings the torrent up unpaused.
        let mut s = Supervisor::new(BindSupport::Supported);
        let mut paused = wanted("aa");
        paused.paused = true;
        s.handle(Input::User(Intent::Add(paused)));
        s.handle(Input::User(Intent::Start("aa".into())));
        let ops = s.handle(Input::Vpn(GuardAction::Bind {
            device: "utun4".into(),
        }));
        assert_eq!(
            ops,
            vec![
                SessionOp::Build {
                    device: Some("utun4".to_string())
                },
                SessionOp::AddTorrent {
                    infohash: "aa".to_string(),
                    magnet: "magnet:?xt=urn:btih:aa".to_string(),
                    dir: std::path::PathBuf::from("/downloads"),
                    paused: false,
                },
            ]
        );
    }

    #[test]
    fn intents_for_an_unknown_torrent_are_harmless() {
        let mut s = protected();
        assert_eq!(s.handle(Input::User(Intent::Start("zz".into()))), vec![]);
        assert_eq!(s.handle(Input::User(Intent::Pause("zz".into()))), vec![]);
        assert_eq!(
            s.handle(Input::User(Intent::Remove {
                infohash: "zz".into(),
                delete_files: false
            })),
            vec![]
        );
        assert_eq!(s.handle(Input::Completed("zz".into())), vec![]);
    }

    #[test]
    fn a_failed_add_forgets_the_engine_has_it_but_keeps_wanting_it() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        // The reconcile above emitted AddTorrent and optimistically marked
        // "aa" in_session; now the driver reports it actually failed.
        s.handle(Input::OpFailed {
            infohash: "aa".into(),
            kind: FailedOp::Add,
        });
        // Still wanted, so a fresh VPN bind (which forces a full reconcile
        // via kill+rebuild) must try to add it again.
        let ops = s.handle(Input::Vpn(GuardAction::Rebind {
            device: "utun9".into(),
        }));
        assert!(
            ops.iter()
                .any(|op| matches!(op, SessionOp::AddTorrent { infohash, .. } if infohash == "aa")),
            "a torrent whose add failed must still be retried once reconnected: {ops:?}"
        );
    }

    #[test]
    fn a_failed_add_does_not_immediately_retry_in_the_same_batch() {
        let mut s = protected();
        let ops = s.handle(Input::OpFailed {
            infohash: "aa".into(),
            kind: FailedOp::Add,
        });
        assert!(
            !ops.iter()
                .any(|op| matches!(op, SessionOp::AddTorrent { .. })),
            "a failed add must not be retried immediately: {ops:?}"
        );
    }

    #[test]
    fn a_failed_pause_kills_the_session() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::OpFailed {
            infohash: "aa".into(),
            kind: FailedOp::Pause,
        });
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Kill { .. })),
            "a pause we cannot be sure took effect must fail closed: {ops:?}"
        );
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn a_failed_remove_kills_the_session() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        let ops = s.handle(Input::OpFailed {
            infohash: "aa".into(),
            kind: FailedOp::Remove,
        });
        assert!(
            ops.iter().any(|op| matches!(op, SessionOp::Kill { .. })),
            "a remove we cannot be sure took effect must fail closed: {ops:?}"
        );
        assert_eq!(s.state(), &SessionState::Down);
    }

    #[test]
    fn a_failed_resume_marks_the_entry_paused_again() {
        let mut s = protected();
        s.handle(Input::User(Intent::Add(wanted("aa"))));
        s.handle(Input::User(Intent::Pause("aa".into())));
        s.handle(Input::User(Intent::Start("aa".into())));
        let ops = s.handle(Input::OpFailed {
            infohash: "aa".into(),
            kind: FailedOp::Resume,
        });
        assert_eq!(ops, vec![], "the driver already notified the user");
        assert!(
            s.wanted().iter().any(|w| w.infohash == "aa" && w.paused),
            "the record must reflect that it did not actually resume"
        );
    }
}
