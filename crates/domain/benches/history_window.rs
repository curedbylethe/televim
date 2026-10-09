//! The open conversation's window: `ConversationWindow` push, replace-free
//! paging and event application.
//!
//! Fixtures are built through the public API and are a pure function of the
//! message identifiers, so every run measures the same work.

use std::borrow::Cow;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use domain::history::{CONVERSATION_WINDOW, ConversationWindow};
use domain::message::{Message, MessageStatus};
use domain::updates::UpdateEvent;

const CHAT: i64 = 1;

/// `CONVERSATION_WINDOW` as an identifier-sized integer. The assertion keeps the
/// two in step, so the fixtures cannot drift from the window they fill.
const WINDOW: i64 = 200;
const _: () = assert!(CONVERSATION_WINDOW == 200);

/// Messages in a page that arrives from the server.
const PAGE: i64 = 50;

fn message(id: i64) -> Message {
    Message {
        id,
        chat_id: CHAT,
        text: Cow::Borrowed("a message of ordinary length, for the window to hold"),
        timestamp: 1_700_000_000 + id,
        status: MessageStatus::Received,
        is_outgoing: id % 3 == 0,
        reply_to: None,
        media: None,
        media_id: None,
    }
}

/// A full window, holding identifiers `first..first + CONVERSATION_WINDOW`.
fn full_window(first: i64) -> ConversationWindow {
    let mut window = ConversationWindow::new(CHAT);
    window.replace((first..first + WINDOW).map(message));
    window
}

fn history_window(c: &mut Criterion) {
    let mut group = c.benchmark_group("history_window");

    // Paging backwards: the page is older than everything held, so all of it
    // is accepted and the newest end falls off.
    let held = full_window(1_000);
    let older: Vec<Message> = (1_000 - PAGE..1_000).map(message).collect();
    group.bench_function("push_front", |b| {
        b.iter_batched(
            || (held.clone(), older.clone()),
            |(mut window, page)| black_box(window.push_front(page)),
            BatchSize::SmallInput,
        );
    });

    // An arrival: the page is newer than everything held, so the oldest end
    // falls off.
    let held = full_window(0);
    let newer: Vec<Message> = (WINDOW..WINDOW + PAGE).map(message).collect();
    group.bench_function("push_back", |b| {
        b.iter_batched(
            || (held.clone(), newer.clone()),
            |(mut window, page)| black_box(window.push_back(page)),
            BatchSize::SmallInput,
        );
    });

    // The feed's most common event: one message arriving in the open chat.
    let held = full_window(0);
    let arrival = UpdateEvent::NewMessage(message(WINDOW));
    group.bench_function("apply_event/new_message", |b| {
        b.iter_batched(
            || held.clone(),
            |mut window| black_box(window.apply_event(&arrival)),
            BatchSize::SmallInput,
        );
    });

    // An edit to a message in the middle of the window.
    let held = full_window(0);
    let edit = UpdateEvent::MessageEdited {
        chat_id: CHAT,
        message_id: WINDOW / 2,
        new_text: Cow::Borrowed("an edited message, of a different length than before"),
    };
    group.bench_function("apply_event/message_edited", |b| {
        b.iter_batched(
            || held.clone(),
            |mut window| black_box(window.apply_event(&edit)),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, history_window);
criterion_main!(benches);
