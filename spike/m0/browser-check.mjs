// Milestone 0's browser check (spec §19): the slice in headless Chromium, over HTTPS under the
// real CSP and serving headers. The canvas can't be inspected, so the page mirrors its state into
// #m0_state, and the check aims real mouse and key events using the view positions it reports.
//
// Usage: node browser-check.mjs PAGE_URL DEBUG_PORT FIXTURE.svg PASSWORD

import fs from "node:fs/promises";

const [pageUrl, debugPort, fixture, password] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tab = await (await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`, { method: "PUT" })).json();
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
let next = 0;
const waiting = new Map();
ws.addEventListener("message", (e) => {
  const message = JSON.parse(e.data);
  if (!message.id) return;
  const { resolve, reject } = waiting.get(message.id);
  waiting.delete(message.id);
  message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
});
function cdp(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++next;
    waiting.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}
async function page(expression) {
  const r = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result.value;
}
const state = () => page(`JSON.parse(document.getElementById("m0_state").textContent || "{}")`);
async function until(what, test, ms = 15000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    last = await state();
    if (test(last)) return last;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}: ${JSON.stringify(last)}`);
}

let failures = 0;
function check(name, ok, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${ok ? "" : `: ${detail}`}`);
  if (!ok) failures += 1;
}
const measured = {};

// Where a mat millimetre point is on screen, from the view the page reports.
const onScreen = (v, p) => [
  (v.rect[0] + v.rect[2]) / 2 + (p[0] - v.center[0]) * v.zoom,
  (v.rect[1] + v.rect[3]) / 2 + (p[1] - v.center[1]) * v.zoom,
];
async function mouse(type, x, y, extra = {}) {
  await cdp("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1, ...extra });
}
async function drag(from, to) {
  await mouse("mousePressed", from[0], from[1]);
  for (let i = 1; i <= 10; i++) {
    await mouse("mouseMoved", from[0] + ((to[0] - from[0]) * i) / 10, from[1] + ((to[1] - from[1]) * i) / 10);
    await sleep(16);
  }
  await mouse("mouseReleased", to[0], to[1]);
}
async function key(k) {
  const code = `Key${k.toUpperCase()}`;
  await cdp("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, text: k });
  await cdp("Input.dispatchKeyEvent", { type: "keyUp", key: k, code });
}

await cdp("Page.enable");
await cdp("Page.addScriptToEvaluateOnNewDocument", {
  source: `window.__csp = []; window.__errors = [];
    document.addEventListener("securitypolicyviolation", (e) => window.__csp.push(e.violatedDirective + " " + e.blockedURI));
    window.addEventListener("error", (e) => window.__errors.push(String(e.message)));`,
});
// The window is sized by Chromium's own flags (browser-check.sh), as a real browser is: device
// emulation confuses eframe's canvas sizing.

// 1. The shell loads, under the CSP, and the wasm starts.
await cdp("Page.navigate", { url: pageUrl });
const first = await until("the wasm to start", (s) => s.signed_in === false, 30000);
measured.first_load_to_running_ms = Math.round(first.ready_at_ms);
const transfer = await page(`performance.getEntriesByType("resource").filter(e => /\\.wasm$/.test(e.name))
  .map(e => ({ transfer: e.transferSize, encoded: e.encodedBodySize, decoded: e.decodedBodySize }))[0]`);
measured.wasm_bytes = transfer;
check("the wasm arrives compressed", transfer && transfer.encoded < transfer.decoded, JSON.stringify(transfer));
measured.webgpu_in_browser = await page(`!!navigator.gpu`);

// 2. Sign in (the server's login; the form itself is egui's), then the page follows live.
const login = await page(`fetch("/api/login", { method: "POST", headers: { "content-type": "application/json" },
  body: JSON.stringify({ password: ${JSON.stringify(password)} }) }).then(r => r.status)`);
check("sign in", login === 200, `HTTP ${login}`);
await cdp("Page.reload");
const cached = await until("signed in and live", (s) => s.signed_in === true && s.live === true, 30000);
measured.cached_load_to_running_ms = Math.round(cached.ready_at_ms);

// 3. Upload the authored fixture, as a drop or the file picker does.
const svg = await fs.readFile(fixture, "utf8");
const uploaded = await page(`fetch("/api/upload?name=star-and-ring.svg", { method: "POST",
  headers: { "x-cutcraft": "1" }, body: ${JSON.stringify(svg)} }).then(r => r.status)`);
check("upload", uploaded === 200, `HTTP ${uploaded}`);
let s = await until("the design, cut-ready, with its geometry", (s) => s.design && s.cut_ready && s.geometry && s.mat_view);
check("Make cut-ready ran in the worker", s.cut_ready === true);
measured.geometry_ms = s.geometry_ms && Math.round(s.geometry_ms);
measured.geometry_bytes = s.geometry_bytes;

// 4. Drag the design on the mat and drop it: one sheet.place, the plan follows. Aim only once the
// layout has settled (the status line's height changes as messages arrive).
let previous = "";
s = await until("a steady view", (t) => {
  const now = JSON.stringify(t.mat_view);
  const steady = now === previous;
  previous = now;
  return steady && t.mat_view;
});
measured.window = await page(`[innerWidth, innerHeight, devicePixelRatio,
  document.getElementById("m0_canvas").clientWidth, document.getElementById("m0_canvas").clientHeight]`);
const box = s.art_box_mm;
const aimedView = s.mat_view;
const centre = [(box[0] + box[2]) / 2, (box[1] + box[3]) / 2];
const from = onScreen(s.mat_view, centre);
const to = onScreen(s.mat_view, [centre[0] + 40, centre[1] + 30]);
const geometryBefore = s.geometry;
const dropAt = Date.now();
await drag(from, to);
s = await until("the new plan after the drop", (s) => s.geometry !== geometryBefore);
measured.drop_to_new_plan_ms_seen_by_check = Date.now() - dropAt;
s = await until("the page's own drop timing", (t) => t.drop_to_plan_ms);
measured.drop_to_snapshot_ms = Math.round(s.drop_to_snapshot_ms);
measured.drop_to_plan_ms = Math.round(s.drop_to_plan_ms);
const moved = [s.art_box_mm[0] - box[0], s.art_box_mm[1] - box[1]];
const sent = s.last_drop.moved_mm;
const pointer = [(s.last_drop.release[0] - s.drag_start.press[0]) / s.drag_start.zoom,
  (s.last_drop.release[1] - s.drag_start.press[1]) / s.drag_start.zoom];
const detail = JSON.stringify({ moved, sent, pointer, aimed: aimedView, start: s.drag_start, drop: s.last_drop });
check("the drop sends the pointer's movement on the mat", Math.abs(sent[0] - pointer[0]) < 0.05 && Math.abs(sent[1] - pointer[1]) < 0.05, detail);
check("the server moved the design by exactly that", Math.abs(moved[0] - sent[0]) < 0.02 && Math.abs(moved[1] - sent[1]) < 0.02, detail);
check("the pointer was where the check aimed (about 40 × 30 mm)", Math.abs(sent[0] - 40) < 3 && Math.abs(sent[1] - 30) < 3, detail);
check("egui sees the whole window", s.drag_start.screen[0] === measured.window[0] && s.drag_start.screen[1] === measured.window[1], JSON.stringify([s.drag_start.screen, measured.window]));
check("one sheet.place applied", s.log.some((l) => l.startsWith("sheet.place applied")), JSON.stringify(s.log));

// 5. Change the material (M): a registry command with the session's revision.
// Focus the canvas for keys: a click in the grey around the mat, which does nothing else.
const corner = [s.mat_view.rect[0] + 4, s.mat_view.rect[1] + 4];
await mouse("mousePressed", corner[0], corner[1]); await mouse("mouseReleased", corner[0], corner[1]);
const material = s.material;
await key("m");
s = await until("another material", (s) => s.material && s.material !== material);
check("the material changed", s.material !== material);
measured.command_ms = s.command_ms && Math.round(s.command_ms);

// 6. A lost reply (L): the server makes the change and drops the answer; the page asks by key.
const beforeLost = s.art_box_mm;
await key("l");
s = await until("the lookup by key", (s) => s.lost_reply);
check("a lost reply is found by its key", /applied at revision/.test(s.lost_reply), s.lost_reply);
s = await until("the 5 mm move", (s) => s.art_box_mm && Math.abs(s.art_box_mm[0] - beforeLost[0] - 5) < 0.5);
check("the change it made is there, once", Math.abs(s.art_box_mm[0] - beforeLost[0] - 5) < 0.5, JSON.stringify(s.art_box_mm));

// 7. The overlay (O): a server picture under the locally drawn lines; zooming asks for another.
await key("o");
s = await until("an artwork picture", (s) => s.screen === "overlay" && s.picture);
measured.picture_ms = s.picture_ms && Math.round(s.picture_ms);
const firstPicture = s.picture;
const v = s.overlay_view;
const mid = [(v.rect[0] + v.rect[2]) / 2, (v.rect[1] + v.rect[3]) / 2];
await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: mid[0], y: mid[1] });
await cdp("Input.dispatchMouseEvent", { type: "mouseWheel", x: mid[0], y: mid[1], deltaX: 0, deltaY: -400 });
s = await until("a sharper picture after zooming", (s) => s.picture && s.picture !== firstPicture);
check("zooming asks for a new picture", s.picture !== firstPicture);
check("the picture region shrank with the zoom", (() => {
  const a = new URLSearchParams(firstPicture), b = new URLSearchParams(s.picture);
  return Number(b.get("x1")) - Number(b.get("x0")) < Number(a.get("x1")) - Number(a.get("x0"));
})(), `${firstPicture} → ${s.picture}`);

// 8. Nothing broke the rules.
const csp = await page("window.__csp");
check("no CSP violations", csp.length === 0, JSON.stringify(csp));
const errors = await page("window.__errors");
check("no page errors", errors.length === 0, JSON.stringify(errors));

console.log("measured: " + JSON.stringify(measured));
ws.close();
if (failures) {
  console.log(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("all browser checks passed");
