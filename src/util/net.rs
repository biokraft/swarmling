use std::sync::OnceLock;
use std::time::Duration;

pub const USER_AGENT: &str = "swarmling (+https://github.com/biokraft/swarmling)";

/// One pooled client for every source: keep-alive and DNS caching are shared,
/// and a source can never stall a search forever.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()
            .expect("static http client configuration is valid")
    })
}
