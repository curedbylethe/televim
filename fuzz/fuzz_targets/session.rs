//! Coverage-guided fuzzing for the two in-tree decoders of untrusted bytes.
//!
//! The first byte of the input picks the decoder; the rest is that decoder's
//! input. Nothing is unwrapped and no result is asserted on: a panic is the
//! finding, and libFuzzer turns one into a reproducer under `fuzz/artifacts/`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use telegram_framework::sealed::{self, PassphraseProvider};
use telegram_framework::session::SessionData;

/// The passphrase the envelope decoder opens with. Inputs under a wrong key
/// are still useful: they take the AEAD failure path.
const PASSPHRASE: &str = "fuzz-passphrase";

fuzz_target!(|data: &[u8]| {
    let Some((&decoder, rest)) = data.split_first() else {
        return;
    };
    match decoder {
        // A persisted snapshot: JSON, with the authorisation key as hex.
        0 => {
            let _ = SessionData::from_bytes(rest);
        }
        // A sealed session envelope. `Ok(None)` means "not an envelope", which
        // is an answer rather than a parse failure.
        1 => {
            let _ = sealed::open(rest, &PassphraseProvider::new(PASSPHRASE));
        }
        // Any other selector is not a decoder this target knows.
        _ => {}
    }
});
