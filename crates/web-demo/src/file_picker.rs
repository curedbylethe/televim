use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use domain::message::MediaKind;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, HtmlInputElement, Window};

use crate::browser::ResultEvent;

#[derive(Default)]
pub(crate) struct Picker {
    pending: Option<Pending>,
}

struct Pending {
    input: HtmlInputElement,
    window: Window,
    _change: Closure<dyn FnMut(Event)>,
    _cancel: Closure<dyn FnMut(Event)>,
    _focus: Closure<dyn FnMut(Event)>,
}

impl Picker {
    pub fn open(
        &mut self,
        results: Rc<RefCell<Vec<ResultEvent>>>,
        chat_id: i64,
        temp_id: i64,
        requested: MediaKind,
    ) -> Result<(), JsValue> {
        if self.pending.is_some() {
            return Err(JsValue::from_str("a file picker is already open"));
        }
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("window is unavailable"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("document is unavailable"))?;
        let input = document
            .create_element("input")?
            .dyn_into::<HtmlInputElement>()?;
        input.set_type("file");
        input.set_accept(accept(requested));
        input.set_attribute("aria-label", "Choose a fictional demo attachment")?;
        input.set_attribute("data-televim-demo-file-picker", "")?;
        input.set_attribute("hidden", "")?;
        document
            .body()
            .ok_or_else(|| JsValue::from_str("document body is unavailable"))?
            .append_child(&input)?;

        let settled = Rc::new(Cell::new(false));
        let event_input = input.clone();
        let event_results = results.clone();
        let event_settled = Rc::clone(&settled);
        let change = Closure::wrap(Box::new(move |_event: Event| {
            if event_settled.replace(true) {
                return;
            }
            let file = event_input.files().and_then(|files| files.get(0));
            remove_input(&event_input);
            event_results
                .borrow_mut()
                .push(ResultEvent::FilePicked(crate::browser::PickedFile {
                    chat_id,
                    temp_id,
                    requested,
                    file,
                }));
        }) as Box<dyn FnMut(Event)>);

        let event_input = input.clone();
        let event_settled = Rc::clone(&settled);
        let event_results = results.clone();
        let cancel = Closure::wrap(Box::new(move |_event: Event| {
            if event_settled.replace(true) {
                return;
            }
            remove_input(&event_input);
            event_results
                .borrow_mut()
                .push(ResultEvent::FilePicked(crate::browser::PickedFile {
                    chat_id,
                    temp_id,
                    requested,
                    file: None,
                }));
        }) as Box<dyn FnMut(Event)>);

        let event_input = input.clone();
        let event_settled = settled;
        let focus = Closure::wrap(Box::new(move |_event: Event| {
            if event_input.files().is_some_and(|files| files.length() > 0) {
                return;
            }
            if event_settled.replace(true) {
                return;
            }
            remove_input(&event_input);
            results
                .borrow_mut()
                .push(ResultEvent::FilePicked(crate::browser::PickedFile {
                    chat_id,
                    temp_id,
                    requested,
                    file: None,
                }));
        }) as Box<dyn FnMut(Event)>);

        input.add_event_listener_with_callback("change", change.as_ref().unchecked_ref())?;
        input.add_event_listener_with_callback("cancel", cancel.as_ref().unchecked_ref())?;
        window.add_event_listener_with_callback("focus", focus.as_ref().unchecked_ref())?;
        self.pending = Some(Pending {
            input: input.clone(),
            window,
            _change: change,
            _cancel: cancel,
            _focus: focus,
        });
        input.click();
        Ok(())
    }

    pub fn clear(&mut self) {
        if let Some(pending) = self.pending.take() {
            remove_input(&pending.input);
            let _ = pending.window.remove_event_listener_with_callback(
                "focus",
                pending._focus.as_ref().unchecked_ref(),
            );
        }
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.clear();
    }
}

fn accept(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo | MediaKind::Sticker | MediaKind::AnimatedSticker => "image/*",
        MediaKind::Video | MediaKind::Gif => "video/*,image/*",
        MediaKind::Voice => "audio/*",
        MediaKind::File => "*/*",
    }
}

pub(crate) fn selected_kind(requested: MediaKind, mime: &str, filename: &str) -> Option<MediaKind> {
    let mime = mime.to_ascii_lowercase();
    let extension = filename
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase();
    let actual = if mime == "image/gif" || extension == "gif" {
        MediaKind::Gif
    } else if mime.starts_with("image/")
        || matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp" | "svg")
    {
        MediaKind::Photo
    } else if mime.starts_with("video/") || matches!(extension.as_str(), "mp4" | "mov" | "webm") {
        MediaKind::Video
    } else if mime.starts_with("audio/")
        || matches!(extension.as_str(), "mp3" | "wav" | "ogg" | "m4a")
    {
        MediaKind::Voice
    } else {
        MediaKind::File
    };
    let compatible = match requested {
        MediaKind::Photo | MediaKind::Sticker | MediaKind::AnimatedSticker => {
            matches!(actual, MediaKind::Photo | MediaKind::Gif)
        }
        MediaKind::Video => actual == MediaKind::Video,
        MediaKind::Gif => actual == MediaKind::Gif,
        MediaKind::Voice => actual == MediaKind::Voice,
        MediaKind::File => true,
    };
    if compatible {
        Some(match requested {
            MediaKind::Sticker | MediaKind::AnimatedSticker => requested,
            MediaKind::File => actual,
            _ => requested,
        })
    } else {
        None
    }
}

fn remove_input(input: &HtmlInputElement) {
    if let Some(parent) = input.parent_node() {
        let _ = parent.remove_child(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_type_must_match_requested_media() {
        assert_eq!(
            selected_kind(MediaKind::Photo, "image/png", "sketch.png"),
            Some(MediaKind::Photo)
        );
        assert_eq!(
            selected_kind(MediaKind::Voice, "", "note.mp3"),
            Some(MediaKind::Voice)
        );
        assert_eq!(
            selected_kind(MediaKind::Photo, "application/pdf", "notes.pdf"),
            None
        );
        assert_eq!(
            selected_kind(MediaKind::File, "image/gif", "loop.gif"),
            Some(MediaKind::Gif)
        );
    }
}
