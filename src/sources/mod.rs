pub mod apibay;
pub mod eztv;
pub mod fitgirl;
pub mod magnet;
pub mod nyaa;
pub mod registry;
pub mod rss;
pub mod subsplease;
pub mod types;
pub mod yts;
pub use types::{
    build_magnet, magnet_from_infohash, SearchResult, Source, SourceError, SourceGroup,
    DEFAULT_TRACKERS,
};
