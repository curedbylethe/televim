//! Translating a message on demand.
//!
//! `grammers` 0.10 wraps no translate call, so this module builds
//! `messages.translateText` itself and reads the answer out of its enum — the
//! same shape `history` uses for `GetHistory`, and for the same reason: the
//! typed layer that would otherwise do it does not exist yet.
//!
//! # What the answer does and does not say
//!
//! **One translated string per text sent**, and this wrapper sends one message,
//! so the answer is one string. The server says nothing about the language it
//! translated *from*: the layer this build speaks answers with a
//! `messages.translateResult` carrying the text and its entities and no language
//! field at all. [`Translation::source`] is therefore `None` here, and the field
//! is where a source arrives if a later layer reports one — the interface's
//! `translated from <lang>` marker needs it and this build cannot supply it.
//!
//! Entities are dropped: the translation is drawn as plain text, the same way a
//! message's own body is (`display_body`), so nothing is lost that the interface
//! would have shown.
//!
//! # The premium gate
//!
//! Translating is a Telegram Premium feature, and an account without it is
//! refused with `PREMIUM_ACCOUNT_REQUIRED` — an ordinary `RPC_ERROR` that says
//! nothing about itself. It is mapped here to
//! [`FrameworkError::TranslationPremiumRequired`] rather than passed through as a
//! code and a name, because the interface draws a different answer for it: one
//! dim sentence saying the feature is premium, not a failed request.

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::tl;

/// A message's text in another language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    /// The translated text, plain: entities are dropped.
    pub text: String,

    /// The language the message was in, when the server names one.
    ///
    /// `None` in this build: `messages.translateText` reports the translation and
    /// nothing about its source. The interface can still say what was asked for —
    /// that is the caller's own argument — but not what it came from.
    pub source: Option<String>,
}

impl Client {
    /// Translates one message into `to`.
    ///
    /// `peer_id` is the conversation's *bare* identifier, resolved through the
    /// session's peer cache like every other call here; `to` is a language code
    /// Telegram understands (`en`, `he`, `pt-BR`). The translation is a request's
    /// answer and nothing else: no message changes, nothing is cached, and no
    /// update follows it.
    ///
    /// Nothing is translated automatically — this is only ever reached because a
    /// reader asked — and the message's own text is not touched.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, [`FrameworkError::TranslationPremiumRequired`]
    /// when the account has no Telegram Premium, [`FrameworkError::TranslationEmpty`]
    /// when the answer carries no text, and [`FrameworkError::Request`] when
    /// Telegram rejects the request or the connection fails.
    pub async fn translate_message(
        &self,
        peer_id: i64,
        message_id: i32,
        to: &str,
    ) -> Result<Translation, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "a translation was asked of a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        // A message is translated by naming its peer and its identifiers; the
        // `text` field is the other way to ask, for text the client holds and
        // Telegram has never seen, and is left empty.
        let request = tl::functions::messages::TranslateText {
            peer: Some(peer.into()),
            id: Some(vec![message_id]),
            text: None,
            to_lang: to.to_owned(),
            tone: None,
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| translation_failure(RequestError::from_invocation(&error)))?;

        // A request can cache a peer or move the datacenter, and that only
        // reaches the store through this call.
        self.flush_session();

        let translation = translated(&response)?;

        tracing::debug!(
            peer_id,
            message_id,
            to,
            chars = translation.text.chars().count(),
            "translated a message"
        );

        Ok(translation)
    }
}

/// Reads the translation out of an answer, or reports one this build cannot use.
///
/// One translated string per text sent, and one text was sent. Anything else —
/// no result at all, or more than one — is an answer this build does not
/// understand rather than an empty translation, and saying so beats handing the
/// interface an empty line and letting it draw one.
pub(crate) fn translated(
    response: &tl::enums::messages::TranslatedText,
) -> Result<Translation, FrameworkError> {
    let tl::enums::messages::TranslatedText::TranslateResult(result) = response;

    let [only] = result.result.as_slice() else {
        return Err(FrameworkError::TranslationEmpty);
    };

    // One constructor, so this is a destructuring rather than a match — and a
    // refutable pattern the moment Telegram grows a second, which is the build
    // error wanted.
    let tl::enums::TextWithEntities::Entities(text) = only;

    Ok(Translation {
        text: text.text.clone(),
        // The layer this build speaks reports no source language.
        source: None,
    })
}

/// Maps a refusal onto this crate's errors, naming the premium gate.
///
/// The gate is one RPC name and it is checked here rather than at the call site,
/// so the knowledge lives once: every other refusal keeps its code and name in
/// [`FrameworkError::Request`] for a caller that does want to read them.
pub(crate) fn translation_failure(error: RequestError) -> FrameworkError {
    if let RequestError::Rpc { name, .. } = &error
        && name == "PREMIUM_ACCOUNT_REQUIRED"
    {
        return FrameworkError::TranslationPremiumRequired;
    }

    FrameworkError::Request(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl::types;

    fn answer(result: Vec<tl::enums::TextWithEntities>) -> tl::enums::messages::TranslatedText {
        tl::enums::messages::TranslatedText::TranslateResult(types::messages::TranslateResult {
            result,
        })
    }

    fn text(text: &str) -> tl::enums::TextWithEntities {
        tl::enums::TextWithEntities::Entities(types::TextWithEntities {
            text: text.to_owned(),
            entities: Vec::new(),
        })
    }

    #[test]
    fn a_translated_answer_yields_its_text_and_no_source_language() {
        let translation = translated(&answer(vec![text("שלום")])).expect("one string is an answer");

        assert_eq!(translation.text, "שלום");
        assert_eq!(
            translation.source, None,
            "this layer reports the translation and nothing about what it came from"
        );
    }

    /// An answer with no text is a shape this build cannot read, not a message
    /// that translates to nothing.
    #[test]
    fn an_answer_with_no_text_is_a_named_error() {
        assert!(matches!(
            translated(&answer(Vec::new())),
            Err(FrameworkError::TranslationEmpty)
        ));
    }

    /// The premium gate is one RPC name, and it is mapped to its own variant so
    /// the interface can answer it with a sentence rather than a failure.
    #[test]
    fn the_premium_gate_maps_to_its_own_error() {
        let refusal = RequestError::Rpc {
            code: 406,
            name: "PREMIUM_ACCOUNT_REQUIRED".to_owned(),
            value: None,
        };

        assert!(matches!(
            translation_failure(refusal),
            FrameworkError::TranslationPremiumRequired
        ));
    }

    /// Every other refusal keeps its code and name: a flood wait is not the
    /// premium gate, and saying it was would send the reader to buy what they
    /// already have.
    #[test]
    fn every_other_refusal_keeps_its_code_and_name() {
        let flood = RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(7),
        };

        assert!(matches!(
            translation_failure(flood),
            FrameworkError::Request(RequestError::Rpc { code: 420, .. })
        ));
    }
}
