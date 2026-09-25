//! Integration tests for `proto` + `telegram-framework`.
//!
//! These require a mock Telegram server or a live test DC, which lands later.
//! Until then they are `#[ignore]`d so CI stays green.

#[test]
#[ignore = "requires a Telegram test DC"]
fn login_flow_round_trip() {
    // TODO: exercise phone -> code -> password -> session store.
}

#[test]
#[ignore = "requires a Telegram test DC"]
fn private_chat_filtering_against_real_server() {
    // TODO: verify group/channel/bot updates are dropped by `proto`.
}
