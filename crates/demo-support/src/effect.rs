use domain::{
    account::Account,
    global_search::{GlobalHit, SearchSpec},
    message::{MediaKind, Message},
    report::ReportReason,
    updates::UpdateEvent,
    user::UserCandidate,
};
use tui::app::FetchDirection;

/// Result of a simulated action. The browser driver applies these to `App`.
#[derive(Debug, Clone)]
pub enum Effect {
    Update(UpdateEvent),
    HistoryPage {
        chat_id: i64,
        direction: FetchDirection,
        messages: Vec<Message>,
        exhausted: bool,
    },
    Sent {
        chat_id: i64,
        temp_id: i64,
        message: Message,
    },
    SendFailed {
        chat_id: i64,
        temp_id: i64,
        reason: String,
    },
    UploadProgress {
        chat_id: i64,
        temp_id: i64,
        sent: u64,
        total: u64,
    },
    RequestFilePicker {
        chat_id: i64,
        temp_id: i64,
        kind: MediaKind,
    },
    RequestOpenMedia {
        chat_id: i64,
        message_id: i64,
    },
    RequestSaveMedia {
        chat_id: i64,
        message_id: i64,
        filename: String,
        mime: &'static str,
        bytes: Vec<u8>,
    },
    Edited {
        chat_id: i64,
        message_id: i64,
        text: String,
    },
    Deleted {
        chat_id: i64,
        message_ids: Vec<i64>,
    },
    Forwarded {
        dest_chat_id: i64,
        messages: Vec<Message>,
    },
    Searched {
        chat_id: i64,
        query: String,
        ids: Vec<i64>,
        total: usize,
    },
    GlobalSearched {
        spec: SearchSpec,
        hits: Vec<GlobalHit>,
        total: usize,
        dropped: usize,
        has_more: bool,
        append: bool,
    },
    Contact {
        peer_id: i64,
        blocked: bool,
        result: Result<Account, String>,
    },
    Contacts {
        result: Result<Vec<UserCandidate>, String>,
    },
    UserSearch {
        query: String,
        result: Result<Vec<UserCandidate>, String>,
    },
    PinToggled {
        chat_id: i64,
        pinned: bool,
    },
    Archived {
        chat_id: i64,
        archived: bool,
    },
    Blocked {
        peer_id: i64,
        blocked: bool,
    },
    LocalNameSaved {
        peer_id: i64,
        name: Option<String>,
    },
    ConversationDeleted {
        chat_id: i64,
    },
    Reported {
        peer_id: i64,
        reason: ReportReason,
    },
    SwitchAccount {
        id: i64,
    },
    Status(String),
    UpdateUnavailable,
}
