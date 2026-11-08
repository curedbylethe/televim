use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::driver::{Driver, DriverExit};
use gloo_timers::callback::Interval;
use ratzilla::{
    DomBackend, WebGl2Backend, WebRenderer,
    backend::webgl2::{FontAtlasConfig, WebGl2BackendOptions},
    ratatui::{Terminal, backend::Backend},
};
use wasm_bindgen::{JsCast, closure::Closure};

use crate::input::to_key;

enum Renderer {
    Dom,
    WebGl2,
}

thread_local! {
    static TICKER: RefCell<Option<Interval>> = const { RefCell::new(None) };
    static CTRL_W_GUARD: RefCell<Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>> = const { RefCell::new(None) };
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
        Renderer::Dom => run_backend(DomBackend::new_by_id("demo")?, driver, "dom"),
        Renderer::WebGl2 => {
            let options = WebGl2BackendOptions::new()
                .grid_id("demo")
                .font_atlas_config(FontAtlasConfig::dynamic(&["JetBrains Mono"], 16.0));
            match WebGl2Backend::new_with_options(options) {
                Ok(backend) => run_backend(backend, driver, "webgl2"),
                Err(_) => {
                    crate::notify_renderer_fallback();
                    run_backend(DomBackend::new_by_id("demo")?, driver, "dom")
                }
            }
        }
    }
}

fn run_backend<B>(
    backend: B,
    driver: Rc<RefCell<Driver>>,
    renderer: &'static str,
) -> Result<(), Box<dyn std::error::Error>>
where
    B: Backend + ratzilla::WebEventHandler + 'static,
{
    install_ctrl_w_guard()?;
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
                if driver.take_exit() == DriverExit::Quit {
                    crate::notify_quit();
                }
            }
        }
    })?;
    let render_driver = Rc::clone(&driver);
    let ready = Cell::new(false);
    terminal.draw_web(move |frame| {
        render_driver.borrow().render(frame);
        if !ready.replace(true) {
            crate::focus_demo();
            crate::notify_ready(renderer);
        }
    });
    let tick_driver = Rc::clone(&driver);
    let ticker = Interval::new(100, move || tick_driver.borrow_mut().tick());
    TICKER.with(|slot| *slot.borrow_mut() = Some(ticker));
    Ok(())
}

fn install_ctrl_w_guard() -> Result<(), Box<dyn std::error::Error>> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| std::io::Error::other("browser document is unavailable"))?;
    let mount = document
        .get_element_by_id("demo")
        .ok_or_else(|| std::io::Error::other("demo terminal mount is unavailable"))?;
    let guard =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                return;
            };
            let terminal_focused = document.active_element().is_some_and(|active| {
                active.tag_name() == "CANVAS"
                    || active.id() == "demo_ratzilla_grid"
                    || active.id() == "demo"
            });
            if terminal_focused && event.ctrl_key() && event.alt_key() && event.key() == "Escape" {
                event.prevent_default();
                event.stop_propagation();
                crate::notify_release();
            } else if event.ctrl_key()
                && (terminal_focused
                    || document
                        .query_selector("dialog[data-televim-demo-dialog][open]")
                        .ok()
                        .flatten()
                        .is_some())
                && (event.key().eq_ignore_ascii_case("w") || event.key().eq_ignore_ascii_case("c"))
            {
                event.prevent_default();
            }
        });
    mount
        .add_event_listener_with_callback_and_bool("keydown", guard.as_ref().unchecked_ref(), true)
        .map_err(|_| std::io::Error::other("failed to install the terminal Ctrl-W guard"))?;
    CTRL_W_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
    Ok(())
}
