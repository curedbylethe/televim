use demo_support::{DemoAccount, DemoSimulator, DemoWorld, Effect};
use domain::{message::Message, presence::Presence, updates::UpdateEvent};
use tui::{
    App, ConnectionState, SessionStore,
    app::{Action, FetchDirection},
    bidi::BidiMode,
    state::ui::{GraphicsMode, StickerMode},
};
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{
    browser::{BrowserBridge, PickedFile},
    file_picker,
};

const OFFLINE_STATUS: &str = "Offline demo · fictional conversations · no Telegram connection.";
const HISTORY_PAGE_SIZE: usize = 8;

pub struct Driver {
    pub app: App,
    pub sim: DemoSimulator,
    pub browser: BrowserBridge,
    base_timestamp: i64,
    started: Instant,
    last_tick: Instant,
    offset_seconds: i64,
    seeded_chat_id: i64,
    exit: DriverExit,
    quit_notified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DriverExit {
    #[default]
    None,
    Quit,
}

impl Driver {
    #[must_use]
    pub fn new() -> Self {
        let base_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_secs();
        let base_timestamp =
            i64::try_from(base_timestamp).expect("current Unix timestamp fits in signed seconds");
        let offset_seconds = -(js_sys::Date::new_0().get_timezone_offset() as i64 * 60);
        let sim = DemoSimulator::new(DemoWorld::new(base_timestamp));
        let seeded_chat_id = sim.world.active().default_chat;
        let app = build_app(sim.world.active(), base_timestamp, offset_seconds);
        let now = Instant::now();
        let mut driver = Self {
            app,
            sim,
            browser: BrowserBridge::default(),
            base_timestamp,
            started: now,
            last_tick: now,
            offset_seconds,
            seeded_chat_id,
            exit: DriverExit::None,
            quit_notified: false,
        };
        driver.schedule_intro();
        driver
    }

    pub fn tick(&mut self) {
        if self.exit != DriverExit::None {
            return;
        }
        for picked in self.browser.process(&mut self.app) {
            self.apply_picked_file(picked);
        }
        let now = Instant::now();
        let delta_ms =
            u64::try_from(now.duration_since(self.last_tick).as_millis()).unwrap_or(u64::MAX);
        self.last_tick = now;

        let open_chat_id = self.open_chat_id();
        self.sim
            .set_open_chat_id((open_chat_id != 0).then_some(open_chat_id));
        for effect in self.sim.advance(delta_ms) {
            self.apply_effect(effect);
        }

        #[cfg(debug_assertions)]
        let mut drained = false;
        for _ in 0..100 {
            let mut any = false;
            while let Some(action) = self.app.take_action() {
                any = true;
                self.dispatch(action);
            }
            while self.app.take_session_action().is_some() {
                any = true;
                self.app
                    .set_status("Session management is disabled in the offline demo.");
            }
            while let Some(action) = self.app.take_local_name_action() {
                any = true;
                self.dispatch(action);
            }
            if !any {
                #[cfg(debug_assertions)]
                {
                    drained = true;
                }
                break;
            }
        }
        #[cfg(debug_assertions)]
        if !drained {
            self.app.flash("demo action loop stopped");
        }

        let elapsed = i64::try_from(self.started.elapsed().as_secs()).unwrap_or(i64::MAX);
        self.app.record_now(
            self.base_timestamp.saturating_add(elapsed),
            self.offset_seconds,
        );
        self.app.expire_status(now);
        self.app.expire_typing(now);
        self.app.open_settled_card(now);
        self.seed_open_chat();
        self.request_history_page();
        self.sync_reads();
        if let Some(text) = self.app.take_clipboard() {
            self.browser.copy(&mut self.app, text);
        }
        if self.app.ui.should_quit && !self.quit_notified {
            self.exit = DriverExit::Quit;
        }
    }

    pub fn handle_key(&mut self, key: tui::key::KeyEvent) {
        self.app.handle_key(key);
    }

    pub fn render(&self, frame: &mut ratzilla::ratatui::Frame<'_>) {
        if self.exit == DriverExit::None && !self.app.ui.should_quit {
            self.app.render(frame);
        }
    }

    pub fn take_exit(&mut self) -> DriverExit {
        let exit = std::mem::take(&mut self.exit);
        if exit == DriverExit::Quit {
            self.quit_notified = true;
        }
        exit
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn switch_account(&mut self, id: i64) -> bool {
        if !self.sim.world.switch_to(id) {
            self.app.set_status("No such demo account.");
            return false;
        }
        self.app = build_app(
            self.sim.world.active(),
            self.base_timestamp,
            self.offset_seconds,
        );
        self.seeded_chat_id = self.sim.world.active().default_chat;
        true
    }

    fn schedule_intro(&mut self) {
        let account = self.sim.world.active();
        let chat_id = account.default_chat;
        let other_chat = account
            .chats
            .iter()
            .find(|chat| chat.id != chat_id)
            .map_or(chat_id, |chat| chat.id);
        let last_incoming = account
            .messages(chat_id)
            .iter()
            .rev()
            .find(|message| !message.is_outgoing)
            .map_or(0, |message| message.id);

        self.sim.set_open_chat_id(Some(chat_id));
        self.sim.schedule_presence(chat_id, Presence::Online, 1_500);
        self.sim.schedule_typing(chat_id, true, 2_000);
        self.sim.schedule_typing(chat_id, false, 5_000);
        self.sim.schedule_arrival(
            chat_id,
            "The lantern is lit. I'll be there in ten.",
            None,
            6_000,
        );
        self.sim
            .schedule_read_receipt(chat_id, last_incoming, 9_000);
        self.sim
            .schedule_presence(other_chat, Presence::Recently, 12_000);
    }

    fn dispatch(&mut self, action: Action) {
        let effects = self.sim.dispatch(&self.app, action);
        for effect in effects {
            self.apply_effect(effect);
        }
    }

    fn apply_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Update(event) => {
                let _ = self.app.apply_update(&event);
            }
            Effect::HistoryPage {
                chat_id,
                direction,
                messages,
                exhausted,
            } => {
                if chat_id != self.open_chat_id() {
                    return;
                }
                match direction {
                    FetchDirection::Latest => {
                        self.app.apply_latest(messages);
                    }
                    FetchDirection::Older => {
                        self.app.apply_older(messages);
                    }
                    FetchDirection::Newer => {
                        self.app.apply_newer(messages);
                    }
                }
                self.app.end_fetch(direction);
                if exhausted {
                    self.app.exhaust(direction);
                }
            }
            Effect::Sent {
                chat_id,
                temp_id,
                message,
            } => {
                self.app.end_send(temp_id);
                let _ = self
                    .app
                    .apply_update(&UpdateEvent::NewMessage(message.clone()));
                if self.open_chat_id() == chat_id {
                    self.app.confirm_sent(temp_id, message.clone());
                }
            }
            Effect::SendFailed {
                temp_id, reason, ..
            } => {
                self.app.fail_send(temp_id, reason.clone());
                self.app.flash(reason);
            }
            Effect::UploadProgress { sent, total, .. } => {
                self.app
                    .set_status(format!("Uploading {sent}/{total} bytes…"));
            }
            Effect::MediaProgress {
                received, total, ..
            } => {
                self.app
                    .set_status(format!("Downloading {received}/{total} bytes…"));
            }
            Effect::RequestFilePicker {
                chat_id,
                temp_id,
                kind,
            } => {
                if let Err(reason) = self
                    .browser
                    .pick_file(&mut self.app, chat_id, temp_id, kind)
                {
                    self.sim.record_failed_upload(chat_id, temp_id);
                    self.app.fail_send(temp_id, reason.to_owned());
                }
            }
            Effect::RequestOpenMedia {
                chat_id,
                message_id,
            } => {
                self.browser.open_media(
                    &mut self.app,
                    self.sim.world.active(),
                    chat_id,
                    message_id,
                );
            }
            Effect::RequestSaveMedia {
                chat_id,
                message_id,
                ..
            } => {
                self.browser.save_media(
                    &mut self.app,
                    self.sim.world.active(),
                    chat_id,
                    message_id,
                );
            }
            Effect::MediaCancelled {
                chat_id,
                message_id,
            } => {
                self.app.conversation.downloads.forget(chat_id, message_id);
                self.app.flash("Download cancelled.");
            }
            Effect::Edited {
                chat_id,
                message_id,
                text,
            } => {
                let _ = self.app.apply_update(&UpdateEvent::MessageEdited {
                    chat_id,
                    message_id,
                    new_text: std::borrow::Cow::Owned(text),
                });
            }
            Effect::Deleted { message_ids, .. } => {
                let _ = self
                    .app
                    .apply_update(&UpdateEvent::MessagesDeleted { message_ids });
            }
            Effect::Forwarded {
                dest_chat_id,
                messages,
            } => {
                for message in messages {
                    let _ = self.app.apply_update(&UpdateEvent::NewMessage(message));
                }
                let destination = self
                    .sim
                    .world
                    .active()
                    .chat(dest_chat_id)
                    .map_or_else(|| "conversation".to_owned(), |chat| chat.title.clone());
                self.app.set_status(format!("Forwarded to {destination}."));
            }
            Effect::Searched {
                chat_id,
                query,
                ids,
                total,
            } => {
                self.app.apply_searched(chat_id, &query, ids, total);
            }
            Effect::GlobalSearched {
                spec,
                hits,
                total,
                dropped,
                has_more,
                append,
            } => {
                if append {
                    self.app
                        .append_global_search(&spec, hits, total, dropped, has_more);
                } else {
                    self.app
                        .adopt_global_search(&spec, hits, total, dropped, has_more);
                }
            }
            Effect::Contact {
                peer_id,
                blocked,
                result,
            } => self.app.set_contact(peer_id, Some(blocked), result),
            Effect::Contacts { result } => match result {
                Ok(people) => self.app.conversation.contacts.adopt(people),
                Err(reason) => self.app.conversation.contacts.fail(reason),
            },
            Effect::UserSearch { query, result } => match result {
                Ok(candidates) => {
                    self.app.apply_users(&query, candidates);
                }
                Err(reason) => {
                    self.app.fail_users(&query, reason);
                }
            },
            Effect::PinToggled { chat_id, pinned } => self.app.set_pinned(chat_id, pinned),
            Effect::Archived { chat_id, archived } => {
                self.app.set_archived(chat_id, archived);
            }
            Effect::Blocked { peer_id, blocked } => self.app.set_blocked(peer_id, blocked),
            Effect::LocalNameSaved { peer_id, name } => {
                self.app.set_local_name(peer_id, name.as_deref());
            }
            Effect::ConversationDeleted { chat_id } => self.app.remove_conversation(chat_id),
            Effect::Reported { peer_id, reason } => self.app.set_status(format!(
                "Report recorded locally for {peer_id} ({reason:?}); nothing was sent to Telegram."
            )),
            Effect::SwitchAccount { id } => {
                self.switch_account(id);
            }
            Effect::Status(sentence) => self.app.set_status(sentence),
            Effect::UpdateUnavailable => {
                self.app.update_cancelled();
                self.app.flash("Updates are disabled in the offline demo.");
            }
        }
    }

    fn apply_picked_file(&mut self, picked: PickedFile) {
        let Some(file) = picked.file else {
            self.fail_picked_file(
                picked.chat_id,
                picked.temp_id,
                "No file selected; nothing was sent.",
            );
            return;
        };
        let name = file.name();
        let Some(kind) = file_picker::selected_kind(picked.requested, &file.type_(), &name) else {
            self.fail_picked_file(
                picked.chat_id,
                picked.temp_id,
                "That file type does not match the requested attachment.",
            );
            return;
        };
        let Some(message) = self
            .sim
            .attachment_message(picked.chat_id, name.clone(), kind)
        else {
            self.fail_picked_file(
                picked.chat_id,
                picked.temp_id,
                "That conversation is no longer in the demo.",
            );
            return;
        };
        self.app.end_send(picked.temp_id);
        let _ = self
            .app
            .apply_update(&UpdateEvent::NewMessage(message.clone()));
        if self.open_chat_id() == picked.chat_id {
            self.app.confirm_sent(picked.temp_id, message.clone());
        }
        self.app.set_status(format!(
            "Recorded {name} in the offline demo; file bytes were not read or uploaded."
        ));
    }

    fn sync_reads(&mut self) {
        let chat_id = self.open_chat_id();
        if self
            .app
            .chats()
            .iter()
            .any(|chat| chat.id == chat_id && chat.unread_count > 0)
        {
            self.clear_world_unread(chat_id);
            self.app.mark_chat_read(chat_id);
        }
    }

    fn seed_open_chat(&mut self) {
        let chat_id = self.app.conversation.conversation.window.chat_id;
        if chat_id == 0 || chat_id == self.seeded_chat_id {
            return;
        }
        self.seeded_chat_id = chat_id;
        let messages = self.sim.world.active().messages(chat_id);
        self.app.apply_latest(latest_page(messages));
        self.app.exhaust(FetchDirection::Newer);
        if messages.len() <= HISTORY_PAGE_SIZE {
            self.app.exhaust(FetchDirection::Older);
        }
    }

    fn request_history_page(&mut self) {
        let chat_id = self.open_chat_id();
        if chat_id == 0 {
            return;
        }
        let (direction, anchor_id) = if self.app.wants_older() {
            let Some(message) = self.app.conversation.conversation.window.iter().next() else {
                return;
            };
            (FetchDirection::Older, message.id)
        } else if self.app.wants_newer() {
            let Some(message) = self.app.conversation.conversation.window.iter().last() else {
                return;
            };
            (FetchDirection::Newer, message.id)
        } else {
            return;
        };

        self.app.begin_fetch(direction);
        if let Some(effect) =
            self.sim
                .history_page(chat_id, direction, anchor_id, HISTORY_PAGE_SIZE)
        {
            self.apply_effect(effect);
        } else {
            self.app.end_fetch(direction);
            self.app.exhaust(direction);
        }
    }

    pub(crate) fn open_chat_id(&self) -> i64 {
        self.app.conversation.conversation.window.chat_id
    }

    fn fail_picked_file(&mut self, chat_id: i64, temp_id: i64, reason: &str) {
        self.sim.record_failed_upload(chat_id, temp_id);
        self.app.fail_send(temp_id, reason.to_owned());
    }

    fn clear_world_unread(&mut self, chat_id: i64) {
        let account = self.sim.world.active_mut();
        for chat in account
            .chats
            .iter_mut()
            .chain(account.archived.iter_mut())
            .filter(|chat| chat.id == chat_id)
        {
            chat.unread_count = 0;
        }
    }
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}

fn build_app(account: &DemoAccount, base_timestamp: i64, offset_seconds: i64) -> App {
    let mut app = App::new()
        .with_bidi(BidiMode::Visual)
        .with_stickers(StickerMode::Token)
        .with_graphics(GraphicsMode::Halfblocks);
    app.conversation = app.conversation.with_jumplist_depth(100);
    app.set_client_available(true);
    app.session.credentials_configured = false;
    app.set_session_store(SessionStore::EncryptedFile(std::path::PathBuf::from(
        "(in-memory demo)",
    )));
    app.set_connection(ConnectionState::Offline);
    app.set_account(Ok(account.account.clone()));
    app.set_known_accounts(account.known_accounts.clone());
    let mut chats = account.chats.clone();
    let mut archived = account.archived.clone();
    for chat in chats.iter_mut().chain(&mut archived) {
        if let Some(messages) = account.messages.get(&chat.id) {
            if let Some(message) = messages.last() {
                chat.last_message =
                    Some(std::borrow::Cow::Owned(message.display_body().to_owned()));
                chat.last_message_id = Some(message.id);
                chat.last_timestamp = Some(message.timestamp);
            } else {
                chat.last_message = None;
                chat.last_message_id = None;
                chat.last_timestamp = None;
            }
        }
    }
    app.set_chats(chats);
    app.set_archived_chats(archived);
    for (peer_id, name) in &account.local_names {
        app.set_local_name(*peer_id, Some(name));
    }
    app.select_chat_by_id(account.default_chat);
    let messages = account.messages(account.default_chat);
    app.apply_latest(latest_page(messages));
    app.exhaust(FetchDirection::Newer);
    if messages.len() <= HISTORY_PAGE_SIZE {
        app.exhaust(FetchDirection::Older);
    }
    app.record_now(base_timestamp, offset_seconds);
    app.set_status(OFFLINE_STATUS);
    app
}

fn latest_page(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .skip(messages.len().saturating_sub(HISTORY_PAGE_SIZE))
        .cloned()
        .collect()
}
