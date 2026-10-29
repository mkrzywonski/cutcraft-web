use crate::Desktop;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = CutCraft)]
extern "C" {
    fn state() -> String;
    fn dispatch(intent: &str);
    fn inspect(controls: &str);
}
impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Ok(remote) = serde_json::from_str(&state()) {
            self.remote = remote;
        }
        self.frame(ui);
        for intent in std::mem::take(&mut self.intents) {
            if let Ok(value) = serde_json::to_string(&intent) {
                dispatch(&value);
            }
        }
        if let Ok(value) = serde_json::to_string(&self.controls) {
            inspect(&value);
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}
#[wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|_| Ok(Box::<Desktop>::default())),
        )
        .await
}
