//! The per-frame row pipeline: `wrap::wrap` and `rows::total_rows`.
//!
//! `wrap` is what a conversation is laid out with, every frame, for every
//! message in view. `total_rows` reads the layout it produces. Both are timed
//! on a fixed, seeded conversation so the same text is wrapped on every run.

use std::hint::black_box;
use std::ops::Range;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tui::rows::{RowKind, RowSpan, total_rows};
use tui::wrap::{wrap, wrap_keeping_whitespace};

/// Messages in the fixture: the order of a full conversation window.
const MESSAGES: usize = 200;

/// Columns the conversation is wrapped to. A narrow panel and a wide one, so
/// the two ends of the row-breaking work are both in the measurement.
const WIDTHS: [u16; 2] = [40, 120];

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
    "docs",
    "thanks",
    "速い",
    "review",
    "pull",
    "request",
    "wrapping",
    "supercalifragilisticexpialidocious",
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

/// Messages of varying length; some carry a newline, which starts a row.
fn conversation() -> Vec<String> {
    let mut rng = Lcg(0x00C0_FFEE);
    (0..MESSAGES)
        .map(|_| {
            let words = 3 + rng.below(40);
            let mut text = String::new();
            for i in 0..words {
                if i > 0 {
                    text.push(if rng.below(16) == 0 { '\n' } else { ' ' });
                }
                text.push_str(WORDS[rng.below(WORDS.len())]);
            }
            text
        })
        .collect()
}

fn wrap_bench(c: &mut Criterion) {
    let conversation = conversation();
    let bytes: u64 = conversation.iter().map(|text| text.len() as u64).sum();

    let mut group = c.benchmark_group("wrap");
    group.throughput(Throughput::Bytes(bytes));
    for width in WIDTHS {
        group.bench_with_input(BenchmarkId::new("wrap", width), &width, |b, &width| {
            b.iter(|| {
                conversation
                    .iter()
                    .map(|text| wrap(black_box(text), width).len())
                    .sum::<usize>()
            });
        });
        // The input bar's variant: same engine, but the trailing space run stays
        // on its row. A sibling variant rather than a competitor.
        group.bench_with_input(
            BenchmarkId::new("wrap_keeping_whitespace", width),
            &width,
            |b, &width| {
                b.iter(|| {
                    conversation
                        .iter()
                        .map(|text| wrap_keeping_whitespace(black_box(text), width).len())
                        .sum::<usize>()
                });
            },
        );
    }
    group.finish();
}

/// The layout `App::row_layout` would build at `width`: one span per message
/// with its rows counted, then a draft row at the end, which `total_rows` must
/// leave out.
fn layout(conversation: &[String], width: u16) -> Vec<RowSpan> {
    let mut spans = Vec::with_capacity(conversation.len() + 1);
    let mut first = 0;
    for (index, text) in conversation.iter().enumerate() {
        let rows: Vec<Range<usize>> = wrap(text, width);
        let len = rows.len().max(1);
        spans.push(RowSpan {
            kind: RowKind::Message { index },
            message_id: Some(i64::try_from(index).expect("a fixture index fits an i64")),
            first,
            len,
            text: 0..text.len(),
        });
        first += len;
    }
    spans.push(RowSpan {
        kind: RowKind::Draft,
        message_id: None,
        first,
        len: 1,
        text: 0..0,
    });
    spans
}

fn total_rows_bench(c: &mut Criterion) {
    let conversation = conversation();
    let mut group = c.benchmark_group("rows");
    for width in WIDTHS {
        let spans = layout(&conversation, width);
        group.bench_with_input(BenchmarkId::new("total_rows", width), &spans, |b, spans| {
            b.iter(|| total_rows(black_box(spans)));
        });
    }
    group.finish();
}

criterion_group!(benches, wrap_bench, total_rows_bench);
criterion_main!(benches);
