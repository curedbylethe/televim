use gloo_timers::callback::Timeout;
use js_sys::{Array, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, BlobPropertyBag, Document, HtmlAnchorElement, HtmlDialogElement, Url};

use demo_support::DemoMedia;

use crate::browser::{append, append_close_form, element};

pub(crate) fn populate(
    document: &Document,
    dialog: &HtmlDialogElement,
    media: &DemoMedia,
) -> Result<Option<String>, JsValue> {
    let title = element(document, "h2", "Demo media · fictional")?;
    title.set_attribute("id", "televim-demo-media-title")?;
    let metadata = element(
        document,
        "p",
        &format!(
            "{} · {} · {}",
            media.kind.label(),
            media.filename,
            media.caption
        ),
    )?;
    dialog.set_attribute("aria-labelledby", "televim-demo-media-title")?;
    dialog.set_attribute("data-televim-demo-media", "")?;
    append(dialog, &title)?;
    append(dialog, &metadata)?;

    let display = media.mime.starts_with("image/")
        || media.mime.starts_with("audio/")
        || media.mime.starts_with("video/");
    let url = if display || media.downloadable {
        Some(object_url(media)?)
    } else {
        None
    };

    if media.mime.starts_with("image/") {
        let image = document.create_element("img")?;
        image.set_attribute("src", url.as_deref().expect("image has an object URL"))?;
        image.set_attribute("alt", &format!("{}: {}", media.filename, media.caption))?;
        append(dialog, &image)?;
    } else if media.mime.starts_with("audio/") || media.mime.starts_with("video/") {
        let tag = if media.mime.starts_with("audio/") {
            "audio"
        } else {
            "video"
        };
        let player = document.create_element(tag)?;
        player.set_attribute("src", url.as_deref().expect("player has an object URL"))?;
        player.set_attribute("controls", "")?;
        player.set_attribute("aria-label", &media.filename)?;
        append(dialog, &player)?;
    } else if media.mime == "text/plain" {
        let text = String::from_utf8_lossy(&media.bytes);
        let pre = element(document, "pre", &text)?;
        pre.set_attribute("data-televim-demo-media-text", "")?;
        append(dialog, &pre)?;
    } else {
        append(
            dialog,
            &element(
                document,
                "p",
                "This media type cannot be previewed in the browser.",
            )?,
        )?;
    }

    if media.downloadable {
        if let Some(url) = &url {
            let link = document
                .create_element("a")?
                .dyn_into::<HtmlAnchorElement>()?;
            link.set_href(url);
            link.set_download(&media.filename);
            link.set_text_content(Some("Download"));
            link.set_attribute("data-televim-demo-download", "")?;
            append(dialog, &link)?;
        }
    }
    append_close_form(document, dialog)?;
    Ok(url)
}

pub(crate) fn download(media: &DemoMedia) -> Result<(), JsValue> {
    if !media.downloadable {
        return Err(JsValue::from_str("this fixture cannot be downloaded"));
    }
    let url = object_url(media)?;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("document is unavailable"))?;
    let anchor = document
        .create_element("a")?
        .dyn_into::<HtmlAnchorElement>()?;
    anchor.set_href(&url);
    anchor.set_download(&media.filename);
    document
        .body()
        .ok_or_else(|| JsValue::from_str("document body is unavailable"))?
        .append_child(&anchor)?;
    anchor.click();
    anchor.remove();
    Timeout::new(0, move || {
        let _ = Url::revoke_object_url(&url);
    })
    .forget();
    Ok(())
}

fn object_url(media: &DemoMedia) -> Result<String, JsValue> {
    let parts = Array::new();
    parts.push(&Uint8Array::from(media.bytes.as_slice()));
    let options = BlobPropertyBag::new();
    options.set_type(media.mime);
    let blob = Blob::new_with_u8_array_sequence_and_options(parts.as_ref(), &options)?;
    Url::create_object_url_with_blob(&blob)
}
