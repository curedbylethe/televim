use std::{cell::RefCell, rc::Rc};

use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::Clipboard;

use crate::browser::ResultEvent;

pub(crate) fn write(results: Rc<RefCell<Vec<ResultEvent>>>, text: String) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("window is unavailable"))?;
    let clipboard = Reflect::get(window.navigator().as_ref(), &JsValue::from_str("clipboard"))?
        .dyn_into::<Clipboard>()?;
    let promise = clipboard.write_text(&text);
    spawn_local(async move {
        let result = JsFuture::from(promise).await;
        results.borrow_mut().push(if result.is_ok() {
            ResultEvent::ClipboardCopied
        } else {
            ResultEvent::ClipboardDenied(text)
        });
    });
    Ok(())
}
