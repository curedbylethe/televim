use std::{cell::RefCell, rc::Rc};

use ratzilla::{
    DomBackend, WebGl2Backend, WebRenderer,
    backend::webgl2::{FontAtlasConfig, WebGl2BackendOptions},
    ratatui::{Terminal, backend::Backend},
};
use tui::App;

use crate::input::to_key;

enum Renderer {
    Dom,
    WebGl2,
}

pub fn run(app: Rc<RefCell<App>>) -> Result<(), Box<dyn std::error::Error>> {
    let renderer = if web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| query.contains("renderer=dom"))
    {
        Renderer::Dom
    } else {
        Renderer::WebGl2
    };

    match renderer {
        Renderer::Dom => run_backend(DomBackend::new()?, app),
        Renderer::WebGl2 => {
            let options = WebGl2BackendOptions::new()
                .font_atlas_config(FontAtlasConfig::dynamic(&["JetBrains Mono"], 16.0));
            run_backend(WebGl2Backend::new_with_options(options)?, app)
        }
    }
}

fn run_backend<B>(backend: B, app: Rc<RefCell<App>>) -> Result<(), Box<dyn std::error::Error>>
where
    B: Backend + ratzilla::WebEventHandler + 'static,
{
    let mut terminal = Terminal::new(backend)?;
    terminal.on_key_event({
        let app = Rc::clone(&app);
        move |key| {
            if let Some(key) = to_key(key) {
                app.borrow_mut().handle_key(key);
            }
        }
    })?;
    terminal.draw_web(move |frame| app.borrow().render(frame));
    Ok(())
}
