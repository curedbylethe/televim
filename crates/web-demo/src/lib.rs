mod backend;
mod browser;
mod clipboard;
mod driver;
mod file_picker;
mod input;
mod media_viewer;

use std::{cell::RefCell, rc::Rc};

use driver::Driver;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};

thread_local! {
    static DRIVER: RefCell<Option<Rc<RefCell<Driver>>>> = const { RefCell::new(None) };
    static SHELL_MESSAGES: RefCell<Option<Closure<dyn FnMut(web_sys::MessageEvent)>>> = const { RefCell::new(None) };
}

pub fn run() -> std::io::Result<()> {
    console_error_panic_hook::set_once();
    let driver = Rc::new(RefCell::new(Driver::new()));
    DRIVER.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&driver)));
    install_shell_messages()?;
    if let Err(error) = backend::run(driver) {
        notify("failed", Some(&error.to_string()));
        return Err(std::io::Error::other(error.to_string()));
    }
    Ok(())
}

#[cfg(all(target_arch = "wasm32", not(test)))]
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    run().map_err(|error| JsValue::from_str(&error.to_string()))
}

pub fn notify_quit() {
    notify("quit-demo", None);
}

pub fn notify_release() {
    notify("release", None);
}

pub fn notify_ready(renderer: &str) {
    post_event("ready", Some(("renderer", renderer)));
}

pub fn notify_renderer_fallback() {
    notify("renderer-fallback", None);
}

fn notify(kind: &str, message: Option<&str>) {
    post_event(kind, message.map(|message| ("message", message)));
}

fn post_event(kind: &str, field: Option<(&str, &str)>) {
    let value = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&value, &JsValue::from_str("type"), &JsValue::from_str(kind));
    if let Some((name, content)) = field {
        let _ = js_sys::Reflect::set(
            &value,
            &JsValue::from_str(name),
            &JsValue::from_str(content),
        );
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(parent)) = window.parent() else {
        return;
    };
    let Ok(origin) = window.location().origin() else {
        return;
    };
    let _ = parent.post_message(&value.into(), &origin);
}

fn install_shell_messages() -> std::io::Result<()> {
    let window =
        web_sys::window().ok_or_else(|| std::io::Error::other("browser window is unavailable"))?;
    let origin = window
        .location()
        .origin()
        .map_err(|_| std::io::Error::other("browser origin is unavailable"))?;
    let callback =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin {
                return;
            }
            let data = event.data();
            let Ok(kind) = js_sys::Reflect::get(&data, &JsValue::from_str("type")) else {
                return;
            };
            match kind.as_string().as_deref() {
                Some("key") => {
                    let Ok(code) = js_sys::Reflect::get(&data, &JsValue::from_str("code")) else {
                        return;
                    };
                    let Ok(modifiers) =
                        js_sys::Reflect::get(&data, &JsValue::from_str("modifiers"))
                    else {
                        return;
                    };
                    if let (Some(code), Some(modifiers)) = (code.as_string(), modifiers.as_f64()) {
                        send_key(&code, modifiers as u8);
                    }
                }
                Some("reset") => reset_demo(),
                Some("focus") => focus_demo(),
                _ => {}
            }
        });
    window
        .add_event_listener_with_callback("message", callback.as_ref().unchecked_ref())
        .map_err(|_| std::io::Error::other("failed to install the shell message bridge"))?;
    SHELL_MESSAGES.with(|slot| *slot.borrow_mut() = Some(callback));
    Ok(())
}

#[wasm_bindgen]
pub fn reset_demo() {
    DRIVER.with(|slot| {
        if let Some(driver) = slot.borrow().as_ref() {
            driver.borrow_mut().reset();
        }
    });
}

#[wasm_bindgen]
pub fn focus_demo() {
    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector("#demo canvas, #demo_ratzilla_grid")
                .ok()
                .flatten()
                .or_else(|| document.get_element_by_id("demo"))
        })
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let _ = element.focus();
}

#[wasm_bindgen]
pub fn send_key(code: &str, modifiers: u8) {
    let Some(key) = input::from_touch_key(code, modifiers) else {
        return;
    };
    DRIVER.with(|slot| {
        if let Some(driver) = slot.borrow().as_ref() {
            let mut driver = driver.borrow_mut();
            if driver.browser.dialog_open() {
                return;
            }
            driver.tick();
            driver.handle_key(key);
            driver.tick();
            if driver.take_exit() == driver::DriverExit::Quit {
                notify_quit();
            }
        }
    });
}

#[cfg(test)]
#[path = "../tests/wasm.rs"]
mod wasm_tests;
