//! `search::word_prefix_match` over a conversation-sized corpus.
//!
//! The corpus is generated from a fixed seed, so the same words are scanned on
//! every run. A hit early in a message and a miss that reads every character
//! are both measured, since they are the two ends of the scan.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use domain::search::word_prefix_match;

/// Messages in the corpus, the order of a full conversation window.
const MESSAGES: usize = 200;

const WORDS: &[&str] = &[
    "hello",
    "there",
    "café",
    "meeting",
    "tomorrow",
    "日本語",
    "ok",
    "sounds",
    "good",
    "let's",
    "see",
    "the",
    "photo",
    "from",
    "yesterday",
    "🙂",
    "lunch",
    "at",
    "noon",
    "bring",
    "the",
    "docs",
    "and",
    "call",
    "me",
    "later",
    "thanks",
    "速い",
    "review",
    "pull",
    "request",
];

/// A small linear congruential generator. Deterministic, dependency-free.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    /// A draw in `0..bound`.
    fn below(&mut self, bound: usize) -> usize {
        let draw = self.next() % bound as u64;
        usize::try_from(draw).expect("a draw below a usize bound fits a usize")
    }
}

fn corpus() -> Vec<String> {
    let mut rng = Lcg(0x5EED);
    (0..MESSAGES)
        .map(|_| {
            let words = 4 + rng.below(20);
            (0..words)
                .map(|_| WORDS[rng.below(WORDS.len())])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn word_prefix(c: &mut Criterion) {
    let corpus = corpus();
    let mut group = c.benchmark_group("word_prefix_match");
    group.throughput(Throughput::Elements(corpus.len() as u64));

    // Matches at the start of a word, which most messages carry early.
    group.bench_function("scan_corpus/hit_early", |b| {
        b.iter(|| {
            corpus
                .iter()
                .filter(|text| word_prefix_match(black_box(text), black_box("hel")))
                .count()
        });
    });

    // A query nothing contains: every message is read in full.
    group.bench_function("scan_corpus/miss", |b| {
        b.iter(|| {
            corpus
                .iter()
                .filter(|text| word_prefix_match(black_box(text), black_box("zzzq")))
                .count()
        });
    });

    group.finish();
}

criterion_group!(benches, word_prefix);
criterion_main!(benches);
