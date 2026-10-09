//! `vim::char_motion` swept across a line of text.
//!
//! Each iteration moves once from every character position, so the cost of a
//! motion is measured over the whole line rather than at one lucky offset.
//! The line mixes ASCII, multi-byte and wide characters, because the motion
//! has to map character indices to byte offsets and back.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use domain::vim::{CharMotion, char_motion};

const LINE: &str = "the café sent 日本語 a photo 🙂 at noon, so let's see the docs";

fn sweep(text: &str, motion: CharMotion) -> usize {
    let positions = text.chars().count();
    (0..positions).fold(0, |acc, at| {
        acc.wrapping_add(char_motion(
            black_box(text),
            black_box(at),
            black_box(motion),
        ))
    })
}

fn motions(c: &mut Criterion) {
    let mut group = c.benchmark_group("char_motion");

    group.bench_function("step/forward", |b| {
        b.iter(|| sweep(LINE, CharMotion::Step { forward: true }));
    });
    group.bench_function("word_start/forward", |b| {
        b.iter(|| sweep(LINE, CharMotion::WordStart { forward: true }));
    });
    group.bench_function("word_end", |b| {
        b.iter(|| sweep(LINE, CharMotion::WordEnd));
    });
    group.bench_function("find/forward", |b| {
        b.iter(|| {
            sweep(
                LINE,
                CharMotion::Find {
                    target: 'e',
                    forward: true,
                    onto: true,
                },
            )
        });
    });

    group.finish();
}

criterion_group!(benches, motions);
criterion_main!(benches);
