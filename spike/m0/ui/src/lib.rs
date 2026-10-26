//! Milestone 0's desktop-layout slice in the browser (spec §19). `view` holds the arithmetic and is
//! tested natively; the rest runs only in the browser.

pub mod view;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod net;

#[cfg(target_arch = "wasm32")]
mod start {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    /// Runs when the page loads the module: the app on `<canvas id="m0_canvas">`.
    #[wasm_bindgen(start)]
    pub fn start() {
        wasm_bindgen_futures::spawn_local(async {
            let Some(canvas) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("m0_canvas"))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            else {
                return;
            };
            let started = eframe::WebRunner::new()
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|cc| Ok(Box::new(crate::app::App::new(cc)))),
                )
                .await;
            if let Some(loading) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("m0_loading"))
            {
                loading.set_text_content(Some(match started {
                    Ok(()) => "",
                    Err(_) => "This browser couldn't start the drawing surface (WebGPU or WebGL2).",
                }));
            }
        });
    }
}
