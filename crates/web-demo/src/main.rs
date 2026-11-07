mod backend;
mod driver;
mod input;

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
