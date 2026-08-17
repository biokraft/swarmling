use super::apibay::Apibay;
use super::fitgirl::Fitgirl;
use super::nyaa::Nyaa;
use super::yts::Yts;
use super::Source;

pub fn all_sources() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(Fitgirl::new()),
        Box::new(Yts::new()),
        Box::new(Apibay::movies()),
        Box::new(Apibay::tv()),
        Box::new(Nyaa::new()),
    ]
}
