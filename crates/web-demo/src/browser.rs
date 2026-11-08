use std::{cell::RefCell, rc::Rc};

use demo_support::DemoAccount;
use domain::message::MediaKind;
use tui::App;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Document, Event, HtmlDialogElement, Url};

use crate::{clipboard, file_picker, media_viewer};

pub(crate) struct PickedFile {
    pub chat_id: i64,
    pub temp_id: i64,
    pub requested: MediaKind,
    pub file: Option<web_sys::File>,
}

pub(crate) enum ResultEvent {
    ClipboardCopied,
    ClipboardDenied(String),
    FilePicked(PickedFile),
}

pub struct BrowserBridge {
    dialog: Option<HtmlDialogElement>,
    close_listener: Option<Closure<dyn FnMut(Event)>>,
    dialog_url: Rc<RefCell<Option<String>>>,
    results: Rc<RefCell<Vec<ResultEvent>>>,
    picker: file_picker::Picker,
}

impl Default for BrowserBridge {
    fn default() -> Self {
        Self {
            dialog: None,
            close_listener: None,
            dialog_url: Rc::new(RefCell::new(None)),
            results: Rc::new(RefCell::new(Vec::new())),
            picker: file_picker::Picker::default(),
        }
    }
}

impl BrowserBridge {
    pub fn copy(&mut self, app: &mut App, text: String) {
        if clipboard::write(self.results.clone(), text.clone()).is_ok() {
            app.set_status("Copying to clipboard…");
        } else {
            self.show_clipboard_fallback(app, text);
        }
    }

    pub fn pick_file(&mut self, app: &mut App, chat_id: i64, temp_id: i64, kind: MediaKind) {
        if self
            .picker
            .open(self.results.clone(), chat_id, temp_id, kind)
            .is_err()
        {
            app.fail_send(
                temp_id,
                "The browser file picker could not open.".to_owned(),
            );
        } else {
            app.set_status("Choose a fictional demo attachment from this device.");
        }
    }

    pub fn open_media(
        &mut self,
        app: &mut App,
        account: &DemoAccount,
        chat_id: i64,
        message_id: i64,
    ) {
        let Some(media) = account.media.get(&(chat_id, message_id)) else {
            app.flash("This attachment is not available in the demo.");
            return;
        };
        let Ok((document, dialog)) = self.prepare_dialog() else {
            app.flash("The browser media viewer could not open.");
            return;
        };
        if dialog.open() {
            app.set_status("Close the current demo panel first.");
            return;
        }
        dialog.set_inner_html("");
        match media_viewer::populate(&document, &dialog, media) {
            Ok(url) => {
                *self.dialog_url.borrow_mut() = url;
                if dialog.show_modal().is_err() {
                    app.flash("The browser media viewer could not open.");
                } else {
                    app.set_status("Fictional demo media is open.");
                }
            }
            Err(_) => app.flash("The browser media viewer could not open."),
        }
    }

    pub fn save_media(
        &mut self,
        app: &mut App,
        account: &DemoAccount,
        chat_id: i64,
        message_id: i64,
    ) {
        let Some(media) = account.media.get(&(chat_id, message_id)) else {
            app.flash("This attachment is not available in the demo.");
            return;
        };
        if !media.downloadable {
            app.flash("This attachment cannot be downloaded in the demo.");
            return;
        }
        match media_viewer::download(media) {
            Ok(()) => app.set_status(format!("Download started: {}.", media.filename)),
            Err(_) => app.flash("Download could not start; no file was saved."),
        }
    }

    pub fn process(&mut self, app: &mut App) -> Vec<PickedFile> {
        let results = std::mem::take(&mut *self.results.borrow_mut());
        let mut picked = Vec::new();
        for result in results {
            match result {
                ResultEvent::ClipboardCopied => app.flash("Copied to clipboard."),
                ResultEvent::ClipboardDenied(text) => self.show_clipboard_fallback(app, text),
                ResultEvent::FilePicked(file) => {
                    self.picker.clear();
                    picked.push(file);
                }
            }
        }
        picked
    }

    pub fn show_clipboard_fallback(&mut self, app: &mut App, text: String) {
        let Ok((document, dialog)) = self.prepare_dialog() else {
            app.flash("Clipboard access was denied; no selectable fallback is available.");
            return;
        };
        if dialog.open() {
            app.set_status("Close the current demo panel before copying text.");
            return;
        }
        dialog.set_inner_html("");
        let result = (|| {
            let title = element(&document, "h2", "Clipboard fallback")?;
            let message = element(
                &document,
                "p",
                "Clipboard access was denied. The text is selected; press Ctrl/Cmd+C.",
            )?;
            let textarea = document
                .create_element("textarea")?
                .dyn_into::<web_sys::HtmlTextAreaElement>()?;
            textarea.set_attribute("readonly", "")?;
            textarea.set_attribute("aria-label", "Text to copy")?;
            textarea.set_value(&text);
            append(&dialog, &title)?;
            append(&dialog, &message)?;
            append(&dialog, &textarea)?;
            append_close_form(&document, &dialog)?;
            dialog.show_modal()?;
            textarea.select();
            Ok::<(), JsValue>(())
        })();
        if result.is_err() {
            app.flash("Clipboard access was denied; no selectable fallback is available.");
        } else {
            app.set_status("Clipboard access was denied. The text is selected; press Ctrl/Cmd+C.");
        }
    }

    fn close_dialog(&mut self) {
        if let Some(dialog) = &self.dialog {
            if dialog.open() {
                dialog.close();
            }
        }
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog.as_ref().is_some_and(HtmlDialogElement::open)
    }

    fn prepare_dialog(&mut self) -> Result<(Document, HtmlDialogElement), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("window is unavailable"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("document is unavailable"))?;
        if self.dialog.is_none() {
            let dialog = document
                .create_element("dialog")?
                .dyn_into::<HtmlDialogElement>()?;
            dialog.set_attribute("data-televim-demo-dialog", "")?;
            let dialog_url = self.dialog_url.clone();
            let onclose = Closure::wrap(Box::new(move |_event: Event| {
                if let Some(url) = dialog_url.borrow_mut().take() {
                    let _ = Url::revoke_object_url(&url);
                }
                crate::focus_demo();
            }) as Box<dyn FnMut(Event)>);
            dialog.add_event_listener_with_callback("close", onclose.as_ref().unchecked_ref())?;
            document
                .body()
                .ok_or_else(|| JsValue::from_str("document body is unavailable"))?
                .append_child(&dialog)?;
            self.close_listener = Some(onclose);
            self.dialog = Some(dialog);
        }
        let dialog = self
            .dialog
            .as_ref()
            .expect("dialog was just created")
            .clone();
        Ok((document, dialog))
    }
}

impl Drop for BrowserBridge {
    fn drop(&mut self) {
        self.picker.clear();
        self.close_dialog();
        if let Some(dialog) = &self.dialog {
            let _ = dialog.remove();
        }
        if let Some(url) = self.dialog_url.borrow_mut().take() {
            let _ = Url::revoke_object_url(&url);
        }
    }
}

pub(crate) fn element(
    document: &Document,
    tag: &str,
    text: &str,
) -> Result<web_sys::Element, JsValue> {
    let element = document.create_element(tag)?;
    element.set_text_content(Some(text));
    Ok(element)
}

pub(crate) fn append(
    parent: &impl AsRef<web_sys::Node>,
    child: &impl AsRef<web_sys::Node>,
) -> Result<(), JsValue> {
    parent.as_ref().append_child(child.as_ref()).map(|_| ())
}

pub(crate) fn append_close_form(
    document: &Document,
    dialog: &HtmlDialogElement,
) -> Result<(), JsValue> {
    let form = document.create_element("form")?;
    form.set_attribute("method", "dialog")?;
    let close = element(document, "button", "Close")?;
    close.set_attribute("type", "submit")?;
    append(&form, &close)?;
    append(dialog, &form)
}
