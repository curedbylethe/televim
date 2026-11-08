use std::{borrow::Cow, collections::VecDeque};

use domain::{
    chat::Chat,
    global_search::SearchSpec,
    message::{MediaKind, Message, MessageStatus},
    presence::Presence,
    report::ReportReason,
    updates::UpdateEvent,
    user::rank_candidates,
};
use tui::{Action, App, app::FetchDirection, state::update};

use crate::search::{self, GLOBAL_PAGE_SIZE, GlobalPage};
use crate::{DemoWorld, Effect};

/// One deterministic item waiting for simulated time to reach `due_ms`.
#[derive(Debug, Clone)]
pub struct Scheduled {
    pub due_ms: u64,
    pub effect: Effect,
}

/// Local action handler and timed event queue for the fictional demo world.
#[derive(Debug, Clone)]
pub struct DemoSimulator {
    pub world: DemoWorld,
    pub now_ms: u64,
    next_message_id: i64,
    queue: VecDeque<Scheduled>,
    last_global: Option<GlobalPage>,
    pub reports: Vec<(i64, ReportReason)>,
    failed_uploads: Vec<(i64, i64, MediaKind)>,
    open_chat_id: Option<i64>,
}

impl DemoSimulator {
    #[must_use]
    pub fn new(world: DemoWorld) -> Self {
        let next_message_id = world
            .accounts
            .iter()
            .flat_map(|account| account.messages.values().flatten())
            .map(|message| message.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Self {
            world,
            now_ms: 0,
            next_message_id,
            queue: VecDeque::new(),
            last_global: None,
            reports: Vec::new(),
            failed_uploads: Vec::new(),
            open_chat_id: None,
        }
    }

    /// Handles one TUI request without changing `app`.
    #[allow(clippy::too_many_lines)] // Keep exhaustive action coverage in one match.
    pub fn dispatch(&mut self, _app: &App, action: Action) -> Vec<Effect> {
        match action {
            Action::Update(
                update::Action::Open
                | update::Action::Apply { .. }
                | update::Action::Restart
                | update::Action::Cancel,
            ) => vec![Effect::UpdateUnavailable],
            Action::Update(update::Action::Later { .. }) => {
                vec![Self::status("Updates are disabled in the offline demo.")]
            }
            Action::Update(update::Action::InstalledLater) => {
                vec![Self::status("No update is installed in the offline demo.")]
            }
            Action::Send {
                chat_id,
                temp_id,
                text,
                reply_to,
            } => {
                if text.trim().is_empty() {
                    return vec![Effect::SendFailed {
                        chat_id,
                        temp_id,
                        reason: "Message is empty; nothing was sent.".into(),
                    }];
                }
                if !self.has_chat(chat_id) {
                    return vec![Self::status("That conversation is not in the demo.")];
                }
                if text.starts_with("!fail") {
                    self.schedule(
                        300,
                        Effect::SendFailed {
                            chat_id,
                            temp_id,
                            reason: "The demo simulated a send failure.".into(),
                        },
                    );
                    return vec![Self::status(
                        "The demo is preparing a simulated send failure.",
                    )];
                }
                let message =
                    self.new_message(chat_id, text, true, MessageStatus::Sent, reply_to, None);
                let delay = 250 + (message.id.unsigned_abs() % 350);
                self.push_message(&message);
                self.schedule(
                    delay,
                    Effect::Sent {
                        chat_id,
                        temp_id,
                        message,
                    },
                );
                vec![Self::status("Message queued in the offline demo.")]
            }
            Action::SendAttachment {
                chat_id,
                temp_id,
                path: _,
                kind,
            } => {
                if self.has_chat(chat_id) {
                    vec![Effect::RequestFilePicker {
                        chat_id,
                        temp_id,
                        kind,
                    }]
                } else {
                    vec![Self::status("That conversation is not in the demo.")]
                }
            }
            Action::CancelUpload { temp_id } => {
                let chat_id = self
                    .queue
                    .iter()
                    .find_map(|scheduled| match &scheduled.effect {
                        Effect::UploadProgress {
                            chat_id,
                            temp_id: id,
                            ..
                        }
                        | Effect::Sent {
                            chat_id,
                            temp_id: id,
                            ..
                        }
                        | Effect::SendFailed {
                            chat_id,
                            temp_id: id,
                            ..
                        } if *id == temp_id => Some(*chat_id),
                        _ => None,
                    });
                let Some(chat_id) = chat_id else {
                    return vec![Self::status("No upload is running for that message.")];
                };
                self.queue.retain(|scheduled| !match &scheduled.effect {
                    Effect::UploadProgress { temp_id: id, .. }
                    | Effect::Sent { temp_id: id, .. }
                    | Effect::SendFailed { temp_id: id, .. } => *id == temp_id,
                    _ => false,
                });
                self.failed_uploads.retain(|(_, id, _)| *id != temp_id);
                self.failed_uploads
                    .push((chat_id, temp_id, MediaKind::File));
                vec![Effect::SendFailed {
                    chat_id,
                    temp_id,
                    reason: "Upload cancelled; nothing was sent.".into(),
                }]
            }
            Action::RetryUpload { chat_id, temp_id } => {
                if let Some(index) = self
                    .failed_uploads
                    .iter()
                    .position(|entry| entry.0 == chat_id && entry.1 == temp_id)
                {
                    let (_, _, kind) = self.failed_uploads.remove(index);
                    self.queue.retain(|scheduled| {
                        !matches!(
                            &scheduled.effect,
                            Effect::UploadProgress { temp_id: id, .. }
                                | Effect::Sent { temp_id: id, .. }
                                | Effect::SendFailed { temp_id: id, .. }
                                if *id == temp_id
                        )
                    });
                    vec![Effect::RequestFilePicker {
                        chat_id,
                        temp_id,
                        kind,
                    }]
                } else {
                    vec![Self::status("There is no failed demo upload to retry.")]
                }
            }
            Action::DismissUpload { temp_id } => {
                self.failed_uploads.retain(|(_, id, _)| *id != temp_id);
                self.queue.retain(|scheduled| {
                    !matches!(
                        &scheduled.effect,
                        Effect::UploadProgress { temp_id: id, .. }
                            | Effect::Sent { temp_id: id, .. }
                            | Effect::SendFailed { temp_id: id, .. }
                            if *id == temp_id
                    )
                });
                vec![Self::status("Attachment dismissed.")]
            }
            Action::Edit {
                chat_id,
                message_id,
                text,
            } => {
                let message = self.message_mut(chat_id, message_id);
                if let Some(message) = message.filter(|message| message.is_outgoing) {
                    message.text = Cow::Owned(text.clone());
                    vec![Effect::Edited {
                        chat_id,
                        message_id,
                        text,
                    }]
                } else {
                    vec![Self::status("Only an outgoing demo message can be edited.")]
                }
            }
            Action::Delete {
                chat_id,
                message_ids,
            } => {
                let messages = self.active_mut().messages.entry(chat_id).or_default();
                let found: Vec<_> = messages
                    .iter()
                    .filter(|message| message_ids.contains(&message.id))
                    .map(|message| message.id)
                    .collect();
                if found.is_empty() {
                    return vec![Self::status(
                        "No matching message exists in that conversation.",
                    )];
                }
                messages.retain(|message| !found.contains(&message.id));
                self.active_mut()
                    .media
                    .retain(|(id, message_id), _| *id != chat_id || !found.contains(message_id));
                vec![Effect::Deleted {
                    chat_id,
                    message_ids: found,
                }]
            }
            Action::Forward {
                chat_id,
                message_ids,
                dest_chat_id,
            } => {
                if !self.has_chat(dest_chat_id) {
                    return vec![Self::status(
                        "The destination conversation is not in the demo.",
                    )];
                }
                let copies: Vec<_> = self
                    .active()
                    .messages(chat_id)
                    .iter()
                    .filter(|message| message_ids.contains(&message.id))
                    .map(|message| {
                        (
                            message.clone(),
                            self.active().media.get(&(chat_id, message.id)).cloned(),
                        )
                    })
                    .collect();
                if copies.is_empty() {
                    return vec![Self::status("No matching message exists to forward.")];
                }
                let mut forwarded = Vec::with_capacity(copies.len());
                for (source, media) in copies {
                    let message = self.new_message(
                        dest_chat_id,
                        source.text.to_string(),
                        true,
                        MessageStatus::Sent,
                        None,
                        source.media,
                    );
                    if let Some(media) = media {
                        self.active_mut()
                            .media
                            .insert((dest_chat_id, message.id), media);
                    }
                    self.push_message(&message);
                    forwarded.push(message);
                }
                vec![Effect::Forwarded {
                    dest_chat_id,
                    messages: forwarded,
                }]
            }
            Action::FetchContact { peer_id } => {
                let blocked = self.active().blocked.contains(&peer_id);
                let result = self
                    .active()
                    .profile(peer_id)
                    .cloned()
                    .ok_or_else(|| "That profile is not in the demo.".to_owned());
                vec![Effect::Contact {
                    peer_id,
                    blocked,
                    result,
                }]
            }
            Action::FetchContacts => vec![Effect::Contacts {
                result: Ok(self.active().contacts.clone()),
            }],
            Action::Search { chat_id, query } => {
                let ids = search::search_messages(self.active().messages(chat_id), &query);
                let total = self
                    .active()
                    .messages(chat_id)
                    .iter()
                    .filter(|message| {
                        domain::search::word_prefix_match(message.display_body(), &query)
                    })
                    .count();
                vec![Effect::Searched {
                    chat_id,
                    query,
                    ids,
                    total,
                }]
            }
            Action::GlobalSearch { spec } => self.run_global(spec, false),
            Action::MoreGlobal { spec } => {
                if self
                    .last_global
                    .as_ref()
                    .is_some_and(|page| page.spec == spec)
                {
                    self.global_page(spec, true)
                } else {
                    self.run_global(spec, false)
                }
            }
            Action::ResolveUser { query } => {
                let chat_ids: Vec<_> = self
                    .active()
                    .chats
                    .iter()
                    .chain(&self.active().archived)
                    .map(|chat| chat.id)
                    .collect();
                let candidates = rank_candidates(&query, self.active().contacts.clone(), |id| {
                    chat_ids.contains(&id)
                });
                if candidates.is_empty() {
                    vec![Effect::UserSearch {
                        query,
                        result: Err("No demo contact matches that search.".into()),
                    }]
                } else {
                    vec![Effect::UserSearch {
                        query,
                        result: Ok(candidates),
                    }]
                }
            }
            Action::TogglePin { chat_id, pinned } => {
                if let Some(chat) = self.chat_mut(chat_id) {
                    chat.pinned = pinned;
                    let chats = &mut self.active_mut().chats;
                    chats.sort_by(|a, b| {
                        if domain::updates::sorts_before(a, b) {
                            std::cmp::Ordering::Less
                        } else if domain::updates::sorts_before(b, a) {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    });
                    vec![Effect::PinToggled { chat_id, pinned }]
                } else {
                    vec![Self::status("That conversation is not in the demo.")]
                }
            }
            Action::Login { field: _, value: _ } => vec![Self::status(
                "Sign-in is disabled in the offline demo. Switch seeded accounts from the account card.",
            )],
            Action::LoginCancelled => {
                vec![Self::status("Sign-in cancelled; the demo stays signed in.")]
            }
            Action::AddAccount => vec![Self::status(
                "The demo has a fixed cast. Use the account card to switch.",
            )],
            Action::SwitchAccount { id } => {
                if !self.world.switch_to(id) {
                    return vec![Self::status("That account is not in the demo cast.")];
                }
                self.queue.clear();
                vec![Effect::SwitchAccount { id }]
            }
            Action::Logout => vec![Self::status(
                "Logout is disabled. Use Reset demo to start over.",
            )],
            Action::ExportSession { path: _ } => vec![Self::status(
                "Session export is unavailable in the browser demo.",
            )],
            Action::ImportSession { path: _ } => vec![Self::status(
                "Session import is unavailable in the browser demo.",
            )],
            Action::SubmitSessionPassphrase { value: _ }
            | Action::ChangeSessionPassphrase { new: _ } => {
                vec![Self::status("There is no session passphrase in the demo.")]
            }
            Action::OpenMedia {
                chat_id,
                message_id,
            } => {
                if self
                    .active()
                    .media
                    .get(&(chat_id, message_id))
                    .is_some_and(|media| media.downloadable)
                {
                    vec![Effect::RequestOpenMedia {
                        chat_id,
                        message_id,
                    }]
                } else {
                    vec![Self::status("That demo message has no available media.")]
                }
            }
            Action::SaveMedia {
                chat_id,
                message_id,
                path: _,
                typed: _,
            } => {
                if let Some(media) = self
                    .active()
                    .media
                    .get(&(chat_id, message_id))
                    .filter(|media| media.downloadable)
                {
                    vec![Effect::RequestSaveMedia {
                        chat_id,
                        message_id,
                        filename: media.filename.clone(),
                        mime: media.mime,
                        bytes: media.bytes.clone(),
                    }]
                } else {
                    vec![Self::status("That demo message has no media to save.")]
                }
            }
            Action::CancelMediaDownload { .. } => {
                vec![Self::status("No media download is running.")]
            }
            Action::SetArchived { chat_id, archived } => {
                let account = self.active_mut();
                let moved = if archived {
                    move_chat(&mut account.chats, &mut account.archived, chat_id)
                } else {
                    move_chat(&mut account.archived, &mut account.chats, chat_id)
                };
                if moved {
                    if (archived && account.default_chat == chat_id)
                        || (!archived && account.default_chat == 0)
                    {
                        account.default_chat = account.chats.first().map_or(0, |chat| chat.id);
                    }
                    vec![Effect::Archived { chat_id, archived }]
                } else {
                    vec![Self::status("That conversation is not in the demo.")]
                }
            }
            Action::SetBlocked {
                peer_id,
                name: _,
                blocked,
            } => {
                if blocked {
                    self.active_mut().blocked.insert(peer_id);
                } else {
                    self.active_mut().blocked.remove(&peer_id);
                }
                vec![Effect::Blocked { peer_id, blocked }]
            }
            Action::SetLocalName { peer_id, name } => {
                if let Some(value) = name.as_ref().filter(|value| !value.trim().is_empty()) {
                    self.active_mut()
                        .local_names
                        .insert(peer_id, value.trim().to_owned());
                } else {
                    self.active_mut().local_names.remove(&peer_id);
                }
                vec![Effect::LocalNameSaved { peer_id, name }]
            }
            Action::DeleteConversation {
                chat_id,
                name: _,
                both: _,
            } => {
                let account = self.active_mut();
                let before = account.chats.len() + account.archived.len();
                account.chats.retain(|chat| chat.id != chat_id);
                account.archived.retain(|chat| chat.id != chat_id);
                account.messages.remove(&chat_id);
                account.media.retain(|(id, _), _| *id != chat_id);
                if account.default_chat == chat_id {
                    account.default_chat = account.chats.first().map_or(0, |chat| chat.id);
                }
                if before == account.chats.len() + account.archived.len() {
                    vec![Self::status("That conversation is not in the demo.")]
                } else {
                    vec![Effect::ConversationDeleted { chat_id }]
                }
            }
            Action::ReportPeer {
                peer_id,
                name: _,
                reason,
            } => {
                self.reports.push((peer_id, reason));
                vec![
                    Effect::Reported { peer_id, reason },
                    Self::status("Report recorded locally. No report was sent to Telegram."),
                ]
            }
        }
    }

    pub fn schedule(&mut self, delay_ms: u64, effect: Effect) {
        let item = Scheduled {
            due_ms: self.now_ms.saturating_add(delay_ms),
            effect,
        };
        let index = self
            .queue
            .iter()
            .position(|queued| queued.due_ms > item.due_ms)
            .unwrap_or(self.queue.len());
        self.queue.insert(index, item);
    }

    pub fn advance(&mut self, delta_ms: u64) -> Vec<Effect> {
        self.now_ms = self.now_ms.saturating_add(delta_ms);
        let mut due = Vec::new();
        while self
            .queue
            .front()
            .is_some_and(|item| item.due_ms <= self.now_ms)
        {
            if let Some(item) = self.queue.pop_front() {
                match &item.effect {
                    Effect::Update(UpdateEvent::NewMessage(message)) if !message.is_outgoing => {
                        if self.active().blocked.contains(&message.chat_id) {
                            continue;
                        }
                        self.push_message(message);
                        if self.open_chat_id != Some(message.chat_id)
                            && let Some(chat) = self.chat_mut(message.chat_id)
                        {
                            chat.unread_count = chat.unread_count.saturating_add(1);
                        }
                    }
                    Effect::Sent { message, .. }
                        if !self
                            .active()
                            .messages(message.chat_id)
                            .iter()
                            .any(|saved| saved.id == message.id) =>
                    {
                        self.push_message(message);
                    }
                    _ => {}
                }
                due.push(item.effect);
            }
        }
        due
    }

    /// Sets the chat currently shown by the driver before it advances time.
    pub fn set_open_chat_id(&mut self, chat_id: Option<i64>) {
        self.open_chat_id = chat_id;
    }

    /// Records metadata for a user-selected attachment without reading its bytes.
    pub fn attachment_message(
        &mut self,
        chat_id: i64,
        filename: String,
        _kind: MediaKind,
    ) -> Option<Message> {
        if !self.has_chat(chat_id) {
            return None;
        }
        let message = self.new_message(chat_id, filename, true, MessageStatus::Sent, None, None);
        self.push_message(&message);
        Some(message)
    }

    pub fn schedule_arrival(
        &mut self,
        chat_id: i64,
        text: &str,
        media: Option<MediaKind>,
        delay_ms: u64,
    ) {
        if self.active().blocked.contains(&chat_id) {
            return;
        }
        let message = self.new_message(
            chat_id,
            text.to_owned(),
            false,
            MessageStatus::Received,
            None,
            media,
        );
        self.schedule(delay_ms, Effect::Update(UpdateEvent::NewMessage(message)));
    }

    pub fn schedule_typing(&mut self, chat_id: i64, typing: bool, delay_ms: u64) {
        self.schedule(
            delay_ms,
            Effect::Update(UpdateEvent::PeerTyping { chat_id, typing }),
        );
    }

    pub fn schedule_presence(&mut self, chat_id: i64, presence: Presence, delay_ms: u64) {
        if let Some(chat) = self.chat_mut(chat_id) {
            chat.presence = Some(presence);
        }
        self.schedule(
            delay_ms,
            Effect::Update(UpdateEvent::PeerStatus { chat_id, presence }),
        );
    }

    pub fn schedule_read_receipt(&mut self, chat_id: i64, max_id: i64, delay_ms: u64) {
        self.schedule(
            delay_ms,
            Effect::Update(UpdateEvent::ReadReceipt { chat_id, max_id }),
        );
    }

    /// Builds a local history page around a loaded message identifier.
    #[must_use]
    pub fn history_page(
        &self,
        chat_id: i64,
        direction: FetchDirection,
        anchor_id: i64,
        limit: usize,
    ) -> Option<Effect> {
        if limit == 0 || direction == FetchDirection::Latest {
            return None;
        }
        let messages = self.active().messages(chat_id);
        if messages.is_empty() {
            return None;
        }

        let (page, exhausted) = match direction {
            FetchDirection::Older => {
                let older: Vec<_> = messages
                    .iter()
                    .filter(|message| message.id < anchor_id)
                    .collect();
                let start = older.len().saturating_sub(limit);
                (
                    older[start..]
                        .iter()
                        .map(|message| (*message).clone())
                        .collect(),
                    start == 0,
                )
            }
            FetchDirection::Newer => {
                let newer: Vec<_> = messages
                    .iter()
                    .filter(|message| message.id > anchor_id)
                    .collect();
                (
                    newer
                        .iter()
                        .take(limit)
                        .map(|message| (*message).clone())
                        .collect(),
                    newer.len() <= limit,
                )
            }
            FetchDirection::Latest => return None,
        };
        Some(Effect::HistoryPage {
            chat_id,
            direction,
            messages: page,
            exhausted,
        })
    }

    pub fn record_failed_upload(&mut self, chat_id: i64, temp_id: i64, kind: MediaKind) {
        if !self
            .failed_uploads
            .iter()
            .any(|entry| entry.0 == chat_id && entry.1 == temp_id)
        {
            self.failed_uploads.push((chat_id, temp_id, kind));
        }
    }

    fn run_global(&mut self, spec: SearchSpec, append: bool) -> Vec<Effect> {
        let all_messages = self.active().messages.values().flatten();
        let page = search::global_hits(&spec, all_messages);
        self.last_global = Some(GlobalPage {
            spec: spec.clone(),
            hits: page,
            cursor: 0,
        });
        self.global_page(spec, append)
    }

    fn global_page(&mut self, spec: SearchSpec, append: bool) -> Vec<Effect> {
        let Some(page) = self.last_global.as_mut() else {
            return vec![Self::status("No demo search page is available.")];
        };
        let offset = if append { page.cursor } else { 0 };
        let hits = page
            .hits
            .iter()
            .skip(offset)
            .take(GLOBAL_PAGE_SIZE)
            .cloned()
            .collect::<Vec<_>>();
        let total = page.hits.len();
        page.cursor = (offset + hits.len()).min(total);
        vec![Effect::GlobalSearched {
            spec,
            hits,
            total,
            dropped: 0,
            has_more: page.cursor < total,
            append,
        }]
    }

    fn new_message(
        &mut self,
        chat_id: i64,
        text: String,
        is_outgoing: bool,
        status: MessageStatus,
        reply_to: Option<i64>,
        media: Option<MediaKind>,
    ) -> Message {
        let id = self.next_message_id;
        self.next_message_id = self.next_message_id.saturating_add(1);
        Message {
            id,
            chat_id,
            text: Cow::Owned(text),
            timestamp: self
                .world
                .base_timestamp
                .saturating_add(i64::try_from(self.now_ms / 1000).unwrap_or(i64::MAX)),
            status,
            is_outgoing,
            reply_to,
            media,
            media_id: None,
        }
    }

    fn push_message(&mut self, message: &Message) {
        self.active_mut()
            .messages
            .entry(message.chat_id)
            .or_default()
            .push(message.clone());
        if let Some(chat) = self.chat_mut(message.chat_id) {
            chat.last_message = Some(Cow::Owned(message.display_body().to_owned()));
            chat.last_message_id = Some(message.id);
            chat.last_timestamp = Some(message.timestamp);
        }
    }

    fn has_chat(&self, chat_id: i64) -> bool {
        self.active().chat(chat_id).is_some()
    }
    fn active(&self) -> &crate::DemoAccount {
        self.world.active()
    }
    fn active_mut(&mut self) -> &mut crate::DemoAccount {
        self.world.active_mut()
    }
    fn chat_mut(&mut self, chat_id: i64) -> Option<&mut Chat> {
        let account = self.active_mut();
        account
            .chats
            .iter_mut()
            .chain(&mut account.archived)
            .find(|chat| chat.id == chat_id)
    }
    fn message_mut(&mut self, chat_id: i64, message_id: i64) -> Option<&mut Message> {
        self.active_mut()
            .messages
            .get_mut(&chat_id)?
            .iter_mut()
            .find(|message| message.id == message_id)
    }
    fn status(sentence: &str) -> Effect {
        Effect::Status(sentence.to_owned())
    }
}

fn move_chat(from: &mut Vec<Chat>, to: &mut Vec<Chat>, id: i64) -> bool {
    let Some(index) = from.iter().position(|chat| chat.id == id) else {
        return false;
    };
    to.push(from.remove(index));
    true
}
