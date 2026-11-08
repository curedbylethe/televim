use std::{cell::RefCell, rc::Rc};

use crate::driver::Driver;
use gloo_timers::callback::Interval;
use ratzilla::{
    DomBackend, WebGl2Backend, WebRenderer,
    backend::webgl2::{FontAtlasConfig, WebGl2BackendOptions},
    ratatui::{Terminal, backend::Backend},
};

use crate::input::to_key;

enum Renderer {
    Dom,
    WebGl2,
}

thread_local! {
    static TICKER: RefCell<Option<Interval>> = const { RefCell::new(None) };
}

pub fn run(driver: Rc<RefCell<Driver>>) -> Result<(), Box<dyn std::error::Error>> {
    let renderer = if web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| query.contains("renderer=dom"))
    {
        Renderer::Dom
    } else {
        Renderer::WebGl2
    };

    match renderer {
        Renderer::Dom => run_backend(DomBackend::new()?, driver),
        Renderer::WebGl2 => {
            let options = WebGl2BackendOptions::new()
                .font_atlas_config(FontAtlasConfig::dynamic(&["JetBrains Mono"], 16.0));
            run_backend(WebGl2Backend::new_with_options(options)?, driver)
        }
    }
}

fn run_backend<B>(backend: B, driver: Rc<RefCell<Driver>>) -> Result<(), Box<dyn std::error::Error>>
where
    B: Backend + ratzilla::WebEventHandler + 'static,
{
    let mut terminal = Terminal::new(backend)?;
    terminal.on_key_event({
        let driver = Rc::clone(&driver);
        move |key| {
            if let Some(key) = to_key(key) {
                if driver.borrow().browser.dialog_open() {
                    return;
                }
                let mut driver = driver.borrow_mut();
                driver.tick();
                driver.handle_key(key);
                driver.tick();
            }
        }
    })?;
    let render_driver = Rc::clone(&driver);
    terminal.draw_web(move |frame| render_driver.borrow().render(frame));
    let tick_driver = Rc::clone(&driver);
    let ticker = Interval::new(100, move || tick_driver.borrow_mut().tick());
    TICKER.with(|slot| *slot.borrow_mut() = Some(ticker));
    Ok(())
}
