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

/// The candidate for the A/B pair: the same answer as [`word_prefix_match`], by a
/// byte scan. An ASCII query is compared byte for byte, and a word start is read
/// from the byte before it, or from the previous character when that byte is part
/// of a multi-byte character. A non-ASCII query goes to the reference.
fn word_prefix_match_bytes(text: &str, query: &str) -> bool {
    if query.is_empty() || !query.is_ascii() {
        return word_prefix_match(text, query);
    }
    let (bytes, wanted) = (text.as_bytes(), query.as_bytes());
    (0..bytes.len()).any(|at| {
        bytes[at].is_ascii_alphanumeric()
            && bytes
                .get(at..at + wanted.len())
                .is_some_and(|span| span.eq_ignore_ascii_case(wanted))
            && word_starts_at(text, at)
    })
}

/// Whether a word starts at byte `at`, which is an ASCII character.
fn word_starts_at(text: &str, at: usize) -> bool {
    match at.checked_sub(1).map(|before| text.as_bytes()[before]) {
        None => true,
        Some(byte) if byte < 0x80 => !byte.is_ascii_alphanumeric(),
        Some(_) => text[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric()),
    }
}

/// Both sides of the A/B pair, asserted equal on the corpus before anything is
/// timed. A candidate that answers differently is not measured at all.
fn assert_same_answers(corpus: &[String]) {
    // Word boundaries right after multi-byte characters, where the candidate reads
    // the previous character rather than the previous byte.
    let edges = [
        "éhel hel",
        "日hel",
        "x-hel",
        "hel-hel",
        "🙂hel",
        "hel",
        "xhel hel",
    ];
    for query in ["hel", "HEL", "zzzq", "ok", "e", "let's", "日", "café"] {
        for text in corpus.iter().map(String::as_str).chain(edges) {
            assert_eq!(
                word_prefix_match(text, query),
                word_prefix_match_bytes(text, query),
                "the candidate disagrees with the reference on {text:?} for {query:?}"
            );
        }
    }
}

fn word_prefix_ab(c: &mut Criterion, corpus: &[String], label: &str, query: &str) {
    let mut group = c.benchmark_group(format!("word_prefix_match_ab/{label}"));
    group.throughput(Throughput::Elements(corpus.len() as u64));
    group.bench_function("reference", |b| {
        b.iter(|| {
            corpus
                .iter()
                .filter(|text| word_prefix_match(black_box(text), black_box(query)))
                .count()
        });
    });
    group.bench_function("candidate", |b| {
        b.iter(|| {
            corpus
                .iter()
                .filter(|text| word_prefix_match_bytes(black_box(text), black_box(query)))
                .count()
        });
    });
    group.finish();
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

    // The A/B pair: one group per query, declared by id as `reference` and
    // `candidate` so `scripts/bench/compare.py` pairs them.
    assert_same_answers(&corpus);
    word_prefix_ab(c, &corpus, "hit", "hel");
    word_prefix_ab(c, &corpus, "miss", "zzzq");
}

criterion_group!(benches, word_prefix);
criterion_main!(benches);
