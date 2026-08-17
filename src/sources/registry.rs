use super::apibay::Apibay;
use super::bittorrented::Bittorrented;
use super::eztv::Eztv;
use super::fitgirl::Fitgirl;
use super::nyaa::Nyaa;
use super::subsplease::Subsplease;
use super::x1337::X1337;
use super::yts::Yts;
use super::Source;

pub fn all_sources() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(Fitgirl::new()),
        Box::new(Yts::new()),
        Box::new(Apibay::movies()),
        Box::new(X1337::movies()),
        Box::new(Eztv::new()),
        Box::new(Apibay::tv()),
        Box::new(X1337::tv()),
        Box::new(Nyaa::new()),
        Box::new(Subsplease::new()),
        Box::new(Bittorrented::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_every_source_exactly_once() {
        let ids: Vec<&str> = all_sources().iter().map(|s| s.id()).collect();
        assert_eq!(
            ids,
            vec![
                "fitgirl",
                "yts",
                "tpb-movies",
                "x1337-movies",
                "eztv",
                "tpb-tv",
                "x1337-tv",
                "nyaa",
                "subsplease",
                "bittorrented",
            ]
        );
    }
}
