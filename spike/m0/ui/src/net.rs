//! The browser's side of the protocol: `fetch` for requests, a WebSocket for snapshots.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
    /// Milliseconds from sending to the whole body arriving.
    pub ms: f64,
}

pub fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

fn js(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "network error".into())
}

/// A same-origin request with the site's own header (spec §13: CSRF), the session cookie included.
pub async fn request(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    extra: &[(&str, &str)],
) -> Result<Answer, String> {
    let started = now_ms();
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let headers = web_sys::Headers::new().map_err(js)?;
    headers.set("x-cutcraft", "1").map_err(js)?;
    if let Some(kind) = content_type {
        headers.set("content-type", kind).map_err(js)?;
    }
    for (k, v) in extra {
        headers.set(k, v).map_err(js)?;
    }
    init.set_headers(&headers);
    if let Some(bytes) = body {
        let array = js_sys::Uint8Array::from(bytes);
        init.set_body(&array);
    }
    let request = web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?;
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(js)?
        .dyn_into()
        .map_err(js)?;
    let buffer = JsFuture::from(response.array_buffer().map_err(js)?)
        .await
        .map_err(js)?;
    Ok(Answer {
        status: response.status(),
        body: js_sys::Uint8Array::new(&buffer).to_vec(),
        ms: now_ms() - started,
    })
}

/// Follows `/api/live`, calling `on_text` with each snapshot. Reconnects after a second when the
/// connection drops (spec §13: the page then asks for the current snapshot, which the server
/// sends first on every connection).
pub fn follow(on_text: std::rc::Rc<dyn Fn(String)>, on_state: std::rc::Rc<dyn Fn(bool)>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let scheme = if location.protocol().as_deref() == Ok("https:") {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().unwrap_or_default();
    let Ok(socket) = web_sys::WebSocket::new(&format!("{scheme}://{host}/api/live")) else {
        return;
    };
    let text = on_text.clone();
    let onmessage =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if let Some(s) = e.data().as_string() {
                text(s);
            }
        });
    socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
    let opened = on_state.clone();
    let onopen = Closure::<dyn FnMut()>::new(move || opened(true));
    socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();
    let onclose = Closure::<dyn FnMut()>::new(move || {
        on_state(false);
        let (text, state) = (on_text.clone(), on_state.clone());
        let again = Closure::<dyn FnMut()>::once(move || follow(text, state));
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                again.as_ref().unchecked_ref(),
                1000,
            );
        }
        again.forget();
    });
    socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();
}

/// A new idempotency key (spec §7).
pub fn new_key() -> String {
    let a = (js_sys::Math::random() * 1e15) as u64;
    let b = (js_sys::Math::random() * 1e15) as u64;
    format!("{a:x}{b:x}")
}

/// Asks the browser for a design file with the page's hidden file input, then hands its name
/// and bytes to `done`.
pub fn pick_file(done: std::rc::Rc<dyn Fn(String, Vec<u8>)>) {
    let Some(input) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("m0_file"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    let chosen = input.clone();
    let onchange = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = chosen.files().and_then(|f| f.get(0)) else {
            return;
        };
        let name = file.name();
        let done = done.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(buffer) = JsFuture::from(file.array_buffer()).await {
                done(name, js_sys::Uint8Array::new(&buffer).to_vec());
            }
        });
    });
    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
    onchange.forget();
    input.set_value("");
    input.click();
}

/// Mirrors the slice's state into the page, for the browser checks to read (the canvas itself
/// can't be inspected).
pub fn report(json: &str) {
    if let Some(e) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("m0_state"))
    {
        e.set_text_content(Some(json));
    }
}
