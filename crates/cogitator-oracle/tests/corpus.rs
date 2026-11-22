//! Runs on every normal build: the corpus loads and every case validates,
//! so a malformed directory fails here rather than only in the oracle job.

use cogitator_oracle::corpus::{corpus_dir, discover, Safety};

#[test]
fn every_case_loads_and_validates() {
    let cases = discover(&corpus_dir()).expect("corpus loads");
    assert!(!cases.is_empty(), "the corpus is empty");
    for safety in [Safety::P, Safety::N] {
        assert!(
            cases.iter().any(|c| c.safety == safety),
            "the corpus has no {safety:?} case"
        );
    }
}
