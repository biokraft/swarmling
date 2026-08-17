use super::apibay::Apibay;
use super::yts::Yts;
use super::Source;

pub fn all_sources() -> Vec<Box<dyn Source>> {
    vec![
        Box::new(Yts::new()),
        Box::new(Apibay::movies()),
        Box::new(Apibay::tv()),
    ]
}
