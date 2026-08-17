pub mod apibay;
pub mod magnet;
pub mod registry;
pub mod types;
pub mod yts;
pub use types::{
    build_magnet, magnet_from_infohash, SearchResult, Source, SourceError, SourceGroup,
    DEFAULT_TRACKERS,
};
