use crate::Desktop;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = CutCraft)]
extern "C" {
    fn state_version() -> f64;
    fn state_update() -> String;
    fn picture_version() -> f64;
    fn picture() -> String;
    fn on_change(callback: JsValue);
    fn dispatch(intent: &str);
    fn inspect(controls: &str);
}
struct Browser {
    desktop: Desktop,
    revision: f64,
    picture_revision: f64,
}
impl eframe::App for Browser {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let revision = state_version();
        if revision != self.revision
            && let Ok(mut remote) = serde_json::from_str::<crate::Remote>(&state_update())
        {
            let image_revision = picture_version();
            remote.picture = if image_revision != self.picture_revision {
                picture()
            } else {
                std::mem::take(&mut self.desktop.remote.picture)
            };
            self.desktop.remote = remote;
            self.revision = revision;
            self.picture_revision = image_revision;
        }
        self.desktop.frame(ui);
        for intent in std::mem::take(&mut self.desktop.intents) {
            if let Ok(value) = serde_json::to_string(&intent) {
                dispatch(&value);
            }
        }
        if let Ok(value) = serde_json::to_string(&self.desktop.controls) {
            inspect(&value);
        }
    }
}
#[wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|creation| {
                let context = creation.egui_ctx.clone();
                let callback =
                    Closure::wrap(Box::new(move || context.request_repaint()) as Box<dyn FnMut()>);
                on_change(callback.into_js_value());
                Ok(Box::new(Browser {
                    desktop: Desktop::default(),
                    revision: -1.,
                    picture_revision: -1.,
                }))
            }),
        )
        .await
}
