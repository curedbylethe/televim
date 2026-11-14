//! Pending text sends shared by the native and demo drivers.

/// A text send waiting for acknowledgement, keyed by conversation and local id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxSend {
    pub chat_id: i64,
    pub temp_id: i64,
    pub text: String,
    pub reply_to: Option<i64>,
    pub failure: Option<String>,
    pub sending: bool,
}

#[derive(Default)]
pub(crate) struct SendOutbox {
    entries: Vec<OutboxSend>,
}

impl SendOutbox {
    pub(crate) fn enqueue(&mut self, send: OutboxSend) {
        if !self
            .entries
            .iter()
            .any(|entry| (entry.chat_id, entry.temp_id) == (send.chat_id, send.temp_id))
        {
            self.entries.push(send);
        }
    }

    pub(crate) fn install(&mut self, sends: Vec<OutboxSend>) {
        self.clear();
        for mut send in sends {
            send.sending = false;
            self.enqueue(send);
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<OutboxSend> {
        self.entries.clone()
    }

    pub(crate) fn for_chat(&self, chat_id: i64) -> impl Iterator<Item = &OutboxSend> {
        self.entries
            .iter()
            .filter(move |send| send.chat_id == chat_id)
    }

    pub(crate) fn next_queued(&self) -> Option<OutboxSend> {
        self.entries
            .iter()
            .find(|send| send.failure.is_none() && !send.sending)
            .cloned()
    }

    pub(crate) fn mark_sending(&mut self, chat_id: i64, temp_id: i64) {
        if let Some(send) = self
            .entries
            .iter_mut()
            .find(|send| (send.chat_id, send.temp_id) == (chat_id, temp_id))
        {
            send.sending = true;
        }
    }

    pub(crate) fn settle_failure(
        &mut self,
        chat_id: i64,
        temp_id: i64,
        reason: String,
    ) -> Option<OutboxSend> {
        let send = self
            .entries
            .iter_mut()
            .find(|send| (send.chat_id, send.temp_id) == (chat_id, temp_id))?;
        send.failure = Some(reason);
        send.sending = false;
        Some(send.clone())
    }

    pub(crate) fn remove(&mut self, chat_id: i64, temp_id: i64) -> Option<OutboxSend> {
        let index = self
            .entries
            .iter()
            .position(|send| (send.chat_id, send.temp_id) == (chat_id, temp_id))?;
        Some(self.entries.remove(index))
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn retry_failed(&mut self) -> Vec<OutboxSend> {
        self.entries
            .iter_mut()
            .filter(|send| send.failure.is_some())
            .map(|send| {
                send.failure = None;
                send.sending = false;
                send.clone()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(chat_id: i64, temp_id: i64) -> OutboxSend {
        OutboxSend {
            chat_id,
            temp_id,
            text: "text".to_owned(),
            reply_to: None,
            failure: None,
            sending: false,
        }
    }

    #[test]
    fn queued_order_skips_in_flight_and_failed_sends_without_dropping_them() {
        let mut outbox = SendOutbox::default();
        outbox.enqueue(send(42, -1));
        outbox.enqueue(send(43, -1));
        outbox.enqueue(send(42, -2));
        outbox.enqueue(send(42, -1));
        assert_eq!(outbox.snapshot().len(), 3);
        assert_eq!(outbox.next_queued(), Some(send(42, -1)));
        outbox.mark_sending(42, -1);
        assert_eq!(outbox.next_queued(), Some(send(43, -1)));
        assert!(
            outbox
                .settle_failure(43, -1, "offline".to_owned())
                .is_some()
        );
        assert_eq!(outbox.next_queued(), Some(send(42, -2)));
        assert!(outbox.remove(42, -1).is_some());
        assert!(outbox.remove(42, -1).is_none());
        assert!(outbox.settle_failure(42, -1, "late".to_owned()).is_none());
        assert_eq!(outbox.for_chat(43).count(), 1);
        outbox.clear();
        assert!(outbox.snapshot().is_empty());
    }

    #[test]
    fn installing_a_snapshot_clears_in_flight_flags_and_preserves_failures() {
        let mut outbox = SendOutbox::default();
        let mut sending = send(42, -1);
        sending.sending = true;
        let mut failed = send(43, -1);
        failed.failure = Some("offline".to_owned());
        outbox.install(vec![sending, failed.clone()]);
        assert_eq!(outbox.next_queued(), Some(send(42, -1)));
        assert_eq!(outbox.snapshot(), vec![send(42, -1), failed]);
    }
}
