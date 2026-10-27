// Browser checks for the web interface: what a person's taps and the page's live updates do
// together, which the Rust tests can't see. Run through scripts/browser-check.sh, which starts a
// scratch server (practice cutter only) and headless Chromium with a debugging port.
//
// Usage: node scripts/browser-check.mjs PAGE_URL DEBUG_PORT FIXTURE_DIR PLANNER

import fs from "node:fs/promises";

const [pageUrl, debugPort, fixtures, planner] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// --- A minimal Chrome DevTools Protocol client ---------------------------------------------------

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

let failures = 0;
function check(name, ok, detail) {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${ok ? "" : `: ${detail}`}`);
  if (!ok) failures += 1;
}

// --- Setup: a phone-sized page, the authored square queued on the practice cutter ---------------

await cdp("Page.enable");
await cdp("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
await cdp("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 1 });
await cdp("Page.navigate", { url: `${pageUrl}/?nolive` });
await sleep(1000);

const project = await fs.readFile(`${fixtures}/square.cutcraft`, "utf8");
const plan = await fs.readFile(`${fixtures}/square-plan.json`, "utf8");
await page(`(async () => {
  const f = new FormData();
  f.append("name", "Browser check square");
  f.append("planner", ${JSON.stringify(planner)});
  f.append("key", "browser-check-upload");
  f.append("project", new Blob([${JSON.stringify(project)}]), "square.cutcraft");
  f.append("plan", new Blob([${JSON.stringify(plan)}]), "plan.json");
  const r = await fetch("/api/v1/jobs", { method: "POST", headers: { "x-csrf-token": cookie("cpi_csrf") }, body: f });
  window.checkJob = await r.json();
  await api("POST", "/api/v1/cutters/practice/queue", { job_id: checkJob.id, revision: 1, key: "browser-check-queue" });
  await refreshState();
})()`);

// Count the cutter controls' requests instead of sending them.
await page(`(() => {
  window.sent = { stop: 0, pause: 0, resume: 0 };
  const real = window.fetch;
  window.fetch = (path, options) => {
    const action = String(path).split("/").pop();
    if (action in sent) {
      sent[action] += 1;
      return Promise.resolve(new Response('{"ok":true}', { status: 200, headers: { "Content-Type": "application/json" } }));
    }
    return real(path, options);
  };
})()`);

/** Shows the practice cutter mid-cut, as a live update would describe it. */
async function showCutting(activity) {
  await page(`(() => {
    location.hash = "#/cutter/practice";
    const c = state.cutters.find((x) => x.id === "practice");
    Object.assign(c.view, {
      online: true, activity: ${JSON.stringify(activity)}, kind: "main", execution: "check",
      cutting: c.view.turn.job,
      progress: { fraction: 0.2, remaining_s: 30, passes_sent: 0, passes_total: 2, phase: "Cutting" },
    });
    render();
  })()`);
  await sleep(200);
}

async function centre(label) {
  return page(`(() => {
    const b = [...document.querySelectorAll("button")].find((x) => x.textContent === ${JSON.stringify(label)});
    if (!b) return null;
    b.scrollIntoView({ block: "center" });
    const r = b.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
  })()`);
}

const liveUpdate = `(() => {
  const c = state.cutters.find((x) => x.id === "practice");
  c.view.progress = { ...c.view.progress, fraction: c.view.progress.fraction + 0.05 };
  window.redrawn = (window.redrawn || 0) + 1;
  render(true);
})()`;

async function mousePress(label, between) {
  const at = await centre(label);
  await cdp("Input.dispatchMouseEvent", { type: "mousePressed", ...at, button: "left", clickCount: 1 });
  if (between) await page(between);
  await sleep(80);
  await cdp("Input.dispatchMouseEvent", { type: "mouseReleased", ...at, button: "left", clickCount: 1 });
  await sleep(120);
}

async function tap(label, between) {
  const at = await centre(label);
  await cdp("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  if (between) await page(between);
  await sleep(80);
  await cdp("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await sleep(200);
}

const count = (action) => page(`sent.${action}`);

// --- Stop, Pause and Resume survive a live update between press and release -------------------

await showCutting("cutting");
await mousePress("Stop", liveUpdate);
check("a Stop click with a live update during the press sends one Stop", (await count("stop")) === 1, `${await count("stop")} sent`);
await tap("Stop", liveUpdate);
check("a Stop tap with a live update during the press sends one Stop", (await count("stop")) === 2, `${await count("stop")} sent`);
await mousePress("Stop");
check("a Stop click without a live update sends one Stop", (await count("stop")) === 3, `${await count("stop")} sent`);
await tap("Pause", liveUpdate);
check("a Pause tap with a live update during the press sends one Pause", (await count("pause")) === 1, `${await count("pause")} sent`);
await showCutting("paused");
await tap("Resume", liveUpdate);
check("a Resume tap with a live update during the press sends one Resume", (await count("resume")) === 1, `${await count("resume")} sent`);

// The update that waited during the press is drawn once the press ends.
await showCutting("cutting");
await page("window.drawnPercent = () => document.querySelector('.progress').getAttribute('aria-valuenow')");
const before = await page("drawnPercent()");
await mousePress("Stop", liveUpdate);
const after = await page("drawnPercent()");
check("a live update held during a press is drawn after it", after !== before, `still ${after}%`);

// --- A loaded fresh mat keeps the blade and placement instructions ------------------------------

await page(`(() => {
  const c = state.cutters.find((x) => x.id === "practice");
  Object.assign(c.view, { activity: "idle", kind: null, execution: null, cutting: null, progress: null,
    mat: { state: "fresh", generation: 1 }, mat_loaded: true });
  render();
})()`);
await sleep(500);
const ready = await page("document.getElementById('view').innerText");
check("a fresh loaded mat still says how to set the blade", ready.includes("Set the blade to 1."), ready);
check("a fresh loaded mat says where the material goes", ready.includes("top-left corner"), ready);
check("a fresh loaded mat doesn't ask to load it again", !ready.includes("press Load Mat"), ready);

// --- The practice mat: Load only when empty -------------------------------------------------------

const practiceButtons = () => page(`(() => {
  const find = (t) => [...document.querySelectorAll("button")].find((b) => b.textContent === t);
  return { load: find("Load mat").disabled, unload: find("Unload mat").disabled };
})()`);
await page(`(() => {
  const c = state.cutters.find((x) => x.id === "practice");
  Object.assign(c.view, { mat: { state: "used", generation: 1 }, mat_loaded: true });
  render();
})()`);
await sleep(300);
let buttons = await practiceButtons();
check("a used mat can't be loaded again, only unloaded", buttons.load && !buttons.unload, JSON.stringify(buttons));
await page(`(() => {
  const c = state.cutters.find((x) => x.id === "practice");
  Object.assign(c.view, { mat: { state: "none" }, mat_loaded: false });
  render();
})()`);
await sleep(300);
buttons = await practiceButtons();
check("an empty practice cutter can be loaded, not unloaded", !buttons.load && buttons.unload, JSON.stringify(buttons));

// --- A mat loaded unseen can be confirmed fresh; a used one can't --------------------------------

const confirmButton = (state_) => page(`(() => {
  location.hash = "#/cutter/practice";
  const c = state.cutters.find((x) => x.id === "practice");
  Object.assign(c.view, { activity: "idle", kind: null, execution: null, cutting: null, progress: null,
    mat: ${JSON.stringify(state_)}, mat_loaded: true });
  render();
  const text = document.getElementById("view").innerText;
  return { button: [...document.querySelectorAll("button")].some((b) => b.textContent === "It has fresh material"), text };
})()`);
let seen = await confirmButton({ state: "unseen" });
check("an unseen mat can be confirmed fresh", seen.button, seen.text);
check("an unseen mat says it may have cut material", seen.text.includes("may contain previously cut material"), seen.text);
seen = await confirmButton({ state: "used", generation: 1 });
check("a used mat can't be confirmed fresh", !seen.button, seen.text);

// --- Adjust: a new width, saved as the next version ----------------------------------------------

await page(`location.hash = "#/adjust/" + checkJob.id`);
await sleep(1500);
const pieceHidden = await page(`(() => {
  const w = document.querySelector('[aria-label="Piece width in inches"]');
  return w.closest(".row").hidden;
})()`);
check("Adjust hides the piece's size until there is a piece", pieceHidden === true, String(pieceHidden));
await page(`(() => {
  const width = document.querySelector('[aria-label="Width in inches"]');
  width.value = "1";
  width.dispatchEvent(new Event("change"));
})()`);
await sleep(1200);
await page(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Save").click()`);
await sleep(1500);
const adjusted = await page(`api("GET", "/api/v1/jobs/" + checkJob.id).then((j) => ({ revision: j.details.current_revision, width: j.settings.design_size_mm[0], from: j.revision.adjusted_from }))`);
check("Adjust saves the new width as the next version", adjusted.revision === 2 && Math.abs(adjusted.width - 25.4) < 0.05 && adjusted.from === 1, JSON.stringify(adjusted));

// --- Library: the folder header and the Move picker ----------------------------------------------

const button = (text) => `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)})`;
await page(`(async () => {
  await api("POST", "/api/v1/library/folders", { parent: "", name: "Shirts" });
  await api("POST", "/api/v1/library/folders", { parent: "Shirts", name: "Summer" });
  const f = new FormData();
  f.append("name", "Browser check folders");
  f.append("planner", ${JSON.stringify(planner)});
  f.append("key", "browser-check-folders");
  f.append("project", new Blob([${JSON.stringify(project)}]), "square.cutcraft");
  f.append("plan", new Blob([${JSON.stringify(plan)}]), "plan.json");
  const r = await fetch("/api/v1/jobs", { method: "POST", headers: { "x-csrf-token": cookie("cpi_csrf") }, body: f });
  window.folderJob = await r.json();
})()`);
await page(`location.hash = "#/library/Shirts/Summer"`);
await sleep(1200);
const header = await page(`(() => {
  const h = document.querySelector(".folder-header");
  return { title: h.querySelector(".title").textContent, above: h.querySelector(".folder-title .muted").textContent,
    back: !!h.querySelector('[aria-label="Back"]'), crumbs: !!document.querySelector("#view .crumbs") };
})()`);
check("a folder's header shows where you are, with a back arrow and no breadcrumb links",
  header.title.startsWith("Summer") && header.above === "Library / Shirts" && header.back && !header.crumbs, JSON.stringify(header));
await page(`document.querySelector(".folder-title").click()`);
await sleep(300);
await page(`[...document.querySelectorAll("dialog.sheet .folder-row")].find((b) => b.textContent.includes("Library")).click()`);
await sleep(800);
check("the folder title jumps to a folder above", await page("location.hash") === "#/library", await page("location.hash"));

await page(`location.hash = "#/job/" + folderJob.id`);
await sleep(1500);
await page(`document.querySelector('#view [aria-label="Actions"]').click()`);
await sleep(300);
await page(`${button("Move to another folder")}.click()`);
await sleep(800);
const start = await page(`({ text: document.querySelector(".picker-footer").innerText, disabled: ${button("Move here")}.disabled })`);
check("Move opens a folder picker where the job's own folder isn't a choice", start.disabled && start.text.includes("already in Library"), JSON.stringify(start));
for (const name of ["Shirts", "Summer"]) {
  await page(`[...document.querySelectorAll("dialog.sheet .folder-row")].find((b) => b.textContent.includes(${JSON.stringify(name)})).click()`);
  await sleep(600);
}
await page(`${button("Move here")}.click()`);
await sleep(1500);
const moved = await page(`api("GET", "/api/v1/jobs/" + folderJob.id).then((j) => j.folder.join("/"))`);
check("browsing into a folder and Move here moves the job", moved === "Shirts/Summer", moved);
const jobHeader = await page(`(() => {
  const h = document.querySelector("#view .folder-header");
  return { title: h.querySelector(".title").textContent, above: h.querySelector(".folder-title .muted").textContent,
    back: !!h.querySelector('[aria-label="Back"]'), crumbs: !!document.querySelector("#view .crumbs") };
})()`);
check("the job page has the folder header: its name, its folders above, a back arrow",
  jobHeader.title.startsWith("Browser check folders") && jobHeader.above === "Library / Shirts / Summer" && jobHeader.back && !jobHeader.crumbs,
  JSON.stringify(jobHeader));
await page(`document.querySelector('#view [aria-label="Back"]').click()`);
await sleep(1000);
check("the job page's back arrow returns to its folder", (await page("location.hash")) === "#/library/Shirts/Summer", await page("location.hash"));
await page(`location.hash = "#/job/" + folderJob.id`);
await sleep(1500);
await page(`document.querySelector('#view [aria-label="Actions"]').click()`);
await sleep(300);
await page(`${button("Move to another folder")}.click()`);
await sleep(800);
await page(`${button("＋ New folder here")}.click()`);
await page(`(() => { const i = document.querySelector('dialog.sheet input[aria-label="New folder name"]'); i.value = "Autumn"; i.form.requestSubmit(); })()`);
await sleep(1000);
await page(`${button("Move here")}.click()`);
await sleep(1500);
const intoNew = await page(`api("GET", "/api/v1/jobs/" + folderJob.id).then((j) => j.folder.join("/"))`);
check("the picker makes a new folder and moves the job into it", intoNew === "Shirts/Summer/Autumn", intoNew);

// Moving a folder: from the ⋯ menu, through the same picker, never into itself.
await page(`location.hash = "#/library/Shirts/Summer"`);
await sleep(1200);
await page(`document.querySelector('#view [aria-label="Folder actions"]').click()`);
await sleep(300);
await page(`${button("Move this folder")}.click()`);
await sleep(800);
const offered = await page(`[...document.querySelectorAll("dialog.sheet .folder-row")].map((b) => b.textContent)`);
check("a folder's own name isn't offered as a place to move it", !offered.some((t) => t.includes("Summer")) && offered.some((t) => t.includes("Winter") || t.includes("New folder")), JSON.stringify(offered));
await page(`document.querySelector('dialog.sheet [aria-label="Back"]').click()`);
await sleep(600);
await page(`${button("Move here")}.click()`);
await sleep(1500);
const folderMoved = await page(`api("GET", "/api/v1/jobs/" + folderJob.id).then((j) => j.folder.join("/"))`);
check("Move this folder moves it and its jobs", folderMoved === "Summer/Autumn" && (await page("location.hash")) === "#/library/Summer", `${folderMoved} ${await page("location.hash")}`);

// Renaming a folder from its ⋯ menu.
await page(`document.querySelector('#view [aria-label="Folder actions"]').click()`);
await sleep(300);
await page(`${button("Rename this folder")}.click()`);
await sleep(300);
await page(`(() => { const i = document.querySelector('dialog.sheet input[aria-label="Folder name"]'); i.value = "Holidays"; i.form.requestSubmit(); })()`);
await sleep(1500);
const renamed = await page(`api("GET", "/api/v1/jobs/" + folderJob.id).then((j) => j.folder.join("/"))`);
check("Rename this folder renames it, its jobs with it", renamed === "Holidays/Autumn" && (await page("location.hash")) === "#/library/Holidays", `${renamed} ${await page("location.hash")}`);

// Recently deleted: a fixed top-level folder showing deleted jobs, each restorable.
await page(`location.hash = "#/job/" + folderJob.id`);
await sleep(1500);
await page(`window.confirm = () => true`);
await page(`document.querySelector('#view [aria-label="Actions"]').click()`);
await sleep(300);
await page(`[...document.querySelectorAll("dialog.sheet button")].find((b) => b.textContent === "Delete").click()`);
await sleep(1500);
await page(`location.hash = "#/library"`);
await sleep(1500);
const row = await page(`(() => { const r = document.querySelector("#view li.recently-deleted"); return r ? r.innerText : null; })()`);
check("the library's top level has Recently deleted", row !== null && row.includes("Recently deleted") && row.includes("1 item"), String(row));
await page(`document.querySelector("#view li.recently-deleted a").click()`);
await sleep(1500);
const binPage = await page(`({ text: document.querySelector("#view").innerText, menu: !!document.querySelector('#view [aria-label="Folder actions"]') })`);
check("Recently deleted lists the job, where it was and when it goes for good, with no ⋯ menu",
  binPage.text.includes("Browser check folders") && binPage.text.includes("From Holidays / Autumn") && /Will be permanently deleted on \S+/.test(binPage.text) && !binPage.menu,
  JSON.stringify(binPage));
await page(`[...document.querySelectorAll("#view .list a")][0].click()`);
await sleep(400);
await page(`[...document.querySelectorAll("dialog.sheet button")].find((b) => b.textContent.startsWith("Restore to")).click()`);
await sleep(1500);
const restored = await page(`api("GET", "/api/v1/jobs/" + folderJob.id).then((j) => j.folder.join("/"))`);
const opened = await page(`location.hash === "#/job/" + folderJob.id`);
check("Restore puts the job back in its folder and opens it", restored === "Holidays/Autumn" && opened, `${restored} ${opened}`);
await page(`api("DELETE", "/api/v1/jobs/" + folderJob.id)`);
await page(`location.hash = "#/deleted"`);
await sleep(1200);
await page(`[...document.querySelectorAll("#view .list a")][0].click()`);
await sleep(400);
await page(`[...document.querySelectorAll("dialog.sheet button")].find((b) => b.textContent === "Delete permanently").click()`);
await sleep(1200);
check("Delete permanently empties Recently deleted", (await page(`api("GET", "/api/v1/deleted").then((d) => d.length)`)) === 0, "still listed");

// Search and sort: the whole library by name and notes, results showing their folder.
await page(`location.hash = "#/library"`);
await sleep(1200);
await page(`(() => { const i = document.querySelector('#view input[type=search]'); i.value = "check sq"; i.dispatchEvent(new Event("input")); })()`);
await sleep(1200);
const searched = await page(`document.querySelector("#view").innerText`);
check("search finds a job anywhere in the library and shows its folder", searched.includes("Browser check square") && searched.includes("📁 Library"), searched.slice(0, 300));
await page(`(() => { const i = document.querySelector('#view input[type=search]'); i.value = ""; i.dispatchEvent(new Event("input")); })()`);
await sleep(800);
await page(`(() => { const s = document.querySelector('#view select[aria-label="Sort jobs by"]'); s.value = "new"; s.dispatchEvent(new Event("change")); })()`);
await sleep(1200);
check("the sort choice is kept", (await page(`localStorage.getItem("cpi_library_sort")`)) === "new" && (await page(`document.querySelector('#view select[aria-label="Sort jobs by"]').value`)) === "new", "not kept");

// --- Settings: collapsible sections, one open at a time ------------------------------------------

await page(`location.hash = "#/settings"`);
await sleep(1500);
const folds = await page(`(async () => {
  const all = [...document.querySelectorAll("details.fold")];
  const open = () => all.filter((d) => d.open).map((d) => d.dataset.key);
  const titles = all.map((d) => d.querySelector(".fold-title").textContent);
  all[0].open = true;
  await new Promise((r) => setTimeout(r, 100));
  const first = open();
  all[2].open = true;
  await new Promise((r) => setTimeout(r, 100));
  return { count: all.length, titles, first, second: open(), key: all[2].dataset.key };
})()`);
check("Settings shows its sections folded with titles", folds.count >= 4 && folds.titles.includes("Library backup"), JSON.stringify(folds));
check("opening a Settings section closes the others", folds.second.length === 1 && folds.second[0] === folds.key, JSON.stringify(folds));

// --- Backup restore: encryption comes from the target, and failed Replace stays an error ---------

await page(`(async () => {
  window.backupTest = { api, prompt, attempts: [], prompts: 0, failed: true };
  const b = await api("GET", "/api/v1/backup");
  api = async (method, path, body) => {
    if (path === "/api/v1/backup" && method === "GET") return { ...b, health: "never", encrypted: false };
    if (path === "/api/v1/backup/restore") {
      backupTest.attempts.push(body.password);
      if (body.password !== "the backup password") throw new ApiError(409, { error: "password", message: "This backup is encrypted" });
      return { jobs: 2, from: "Backup Pi", updated: 42, replace_allowed: true };
    }
    if (path === "/api/v1/backup/restore/apply") {
      if (backupTest.failed) throw new ApiError(409, { error: "busy", message: "A cutter is in use" });
      return { restored: true, jobs: 2 };
    }
    return backupTest.api(method, path, body);
  };
  prompt = () => { backupTest.prompts++; return "the backup password"; };
  await renderSettings();
})()`);
await sleep(500);
await page(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Restore from the backup…").click()`);
await sleep(500);
const password = await page(`({ attempts: backupTest.attempts, prompts: backupTest.prompts, replace: [...document.querySelectorAll("button")].some((b) => b.textContent === "Replace the library") })`);
check("Restore on a fresh Pi asks for the target backup's password", password.prompts === 1 && password.attempts[0] === null && password.attempts[1] === "the backup password" && password.replace, JSON.stringify(password));
await page(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Replace the library").click()`);
await sleep(500);
const failedRestore = await page(`document.getElementById("view").textContent`);
check("a refused Replace shows its error without claiming the library was restored", failedRestore.includes("A cutter is in use") && !failedRestore.includes("The library is restored."), failedRestore.slice(-1000));
await page(`backupTest.failed = false; [...document.querySelectorAll("button")].find((b) => b.textContent === "Restore from the backup…").click()`);
await sleep(300);
await page(`[...document.querySelectorAll("button")].find((b) => b.textContent === "Replace the library").click()`);
await sleep(500);
check("a confirmed Replace reports the restored library", await page(`document.getElementById("view").textContent.includes("The library is restored.")`), "missing success message");
await page(`api = backupTest.api; prompt = backupTest.prompt; delete window.backupTest;`);

// Update waiting is visible in every view, without replacing this version's assets.
await page(`state.update = {waiting: true, ready: false}; render()`);
check("update waiting is visible", await page(`!document.getElementById("update-status").hidden && document.getElementById("update-status").textContent.includes("Current cuts can finish")`), "missing update notice");
await page(`state.update = null; render()`);
check("update notice clears", await page(`document.getElementById("update-status").hidden`), "stale update notice");

// --- The login screen hides the tabs -------------------------------------------------------------

await page("showLogin()");
const tabs = await page("getComputedStyle(document.getElementById('tabs')).display");
check("the login screen hides the tabs", tabs === "none", `display is ${tabs}`);

ws.close();
console.log(failures ? `${failures} browser check(s) failed` : "all browser checks passed");
process.exit(failures ? 1 : 0);
