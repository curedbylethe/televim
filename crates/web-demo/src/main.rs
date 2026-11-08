mod backend;
mod browser;
mod clipboard;
mod driver;
mod file_picker;
mod input;
mod media_viewer;

use std::{cell::RefCell, rc::Rc};

use driver::Driver;
use wasm_bindgen::prelude::wasm_bindgen;

thread_local! {
    static DRIVER: RefCell<Option<Rc<RefCell<Driver>>>> = const { RefCell::new(None) };
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    console_error_panic_hook::set_once();
    let driver = Rc::new(RefCell::new(Driver::new()));
    DRIVER.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&driver)));
    backend::run(driver)
}

pub fn notify_quit() {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Ok(event) = web_sys::Event::new("quit-demo") {
        let _ = window.dispatch_event(&event);
    }
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
pub fn focus_demo() {}

#[wasm_bindgen]
pub fn release_demo() {}
