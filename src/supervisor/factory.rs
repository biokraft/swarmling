//! Where a torrent session comes from.
//!
//! The driver never constructs a session itself; it asks a factory. That seam
//! is what lets every test in this crate run the driver against `FakeEngine`,
//! so no test can open a socket even by accident.

use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::librqbit_engine::LibrqbitEngine;
use crate::engine::{EngineError, TorrentEngine};

#[async_trait::async_trait]
pub trait SessionFactory: Send + Sync {
    /// Build a session. `device` is the VPN interface to bind traffic to, or
    /// `None` on a platform that cannot bind.
    async fn build(&self, device: Option<&str>) -> Result<Arc<dyn TorrentEngine>, EngineError>;
}

/// The real thing. **Never used from a test.** Constructing this and calling
/// `build` opens a real BitTorrent session, which the project's hard rule
/// forbids anywhere except a maintainer's own machine.
pub struct LibrqbitFactory {
    download_dir: PathBuf,
    session_dir: PathBuf,
}

impl LibrqbitFactory {
    pub fn new(download_dir: PathBuf, session_dir: PathBuf) -> Self {
        Self {
            download_dir,
            session_dir,
        }
    }
}

#[async_trait::async_trait]
impl SessionFactory for LibrqbitFactory {
    async fn build(&self, device: Option<&str>) -> Result<Arc<dyn TorrentEngine>, EngineError> {
        let engine = match device {
            Some(device) => {
                LibrqbitEngine::new_bound(
                    self.download_dir.clone(),
                    self.session_dir.clone(),
                    device,
                )
                .await?
            }
            None => {
                LibrqbitEngine::new(self.download_dir.clone(), self.session_dir.clone()).await?
            }
        };
        Ok(Arc::new(engine))
    }
}

#[cfg(test)]
pub use fake_factory::FakeFactory;

#[cfg(test)]
mod fake_factory {
    use super::*;
    use crate::engine::fake::FakeEngine;
    use std::sync::Mutex;

    /// A factory that hands out one shared `FakeEngine`, so a test can inspect
    /// exactly what the driver did to it. Opens nothing.
    pub struct FakeFactory {
        engine: Arc<FakeEngine>,
        built: Mutex<Vec<Option<String>>>,
        fail_next: Mutex<Option<String>>,
    }

    impl Default for FakeFactory {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeFactory {
        pub fn new() -> Self {
            Self {
                engine: Arc::new(FakeEngine::new()),
                built: Mutex::new(Vec::new()),
                fail_next: Mutex::new(None),
            }
        }

        /// The shared engine, so a test can drive progress and completion.
        pub fn engine(&self) -> Arc<FakeEngine> {
            Arc::clone(&self.engine)
        }

        /// The devices `build` was called with, in order.
        pub fn built_devices(&self) -> Vec<Option<String>> {
            self.built.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        pub fn fail_next(&self, message: &str) {
            *self.fail_next.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.to_string());
        }
    }

    #[async_trait::async_trait]
    impl SessionFactory for FakeFactory {
        async fn build(&self, device: Option<&str>) -> Result<Arc<dyn TorrentEngine>, EngineError> {
            if let Some(message) = self
                .fail_next
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                return Err(EngineError::Backend(message));
            }
            self.built
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(device.map(str::to_string));
            Ok(Arc::clone(&self.engine) as Arc<dyn TorrentEngine>)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_fake_factory_hands_back_an_engine() {
        let f = FakeFactory::new();
        let engine = f
            .build(Some("utun4"))
            .await
            .expect("the fake never fails by default");
        assert!(engine.snapshots().await.is_empty());
        assert_eq!(f.built_devices(), vec![Some("utun4".to_string())]);
    }

    #[tokio::test]
    async fn a_failing_factory_reports_its_error() {
        let f = FakeFactory::new();
        f.fail_next("no tunnel");
        let err = match f.build(None).await {
            Ok(_) => panic!("the factory was told to fail"),
            Err(err) => err,
        };
        assert!(format!("{err}").contains("no tunnel"));
    }

    #[tokio::test]
    async fn the_fake_factory_returns_the_same_engine_each_time() {
        // The driver holds one engine at a time; tests need to inspect the one
        // the driver is actually using.
        let f = FakeFactory::new();
        let a = f.build(None).await.expect("build");
        a.add_magnet(
            "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            std::path::Path::new("/downloads"),
            false,
        )
        .await
        .expect("add");
        let b = f.build(None).await.expect("build");
        assert_eq!(
            b.snapshots().await.len(),
            1,
            "the fake factory must reuse one engine"
        );
    }
}
