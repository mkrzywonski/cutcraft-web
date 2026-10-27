// cameo-pi's web interface (spec §12): mobile first, plain JavaScript, nothing fetched from
// anywhere but this Pi. Every piece of data is put in the page as text, never as HTML.
"use strict";

// --- Small helpers ------------------------------------------------------------------------------

function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

/** Replaces children, skipping null and false (replaceChildren would show them as "null"). */
function swap(el, ...children) {
  el.replaceChildren(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
  return el;
}

/** Appends children, skipping null and false (the DOM's own append would show them as text). */
function put(el, ...children) {
  el.append(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
  return el;
}

function cookie(name) {
  const found = document.cookie.split(";").map((c) => c.trim()).find((c) => c.startsWith(name + "="));
  return found ? decodeURIComponent(found.slice(name.length + 1)) : null;
}

/** A fresh idempotency key for one action (spec §9.1). */
function newKey() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

class ApiError extends Error {
  constructor(status, body) {
    super((body && body.message) || `Request failed (${status})`);
    this.status = status;
    this.code = body && body.error;
    this.detail = body && body.detail;
  }
}

async function api(method, path, body) {
  const headers = { Accept: "application/json" };
  const csrf = cookie("cpi_csrf");
  if (csrf) headers["X-CSRF-Token"] = csrf;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try {
    data = await response.json();
  } catch (_) {
    data = null;
  }
  if (!response.ok) {
    if (response.status === 401 && data && data.error === "login") showLogin();
    throw new ApiError(response.status, data);
  }
  return data;
}

/**
 * Runs an action once, keeping its idempotency key until it succeeds, so a retry after a dropped
 * connection can't do it twice.
 */
const pendingKeys = new Map();
async function once(label, fn) {
  let key = pendingKeys.get(label);
  if (!key) {
    key = newKey();
    pendingKeys.set(label, key);
  }
  const result = await fn(key);
  pendingKeys.delete(label);
  return result;
}

/** A duration in plain words: "9 s", "3 min", "1 h 5 min". */
function minutes(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`;
}

function when(unix) {
  if (!unix) return "never";
  return new Date(unix * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

function inches(mm) {
  return `${(mm / 25.4).toFixed(1)} in`;
}

function showError(where, error) {
  where.replaceChildren(h("p", { class: "error", role: "alert" }, error.message || String(error)));
}

// --- Live state (spec §9.3) ---------------------------------------------------------------------

const state = { boot: null, seq: 0, cutters: [], version: "", me: null };
const jobs = new Map(); // job id -> job details, cached for names
let source = null;

function setLive(on) {
  document.getElementById("connection").classList.toggle("live", on);
}

async function refreshState() {
  const snapshot = await api("GET", "/api/v1/state");
  applySnapshot(snapshot);
}

function applySnapshot(snapshot) {
  state.boot = snapshot.boot;
  state.seq = snapshot.seq;
  state.cutters = snapshot.cutters;
  state.version = snapshot.version || state.version;
  render();
}

function connect() {
  if (source) source.close();
  source = new EventSource("/api/v1/events");
  source.addEventListener("open", () => setLive(true));
  source.addEventListener("error", () => setLive(false));
  source.addEventListener("snapshot", (e) => applySnapshot(JSON.parse(e.data)));
  source.addEventListener("change", (e) => {
    const change = JSON.parse(e.data);
    // A gap or a restart: start again from a full snapshot.
    if (change.boot !== state.boot || change.seq !== state.seq + 1) {
      refreshState().catch(() => {});
      return;
    }
    state.seq = change.seq;
    const i = state.cutters.findIndex((c) => c.id === change.cutter.id);
    if (i >= 0) state.cutters[i] = change.cutter;
    else state.cutters.push(change.cutter);
    render(true);
  });
}

async function job(id, fresh) {
  if (!fresh && jobs.has(id)) return jobs.get(id);
  const data = await api("GET", `/api/v1/jobs/${encodeURIComponent(id)}`);
  jobs.set(id, data);
  return data;
}

const revisions = new Map(); // "job/revision" -> that revision's facts and steps; revisions never change
async function jobRevision(id, revision) {
  const key = `${id}/${revision}`;
  if (!revisions.has(key)) {
    revisions.set(key, await api("GET", `/api/v1/jobs/${encodeURIComponent(id)}?revision=${revision}`));
  }
  return revisions.get(key);
}

// --- Routing -----------------------------------------------------------------------------------

const view = () => document.getElementById("view");
let currentRoute = "";

function route() {
  return (location.hash || "#/cutters").slice(2).split("/").map(decodeURIComponent);
}

function setTab(name) {
  for (const a of document.querySelectorAll(".tabs a")) {
    a.classList.toggle("active", a.dataset.tab === name);
  }
}

// While a finger or button is pressed, live redraws wait: replacing the control under it between
// press and release would lose the click, and with it a Stop.
let pressing = 0;
let redrawWaiting = false;
function pressStart() {
  pressing += 1;
}
function pressEnd() {
  // Released after the click this release produces has been handled.
  setTimeout(() => {
    pressing = Math.max(0, pressing - 1);
    if (!pressing && redrawWaiting) {
      redrawWaiting = false;
      render(true);
    }
  }, 0);
}
const activationKey = (e) => e.key === "Enter" || e.key === " ";
document.addEventListener("pointerdown", pressStart, true);
document.addEventListener("pointerup", pressEnd, true);
document.addEventListener("pointercancel", pressEnd, true);
document.addEventListener("keydown", (e) => { if (activationKey(e) && !e.repeat) pressStart(); }, true);
document.addEventListener("keyup", (e) => { if (activationKey(e)) pressEnd(); }, true);
window.addEventListener("blur", () => { pressing = 0; });

/** Re-renders the current page. Live updates only redraw pages that show cutters. */
function render(live) {
  const [page, ...rest] = route();
  if (!state.me || state.me.locked || state.me.needsLogin || state.me.mustChange) return;
  if (live && !["cutters", "cutter"].includes(page)) return;
  if (live && pressing) {
    redrawWaiting = true;
    return;
  }
  const key = location.hash;
  const fresh = key !== currentRoute;
  currentRoute = key;
  setTab(page === "cutter" ? "cutters" : ["job", "adjust", "deleted"].includes(page) ? "library" : page);
  if (page === "cutter") renderCutter(rest[0], fresh);
  else if (page === "library") renderLibrary(rest.filter(Boolean));
  else if (page === "job") renderJob(rest[0]);
  else if (page === "deleted") renderDeleted();
  else if (page === "adjust") renderAdjust(rest[0]);
  else if (page === "settings") renderSettings();
  else renderCutters();
}

window.addEventListener("hashchange", () => render());
let resizing = null;
window.addEventListener("resize", () => {
  clearTimeout(resizing);
  resizing = setTimeout(() => render(true), 200);
});

// --- Who is using this (spec §14.1) -------------------------------------------------------------

async function loadMe() {
  const me = await api("GET", "/api/v1/me");
  state.me = {
    open: me.mode === "open",
    locked: me.mode === "locked",
    needsLogin: me.mode === "users" && !me.user,
    mustChange: !!me.must_change,
    user: me.user || null,
    admin: me.role === "admin",
  };
  return state.me;
}

function showBanner() {
  const banner = document.getElementById("banner");
  const notes = [];
  if (state.me && state.me.open) {
    notes.push(h("p", { class: "note" },
      "Anyone on your network can use cameo-pi. ",
      h("a", { href: "#/settings" }, "Add a user"), " to require a login."));
  }
  // The designs must survive the SD card (spec §15): say so until they're backed up.
  const b = state.backup;
  if (b && ["off", "failed", "warning"].includes(b.health)) {
    const openBackup = () => { try { localStorage.setItem(OPEN_SECTION, "backup"); } catch (_) { /* nothing */ } };
    notes.push(h("p", { class: "note danger" },
      b.health === "off" ? "⚠ Your designs are only on this Pi's SD card. " : b.health === "warning" ? `⚠ ${b.maintenance_warning} ` : `⚠ Backups have been failing since ${when(b.failing_since)}. `,
      h("a", { href: "#/settings", onclick: openBackup }, b.health === "off" ? "Set up a backup" : "Check the backup"), "."));
  }
  banner.hidden = !notes.length;
  banner.replaceChildren(...notes);
}

function showLogin() {
  document.getElementById("tabs").hidden = true;
  if (source) source.close();
  const error = h("div");
  const name = h("input", { id: "name", autocomplete: "username", autocapitalize: "none", required: true });
  const password = h("input", { id: "password", type: "password", autocomplete: "current-password", required: true });
  view().replaceChildren(
    h("form", {
      class: "card",
      onsubmit: async (e) => {
        e.preventDefault();
        try {
          await api("POST", "/api/v1/login", { name: name.value.trim(), password: password.value });
          await start();
        } catch (err) {
          showError(error, err);
        }
      },
    },
      h("h2", {}, "Log in to cameo-pi"),
      h("label", { for: "name" }, "User name"), name,
      h("label", { for: "password" }, "Password"), password,
      error,
      h("p", {}, h("button", { class: "primary big", type: "submit" }, "Log in")),
      h("p", { class: "muted small" }, "Forgot the admin password? Run sudo cameo-pi reset-admin on the Pi."),
    ),
  );
}

function showChangePassword(forced) {
  const error = h("div");
  const old = h("input", { id: "old", type: "password", autocomplete: "current-password" });
  const fresh = h("input", { id: "new", type: "password", autocomplete: "new-password", minlength: 8 });
  return h("form", {
    class: "card",
    onsubmit: async (e) => {
      e.preventDefault();
      try {
        await api("POST", "/api/v1/me/password", { old: old.value, new: fresh.value });
        await start();
      } catch (err) {
        showError(error, err);
      }
    },
  },
    h("h2", {}, forced ? "Choose your own password" : "Change your password"),
    forced ? h("p", { class: "muted" }, "An admin set a temporary password. Choose a new one to continue.") : null,
    h("label", { for: "old" }, forced ? "Temporary password" : "Current password"), old,
    h("label", { for: "new" }, "New password (at least 8 characters)"), fresh,
    error,
    h("p", {}, h("button", { class: "primary", type: "submit" }, "Save password")),
  );
}

// --- Cutters (home) ----------------------------------------------------------------------------

/** What a person needs to know about a cutter, in plain words. */
function describe(c) {
  const v = c.view;
  if (!v.online) return ["Off or unplugged", "danger"];
  switch (v.activity) {
    case "check-cutter": return ["Check the cutter: unload the mat", "danger"];
    case "testing": return ["Test cutting", "ok"];
    case "cutting": return ["Cutting", "ok"];
    case "paused": return ["Paused", "warn"];
    case "stopping": return ["Stopping…", "warn"];
    case "confirming-stop": return ["Stopped, checking the cutter…", "warn"];
    default: break;
  }
  const mat = v.mat.state;
  if (mat === "none") return [v.turn ? "Load a fresh mat" : "No mat", v.turn ? "warn" : ""];
  if (mat === "unseen") return ["Currently loaded mat may contain previously cut material: unload it and load it again with fresh material", "warn"];
  if (mat === "used") return ["Mat used: load fresh material", "warn"];
  if (mat === "fresh" && !v.mat_loaded) return ["Checking the mat…", ""];
  if (v.mat_confirmed_by) return [`Fresh mat loaded (confirmed by ${v.mat_confirmed_by.user || v.mat_confirmed_by.client})`, "ok"];
  return ["Fresh mat loaded", "ok"];
}

function running(v) {
  return ["testing", "cutting", "paused", "stopping"].includes(v.activity);
}

function jobName(id) {
  const j = jobs.get(id);
  if (!j) {
    job(id).then(() => render(true)).catch(() => {});
    return "…";
  }
  return j.details.name;
}

function progressBlock(v) {
  const p = v.progress;
  if (!p) return h("p", { class: "muted" }, "Starting…");
  const percent = Math.round(Math.min(1, Math.max(0, p.fraction)) * 100);
  const bar = h("div", { class: "progress", role: "progressbar", "aria-valuenow": percent, "aria-valuemin": 0, "aria-valuemax": 100 }, h("div"));
  bar.firstChild.style.width = `${percent}%`;
  return h("div", {},
    bar,
    h("p", { class: "muted small" },
      `${percent}% · about ${minutes(p.remaining_s)} left · cut ${p.passes_sent} of ${p.passes_total}`),
  );
}

function cutterSummary(c) {
  const v = c.view;
  const [text, tone] = describe(c);
  return h("a", { class: "card card-link", href: `#/cutter/${encodeURIComponent(c.id)}` },
    h("h2", {}, v.name, c.simulated ? " (practice)" : ""),
    h("p", { class: `state ${tone}` }, text),
    v.turn ? h("p", {}, running(v) ? "On it: " : "Up next: ", h("strong", {}, jobName(v.turn.job.job_id))) : null,
    running(v) ? progressBlock(v) : null,
    v.queue.length ? h("p", { class: "muted small" }, `${v.queue.length} more in the queue`) : null,
  );
}

function renderCutters() {
  if (!state.cutters.length) {
    view().replaceChildren(h("p", { class: "pad muted" }, "No cutters yet."));
    return;
  }
  view().replaceChildren(...state.cutters.map(cutterSummary));
}

// --- One cutter: loading, starting, watching (spec §8, §12) -------------------------------------

const viewerCache = new Map(); // "job/revision" -> geometry

async function geometry(jobId, revision) {
  const key = `${jobId}/${revision}`;
  if (!viewerCache.has(key)) {
    viewerCache.set(key, await api("GET", `/api/v1/jobs/${encodeURIComponent(jobId)}/revisions/${revision}/geometry`));
  }
  return viewerCache.get(key);
}

/** The live viewer: the mat, the cut lines in order, what is done, and the blade (spec §12). */
function drawViewer(canvas, g, v) {
  const css = getComputedStyle(document.documentElement);
  const [mw, mh] = g.mat_mm;
  // The canvas keeps the mat's proportions and CSS scales it to the page, so a width measured
  // before layout settles only affects sharpness, never the shape.
  const width = Math.max(canvas.clientWidth, 320);
  const scale = width / mw;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(mh * scale * dpr);
  const ctx = canvas.getContext("2d");
  ctx.setTransform(scale * dpr, 0, 0, scale * dpr, 0, 0);
  ctx.clearRect(0, 0, mw, mh);
  ctx.strokeStyle = css.getPropertyValue("--line");
  ctx.lineWidth = 0.6;
  ctx.strokeRect(0, 0, g.cutting_area_mm[0], g.cutting_area_mm[1]);

  const p = v.progress;
  const kind = v.kind;
  const passesSent = p && kind === "main" ? p.passes_sent : 0;
  let pass = 0;
  for (const op of g.operations) {
    const done = pass + op.passes <= passesSent;
    pass += op.passes;
    if (op.points.length < 2) continue;
    ctx.beginPath();
    ctx.moveTo(op.points[0][0], op.points[0][1]);
    for (const [x, y] of op.points.slice(1)) ctx.lineTo(x, y);
    ctx.strokeStyle = done ? css.getPropertyValue("--done") : css.getPropertyValue("--cut");
    ctx.globalAlpha = done ? 1 : 0.55;
    ctx.lineWidth = done ? 1.4 : 0.8;
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
  if (p && p.estimate && kind === "main") {
    const u = g.units_per_mm;
    const ahead = p.estimate.ahead || [];
    if (ahead.length) {
      ctx.beginPath();
      ctx.moveTo(p.estimate.at[0] / u, p.estimate.at[1] / u);
      for (const [x, y] of ahead) ctx.lineTo(x / u, y / u);
      ctx.strokeStyle = css.getPropertyValue("--warn");
      ctx.lineWidth = 1;
      ctx.setLineDash([3, 3]);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    ctx.beginPath();
    ctx.arc(p.estimate.at[0] / u, p.estimate.at[1] / u, 3.5, 0, Math.PI * 2);
    ctx.fillStyle = css.getPropertyValue("--danger");
    ctx.fill();
  }
}

let viewerCanvas = null;

// The running cut's controls keep their elements across redraws, so they are never swapped out
// from under a finger.
const kept = new Map();
function keep(key, make) {
  if (!kept.has(key)) kept.set(key, make());
  return kept.get(key);
}

async function startCut(c, kind, j) {
  const v = c.view;
  const turn = v.turn;
  const body = {
    boot: state.boot,
    kind,
    turn: turn ? turn.id : null,
    job_id: turn.job.job_id,
    revision: turn.job.revision,
    generation: v.mat.generation,
    action_epoch: v.action_epoch,
  };
  // The key is tied to what the person saw: a different screen is a different action.
  const label = `start/${c.id}/${kind}/${turn.id}/${v.action_epoch}/${v.mat.generation}`;
  return once(label, (key) => api("POST", `/api/v1/cutters/${encodeURIComponent(c.id)}/start`, { ...body, key }));
}

function renderCutter(id, fresh) {
  const c = state.cutters.find((x) => x.id === id);
  if (!c) {
    view().replaceChildren(h("p", { class: "pad muted" }, "No such cutter."));
    return;
  }
  const v = c.view;
  const [text, tone] = describe(c);
  const message = h("div");
  const act = (fn) => async () => {
    try {
      await fn();
    } catch (err) {
      showError(message, err);
    }
  };
  const post = (path, body) => api("POST", `/api/v1/cutters/${encodeURIComponent(c.id)}/${path}`, body || {});

  const card = h("section", { class: "card" },
    pageHeader(`${v.name}${c.simulated ? " (practice)" : ""}`, "Cutters", () => { location.hash = "#/cutters"; }),
    h("p", { class: `state ${tone}` }, text),
    // The override for a mat loaded while cameo-pi wasn't watching (spec §8.2).
    v.mat.state === "unseen" && v.mat_loaded && v.activity === "idle" ? h("p", {}, h("button", {
      onclick: act(async () => {
        if (!confirm("Only if the loaded mat has fresh material where the cut goes. cameo-pi didn't see this mat being loaded, so it can't tell.")) return;
        await post("mat-fresh");
      }),
    }, "It has fresh material")) : null,
    message,
  );

  if (running(v)) {
    put(card, 
      h("p", {}, v.kind === "test" ? "Test cut for " : "Cutting ", h("strong", {}, jobName(v.turn.job.job_id))),
      progressBlock(v),
    );
    if (v.kind === "main" && v.cutting) {
      if (!viewerCanvas || fresh) viewerCanvas = h("canvas", { class: "viewer", "aria-label": "The cut, with what's done so far" });
      put(card, viewerCanvas);
      geometry(v.cutting.job_id, v.cutting.revision).then((g) => drawViewer(viewerCanvas, g, v)).catch(() => {});
    }
    const paused = v.activity === "paused";
    const pause = keep(`${c.id}/pause`, () => h("button"));
    pause.textContent = paused ? "Resume" : "Pause";
    pause.onclick = act(() => post(paused ? "resume" : "pause"));
    const stop = keep(`${c.id}/stop`, () => h("button", { class: "danger big" }, "Stop"));
    stop.onclick = act(() => post("stop"));
    put(card,
      h("div", { class: "row" }, v.activity === "stopping" ? null : pause),
      h("p", {}, stop),
    );
  } else if (v.turn) {
    const turn = v.turn;
    const ready = v.online && v.activity === "idle" && v.mat.state === "fresh" && v.mat_loaded;
    const steps = h("ol", { class: "steps" });
    const facts = h("div");
    // Exactly the revision this turn cuts: the steps, the material and Start all name it.
    jobRevision(turn.job.job_id, turn.job.revision).then((j) => {
      const shown = ready ? j.loaded_steps : j.steps;
      steps.replaceChildren(...shown.map((s) => h("li", {}, s)));
      facts.replaceChildren(h("p", { class: "muted small" }, `${j.revision.material} · about ${minutes(j.revision.estimate_s)} to cut`));
    }).catch(() => {});
    put(card, 
      h("h3", {}, "Up next"),
      h("p", {}, h("strong", {}, jobName(turn.job.job_id)), turn.actor && turn.actor.user ? h("span", { class: "muted" }, ` · queued by ${turn.actor.user}`) : null),
      facts,
      h("h3", {}, ready ? "Mat loaded: check, then cut" : "Load the material"),
      steps,
      turn.tests ? h("p", { class: "muted small" }, `${turn.tests} test cut${turn.tests > 1 ? "s" : ""} on this mat`) : null,
      h("div", { class: "row" },
        h("button", { disabled: !ready, onclick: act(async () => { await startCut(c, "test"); }) }, "Test cut"),
        h("button", { class: "primary", disabled: !ready, onclick: act(async () => { await startCut(c, "main"); }) }, "Cut"),
      ),
      h("div", { class: "row spaced" },
        h("button", { onclick: act(() => post("release", { turn: turn.id })) }, "Done with this cutter"),
        v.queue.length ? h("button", { onclick: act(() => post("skip", { turn: turn.id })) }, "Skip for now") : null,
      ),
    );
  } else {
    put(card, h("p", { class: "muted" }, "Nothing queued. Pick a job in the ", h("a", { href: "#/library" }, "Library"), " and choose Cut."));
  }

  if (c.simulated) {
    put(card, 
      h("h3", {}, "Practice cutter's virtual mat"),
      // Like the real Cameo: one mat at a time, so Load needs it empty and Unload needs a mat.
      h("div", { class: "row" },
        h("button", { disabled: v.mat_loaded, onclick: act(() => post("practice", { load: true })) }, "Load mat"),
        h("button", { disabled: !v.mat_loaded, onclick: act(() => post("practice", { load: false })) }, "Unload mat"),
      ),
      v.mat_loaded ? h("p", { class: "muted small" }, "Unload the mat before loading another.") : null,
    );
  }

  if (v.queue.length) {
    put(card, 
      h("h3", {}, "Queue"),
      h("ul", { class: "list" }, v.queue.map((q, i) => h("li", {},
        h("div", { class: "item" },
          h("div", { class: "grow" }, h("div", { class: "name" }, `${i + 1}. ${jobName(q.job.job_id)}`),
            q.actor && q.actor.user ? h("div", { class: "muted small" }, `queued by ${q.actor.user}`) : null),
          h("button", { class: "danger", onclick: act(() => post("remove", { turn: q.turn })) }, "Remove"),
        )))),
    );
  }
  view().replaceChildren(card);
}

// --- Library (spec §7.4) ------------------------------------------------------------------------

const RECENTLY_DELETED = "Recently deleted";
const libraryHref = (path) => "#/library" + (path.length ? "/" + path.map(encodeURIComponent).join("/") : "");

/** A sheet that rises from the bottom (a full-height panel on small phones), closed by its own
 *  buttons, Escape or a tap outside. Returns the dialog; `close()` removes it. */
function sheet(...content) {
  const dialog = h("dialog", { class: "sheet" }, ...content);
  dialog.addEventListener("close", () => dialog.remove());
  dialog.addEventListener("click", (e) => { if (e.target === dialog) dialog.close(); });
  document.body.append(dialog);
  dialog.showModal();
  return dialog;
}

/** The folder header: a big back arrow (up one folder), where you are as the title (tap it to
 *  jump to any folder above), and ⋯ for folder actions. With `item` (a job's name), it's the
 *  header of that job inside the folder `path`: back returns to the folder. */
function folderHeader(path, { back, open, menu, item }) {
  const folders = ["Library", ...path];
  // Every folder above what's shown can be jumped to; what's shown is last, marked "here".
  const above = item ? folders : folders.slice(0, -1);
  const title = item || folders[folders.length - 1];
  const jump = () => {
    const d = sheet(
      h("h2", {}, "Go to"),
      h("ul", { class: "list folders" },
        above.map((name, i) => h("li", {}, h("button", {
          type: "button",
          class: `folder-row depth-${Math.min(i, 6)}`,
          onclick: () => { d.close(); open(path.slice(0, i)); },
        }, h("span", { class: "grow" }, `${i ? "📁" : "🏠"} ${name}`)))),
        h("li", {}, h("button", { type: "button", class: `folder-row depth-${Math.min(above.length, 6)}`, disabled: true },
          h("span", { class: "grow" }, `${item ? "✂️" : "📁"} ${title}`), h("span", { class: "muted small" }, "here")))),
      h("button", { type: "button", onclick: () => d.close() }, "Close"));
  };
  const up = back || (above.length ? () => open(item ? path : path.slice(0, -1)) : null);
  return h("div", { class: "folder-header" },
    up ? h("button", { type: "button", class: "icon", "aria-label": "Back", onclick: up }, "‹") : null,
    h("button", {
      type: "button",
      class: "folder-title grow",
      disabled: !above.length,
      onclick: jump,
      "aria-label": above.length ? `${title}: go to a folder above` : title,
    },
      above.length ? h("span", { class: "muted small" }, above.join(" / ")) : null,
      h("span", { class: "title" }, title, above.length ? h("span", { class: "chevron" }, " ▾") : null)),
    menu ? h("button", { type: "button", class: "icon", "aria-label": item ? "Actions" : "Folder actions", onclick: menu }, "⋯") : null);
}

/** The same header for a page under another page: back arrow, title, and where it belongs. */
function pageHeader(title, under, back) {
  return h("div", { class: "folder-header" },
    h("button", { type: "button", class: "icon", "aria-label": "Back", onclick: back }, "‹"),
    h("div", { class: "folder-title grow" }, h("span", { class: "muted small" }, under), h("span", { class: "title" }, title)));
}

/** Choosing a folder by browsing to it (spec §7.4): the folder you're in is the destination.
 *  `from` is where the item is now, which isn't a destination. */
function pickFolder({ heading, action, start, from, exclude, onPick }) {
  let path = [...start];
  const body = h("div", { class: "picker-body" });
  const footer = h("div", { class: "picker-footer" });
  const message = h("div");
  const d = sheet(h("h2", {}, heading), body, message, footer);
  d.classList.add("tall");
  const same = (p, q = from) => p.length === q.length && p.every((n, i) => n === q[i]);
  const show = async () => {
    let listing;
    try {
      listing = await api("GET", `/api/v1/library?path=${encodeURIComponent(path.join("/"))}`);
    } catch (err) {
      showError(message, err);
      return;
    }
    const newName = h("input", { placeholder: "New folder name", "aria-label": "New folder name" });
    const newForm = h("form", {
      class: "row", hidden: true,
      onsubmit: async (e) => {
        e.preventDefault();
        try {
          const name = newName.value.trim();
          await api("POST", "/api/v1/library/folders", { parent: path.join("/"), name });
          path = [...path, name];
          show();
        } catch (err) {
          showError(message, err);
        }
      },
    }, newName, h("button", { type: "submit", class: "primary" }, "Add"));
    swap(message);
    swap(body,
      folderHeader(path, { open: (p) => { path = p; show(); } }),
      h("ul", { class: "list folders" },
        listing.folders.filter((f) => !(exclude && same([...path, f], exclude))).map((f) => h("li", {}, h("button", {
          type: "button", class: "folder-row", onclick: () => { path = [...path, f]; show(); },
        }, h("span", { class: "grow" }, `📁 ${f}`), h("span", { class: "muted" }, "›")))),
        h("li", {}, h("button", {
          type: "button", class: "folder-row add",
          onclick: (e) => { e.currentTarget.parentElement.hidden = true; newForm.hidden = false; newName.focus(); },
        }, "＋ New folder here"))),
      newForm,
      listing.folders.length ? null : h("p", { class: "muted small" }, "No folders inside this one."));
    const here = path.length ? path.join(" / ") : "Library";
    swap(footer,
      h("p", { class: "small" }, same(path) ? `It's already in ${here}.` : `${action} to: `, same(path) ? null : h("strong", {}, here)),
      h("div", { class: "row" },
        h("button", { type: "button", class: "primary grow", disabled: same(path), onclick: async () => {
          try {
            await onPick(path);
            d.close();
          } catch (err) {
            showError(message, err);
          }
        } }, `${action} here`),
        h("button", { type: "button", onclick: () => d.close() }, "Cancel")));
  };
  show();
}

const SORT_KEY = "cpi_library_sort";
/** Jobs in the chosen order: name, last cut (most recent first) or newest (spec §7.4). */
function sortJobs(list) {
  let by = "name";
  try { by = localStorage.getItem(SORT_KEY) || "name"; } catch (_) { /* the default */ }
  const name = (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  const order = {
    name,
    cut: (a, b) => (b.last_cut || 0) - (a.last_cut || 0) || name(a, b),
    new: (a, b) => (b.created_at || 0) - (a.created_at || 0) || name(a, b),
  }[by] || name;
  return [...list].sort((a, b) => order(a.job || a, b.job || b));
}

/** A job's card in a list; `folder` shows where it is (in search results). */
function jobTile(j, folder) {
  jobs.delete(j.id);
  return h("li", {}, h("a", { href: `#/job/${encodeURIComponent(j.id)}` },
    h("img", { src: `/api/v1/jobs/${encodeURIComponent(j.id)}/revisions/${j.current_revision}/thumb`, alt: "", loading: "lazy" }),
    h("div", { class: "grow" },
      h("div", { class: "name" }, j.name),
      folder ? h("div", { class: "muted small" }, `📁 ${folder.length ? folder.join(" / ") : "Library"}`) : null,
      h("div", { class: "muted small" }, j.times_cut ? `Cut ${j.times_cut}× · last ${when(j.last_cut)}` : "Not cut yet"),
      j.notes ? h("div", { class: "muted small" }, j.notes.slice(0, 80)) : null,
    )));
}

/** What's being searched for, kept while moving between library pages. */
let librarySearch = "";

async function renderLibrary(path) {
  const where = h("div");
  view().replaceChildren(where);
  try {
    const listing = await api("GET", `/api/v1/library?path=${encodeURIComponent(path.join("/"))}`);
    // Recently deleted sits at the top level, after the folders; it isn't a real folder (spec §7.4).
    const deleted = path.length ? null : await api("GET", "/api/v1/deleted").catch(() => null);
    const message = h("div");
    const folderName = h("input", { placeholder: "New folder name", "aria-label": "New folder name" });
    const newFolder = h("form", {
      class: "row", hidden: true,
      onsubmit: async (e) => {
        e.preventDefault();
        try {
          await api("POST", "/api/v1/library/folders", { parent: path.join("/"), name: folderName.value.trim() });
          renderLibrary(path);
        } catch (err) {
          showError(message, err);
        }
      },
    }, folderName, h("button", { type: "submit", class: "primary" }, "Add"),
      h("button", { type: "button", onclick: () => { newFolder.hidden = true; } }, "Cancel"));
    const items = [
      ...listing.folders.map((f) => h("li", {}, h("a", { href: libraryHref([...path, f]) },
        h("div", { class: "grow name" }, `📁 ${f}`), h("span", { class: "muted" }, "›")))),
      ...(deleted ? [h("li", { class: "recently-deleted" }, h("a", { href: "#/deleted" },
        h("div", { class: "grow" }, h("div", { class: "name" }, "🗑 Recently deleted"),
          h("div", { class: "muted small" }, deleted.length ? `${deleted.length} item${deleted.length === 1 ? "" : "s"}, kept 30 days` : "Empty")),
        h("span", { class: "muted" }, "›")))] : []),
      ...sortJobs(listing.jobs).map((j) => jobTile(j)),
    ];
    const menu = () => {
      const d = sheet(
        h("h2", {}, path.length ? path[path.length - 1] : "Library"),
        h("div", { class: "stack" },
          h("button", { type: "button", onclick: () => { d.close(); newFolder.hidden = false; folderName.focus(); } }, "＋ New folder"),
          path.length ? h("button", {
            type: "button",
            onclick: () => {
              d.close();
              const parent = path.slice(0, -1);
              pickFolder({
                heading: `Move “${path[path.length - 1]}”`,
                action: "Move",
                start: parent,
                from: parent,
                exclude: path,
                onPick: async (to) => {
                  const moved = await api("POST", "/api/v1/library/folders/move", { path: path.join("/"), to: to.join("/") });
                  location.hash = libraryHref(moved.path);
                },
              });
            },
          }, "Move this folder") : null,
          path.length ? h("button", {
            type: "button",
            onclick: () => {
              d.close();
              const name = h("input", { value: path[path.length - 1], "aria-label": "Folder name", autocapitalize: "sentences" });
              const why = h("div");
              const r = sheet(
                h("h2", {}, "Rename this folder"),
                h("form", {
                  class: "stack",
                  onsubmit: async (e) => {
                    e.preventDefault();
                    try {
                      const renamed = await api("POST", "/api/v1/library/folders/rename", { path: path.join("/"), name: name.value.trim() });
                      r.close();
                      location.hash = libraryHref(renamed.path);
                    } catch (err) {
                      showError(why, err);
                    }
                  },
                }, name, why,
                  h("button", { type: "submit", class: "primary" }, "Rename"),
                  h("button", { type: "button", onclick: () => r.close() }, "Cancel")));
              name.focus();
              name.select();
            },
          }, "Rename this folder") : null,
          path.length ? h("button", {
            type: "button",
            class: "danger",
            disabled: items.length > 0,
            onclick: async () => {
              d.close();
              try {
                await api("DELETE", "/api/v1/library/folders", { path: path.join("/") });
                location.hash = libraryHref(path.slice(0, -1));
              } catch (err) {
                showError(message, err);
              }
            },
          }, "Delete this folder") : null,
          path.length && items.length ? h("p", { class: "muted small" }, "Only an empty folder can be deleted.") : null,
          h("button", { type: "button", onclick: () => d.close() }, "Close")));
    };
    const folderView = items.length > (deleted ? 1 : 0) ? h("ul", { class: "list tiles" }, items)
      : h("div", {}, h("p", { class: "muted" }, path.length ? "This folder is empty." : "Nothing here yet. Send a job from CutCraft."),
        deleted ? h("ul", { class: "list tiles" }, items) : null);
    // Search covers the whole library, by name and notes; results replace the folder's list.
    const results = h("div");
    const search = h("input", { type: "search", value: librarySearch, placeholder: "Search all jobs", "aria-label": "Search all jobs", enterkeyhint: "search" });
    let sortBy = "name";
    try { sortBy = localStorage.getItem(SORT_KEY) || "name"; } catch (_) { /* the default */ }
    const sort = h("select", { "aria-label": "Sort jobs by" },
      ...[["name", "Name"], ["cut", "Last cut"], ["new", "Newest"]].map(([v, t]) => h("option", { value: v, selected: sortBy === v }, t)));
    let asked = 0;
    const find = async () => {
      librarySearch = search.value.trim();
      folderView.hidden = !!librarySearch;
      if (!librarySearch) return swap(results);
      const mine = ++asked;
      try {
        const found = await api("GET", `/api/v1/library/search?text=${encodeURIComponent(librarySearch)}`);
        if (mine !== asked) return;
        swap(results, found.length
          ? h("ul", { class: "list tiles" }, sortJobs(found).map((f) => jobTile(f.job, f.folder)))
          : h("p", { class: "muted" }, `No job's name or notes contain “${librarySearch}”.`));
      } catch (err) {
        showError(results, err);
      }
    };
    let typing = null;
    search.addEventListener("input", () => { clearTimeout(typing); typing = setTimeout(find, 250); });
    sort.addEventListener("change", () => {
      try { localStorage.setItem(SORT_KEY, sort.value); } catch (_) { /* this visit only */ }
      renderLibrary(path);
    });
    where.replaceChildren(h("section", { class: "card" },
      folderHeader(path, { open: (p) => { location.hash = libraryHref(p); }, menu }),
      h("div", { class: "row library-tools" }, search, sort),
      newFolder,
      message,
      folderView,
      results,
    ));
    if (librarySearch) find();
  } catch (err) {
    showError(where, err);
  }
}

/** When something in Recently deleted goes for good, as a date. */
const goneOn = (unix) => new Date(unix * 1000).toLocaleDateString();

/** Recently deleted (spec §7.4): deleted jobs for 30 days, each restorable or deletable now.
 *  It can't be renamed, moved or deleted itself, so it has no ⋯ menu. */
async function renderDeleted() {
  const where = h("div");
  view().replaceChildren(where);
  try {
    const items = await api("GET", "/api/v1/deleted");
    const message = h("div");
    const open = (item) => {
      const from = item.folder.length ? item.folder.join(" / ") : "Library";
      const d = sheet(
        h("h2", {}, item.name),
        h("p", { class: "muted small" }, `Will be permanently deleted on ${goneOn(item.expires_at)}.`),
        h("div", { class: "stack" },
          item.kind === "job" ? h("button", {
            type: "button",
            class: "primary",
            onclick: async () => {
              try {
                const back = await api("POST", `/api/v1/deleted/${encodeURIComponent(item.entry)}/restore`);
                d.close();
                location.hash = `#/job/${encodeURIComponent(back.id)}`;
              } catch (err) {
                showError(message, err);
                d.close();
              }
            },
          }, `Restore to ${from}`) : h("p", { class: "muted small" }, "The whole library as it was before a restore. It can't be put back from here."),
          h("button", {
            type: "button",
            class: "danger",
            onclick: async () => {
              if (!confirm(`Delete “${item.name}” permanently? This can't be undone.`)) return;
              try {
                await api("DELETE", `/api/v1/deleted/${encodeURIComponent(item.entry)}`);
                d.close();
                renderDeleted();
              } catch (err) {
                showError(message, err);
                d.close();
              }
            },
          }, "Delete permanently"),
          h("button", { type: "button", onclick: () => d.close() }, "Close")));
    };
    where.replaceChildren(h("section", { class: "card" },
      folderHeader([RECENTLY_DELETED], { open: (p) => { location.hash = libraryHref(p); } }),
      h("p", { class: "muted small" }, "Deleted jobs stay here for 30 days. Tap one to restore it or delete it now."),
      message,
      items.length ? h("ul", { class: "list tiles" }, items.map((item) => h("li", {}, h("a", {
        href: "#/deleted",
        onclick: (e) => { e.preventDefault(); open(item); },
      },
        item.kind === "job" ? h("img", { src: `/api/v1/deleted/${encodeURIComponent(item.entry)}/thumb`, alt: "", loading: "lazy" }) : null,
        h("div", { class: "grow" },
          h("div", { class: "name" }, item.kind === "job" ? item.name : `📚 ${item.name}`),
          h("div", { class: "muted small" }, `From ${item.folder.length ? item.folder.join(" / ") : "Library"} · deleted ${when(item.deleted_at)}`),
          h("div", { class: "muted small" }, `Will be permanently deleted on ${goneOn(item.expires_at)}`))))))
        : h("p", { class: "muted" }, "Nothing deleted in the last 30 days."),
    ));
  } catch (err) {
    showError(where, err);
  }
}

async function renderJob(id) {
  const where = h("div");
  view().replaceChildren(where);
  try {
    const j = await job(id, true);
    const d = j.details;
    const r = j.revision;
    const message = h("div");
    const name = h("input", { value: d.name, "aria-label": "Name" });
    const notes = h("textarea", { "aria-label": "Notes" }, d.notes);
    const base = `/api/v1/jobs/${encodeURIComponent(id)}/revisions/${d.current_revision}`;
    const save = async () => {
      try {
        const changed = await api("PATCH", `/api/v1/jobs/${encodeURIComponent(id)}`, {
          based_on: d.details_revision,
          name: name.value.trim() !== d.name ? name.value.trim() : undefined,
          notes: notes.value !== d.notes ? notes.value : undefined,
        });
        jobs.delete(id);
        message.replaceChildren(h("p", { class: "muted" }, "Saved."));
        Object.assign(d, changed);
      } catch (err) {
        if (err.code === "conflict" && err.detail) {
          message.replaceChildren(h("p", { class: "note" }, err.message, " Your text is still here; the current notes are: ", h("em", {}, err.detail.notes || "(none)")));
          d.details_revision = err.detail.details_revision;
          d.name = err.detail.name;
          d.notes = err.detail.notes;
        } else showError(message, err);
      }
    };
    const cutButtons = state.cutters.map((c) => h("button", {
      class: "primary",
      onclick: async () => {
        try {
          await once(`queue/${c.id}/${id}/${d.current_revision}`, (key) =>
            api("POST", `/api/v1/cutters/${encodeURIComponent(c.id)}/queue`, { job_id: id, revision: d.current_revision, key }));
          location.hash = `#/cutter/${encodeURIComponent(c.id)}`;
        } catch (err) {
          showError(message, err);
        }
      },
    }, state.cutters.length > 1 ? `Cut on ${c.view.name}` : "Cut"));
    where.replaceChildren(h("section", { class: "card" },
      folderHeader(j.folder, { item: d.name, open: (p) => { location.hash = libraryHref(p); }, menu: () => {
        const sheetBox = sheet(
          h("h2", {}, d.name),
          h("div", { class: "stack" },
            h("button", {
              type: "button",
              onclick: () => {
                sheetBox.close();
                pickFolder({
                  heading: `Move “${d.name}”`,
                  action: "Move",
                  start: j.folder,
                  from: j.folder,
                  onPick: async (to) => {
                    await api("POST", `/api/v1/jobs/${encodeURIComponent(id)}/move`, { to: to.join("/") });
                    jobs.delete(id);
                    renderJob(id);
                  },
                });
              },
            }, "Move to another folder"),
            h("button", {
              type: "button",
              class: "danger",
              onclick: async () => {
                if (!confirm(`Delete “${d.name}”? It goes to Recently deleted, where it can be restored for 30 days.`)) return;
                sheetBox.close();
                try {
                  await api("DELETE", `/api/v1/jobs/${encodeURIComponent(id)}`);
                  location.hash = libraryHref(j.folder);
                } catch (err) {
                  showError(message, err);
                }
              },
            }, "Delete"),
            h("button", { type: "button", onclick: () => sheetBox.close() }, "Close")));
      } }),
      h("img", { class: "preview", src: `${base}/preview`, alt: `Preview of ${d.name}` }),
      h("div", { class: "row spaced" }, cutButtons,
        h("a", { class: "button", href: `#/adjust/${encodeURIComponent(id)}` }, "Adjust")),
      j.material_drift ? h("div", { class: "note" },
        `${j.material_drift.name} has different settings in CutCraft now (force ${j.material_drift.force}, speed ${j.material_drift.speed}, ${j.material_drift.passes} pass${j.material_drift.passes === 1 ? "" : "es"}). This job cuts with the settings it was sent with.`,
        h("div", { class: "row spaced" }, h("button", {
          onclick: async () => {
            try {
              await once(`drift/${id}/${d.current_revision}`, (key) => api("POST", `/api/v1/jobs/${encodeURIComponent(id)}/adjust`,
                { revision: d.current_revision, adjustments: { material: j.material_drift.name }, key }));
              jobs.delete(id);
              renderJob(id);
            } catch (err) {
              showError(message, err);
            }
          },
        }, "Use the current settings"))) : null,
      message,
      h("dl", { class: "facts" },
        h("dt", {}, "Material"), h("dd", {}, r.material),
        h("dt", {}, "Size"), h("dd", {}, r.size_mm ? `${inches(r.size_mm[0])} × ${inches(r.size_mm[1])}` : "—"),
        h("dt", {}, "Time"), h("dd", {}, `about ${minutes(r.estimate_s)}`),
        h("dt", {}, "Last cut"), h("dd", {}, when(d.last_cut)),
        h("dt", {}, "Times cut"), h("dd", {}, String(d.times_cut)),
        h("dt", {}, "Version"), h("dd", {}, `${d.current_revision}`,
          r.adjusted_from ? h("span", { class: "muted small" }, ` (adjusted here from ${r.adjusted_from})`) : null,
          r.planner.includes("not independently verified") ? h("span", { class: "muted small" }, " (from another CutCraft version)") : null),
      ),
      h("h3", {}, "Name and notes"),
      name, h("div", { class: "gap" }), notes,
      h("p", {}, h("button", { onclick: save }, "Save")),
      h("h3", {}, "Download"),
      h("div", { class: "row" },
        h("a", { class: "button", href: `${base}/design` }, "Original design"),
        h("a", { class: "button", href: `${base}/cut-lines` }, "Cut lines (SVG)"),
        h("a", { class: "button", href: `${base}/project` }, "CutCraft project"),
      ),
      h("h3", {}, "History"),
      d.history.length ? h("ul", { class: "list" }, [...d.history].reverse().slice(0, 15).map((e) => h("li", {},
        h("div", { class: "item" }, h("div", { class: "grow" },
          h("div", {}, `${e.kind === "test" ? "Test cut" : "Cut"} · ${e.ended}`),
          h("div", { class: "muted small" }, `${when(e.at)} · ${e.cutter} · ${e.by}`)))))) : h("p", { class: "muted" }, "Not cut yet."),
    ));
  } catch (err) {
    showError(where, err);
  }
}

// --- Adjust (spec §12.1) -------------------------------------------------------------------------

/** Length in inches for the inputs, two decimals. */
const toIn = (mm) => (mm / 25.4).toFixed(2);
const fromIn = (text) => Number(String(text).replace(",", ".")) * 25.4;

async function renderAdjust(id) {
  const where = h("div");
  view().replaceChildren(where);
  let j, catalog;
  try {
    [j, catalog] = await Promise.all([job(id, true), api("GET", "/api/v1/catalog")]);
  } catch (err) {
    showError(where, err);
    return;
  }
  const d = j.details;
  const revision = d.current_revision;
  const start = j.settings;
  // The changes so far, relative to the revision; each preview answers with the settings they make.
  const adj = {};
  let shown = start;
  const canvas = h("canvas", { class: "viewer", role: "img", "aria-label": "The cut on the mat" });
  const summary = h("p", { class: "muted small" });
  const problems = h("div");
  const message = h("div");
  let timer = null;
  let asked = 0;
  const preview = () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = ++asked;
      try {
        const p = await api("POST", `/api/v1/jobs/${encodeURIComponent(id)}/adjust/preview`, { revision, adjustments: adj });
        if (mine !== asked) return;
        shown = p.settings;
        drawViewer(canvas, p.geometry, { progress: null });
        const [w, hgt] = shown.design_size_mm || [0, 0];
        summary.textContent = `${inches(w)} × ${inches(hgt)} · about ${minutes(p.estimate_s)}`;
        swap(problems, p.valid ? null : h("div", { class: "note danger" }, "This can't be cut: ", p.problems.join(" ")));
        save.disabled = !p.valid;
        for (const b of cutButtons) b.disabled = !p.valid;
        sync();
      } catch (err) {
        if (mine === asked) showError(problems, err);
      }
    }, 250);
  };
  const change = (fields) => {
    Object.assign(adj, fields);
    preview();
  };

  // Size: the width, the height following.
  const width = h("input", { inputmode: "decimal", "aria-label": "Width in inches" });
  const height = h("span", { class: "muted" });
  // Position: corners and the middle, then nudges of ⅛ in.
  const nudge = (dx, dy) => {
    const [x, y] = shown.offset_mm;
    delete adj.anchor;
    change({ offset_mm: [x + dx * 3.175, y + dy * 3.175] });
  };
  const anchors = [["top-left", "↖"], ["top-right", "↗"], ["centre", "◎"], ["bottom-left", "↙"], ["bottom-right", "↘"]];
  const mirror = h("input", { type: "checkbox", onchange: () => change({ mirror: mirror.checked }) });
  // Material and its settings.
  const material = h("select", { "aria-label": "Material" },
    h("option", { value: "" }, `${start.material.name} (as sent)`),
    ...catalog.materials.map((m) => h("option", { value: m.name },
      senders.length > 1 && catalog.material_from[m.name] ? `${m.name} — ${catalog.material_from[m.name].replace(/^CutCraft on /, "")}` : m.name)));
  const number = (label, key, attrs) => {
    const input = h("input", { type: "number", inputmode: "decimal", "aria-label": label, ...attrs,
      onchange: () => change({ [key]: Number(input.value) }) });
    return [h("label", {}, label), input];
  };
  const [bladeLabel, blade] = number("Blade", "blade", { min: 1, max: 10, step: 1 });
  const [forceLabel, force] = number("Force", "force", { min: 1, max: 33, step: 1 });
  const [speedLabel, speed] = number("Speed", "speed", { min: 1, max: 10, step: 1 });
  const [passesLabel, passes] = number("Passes", "passes", { min: 1, max: 4, step: 1 });
  const overcut = h("input", { type: "number", inputmode: "decimal", min: 0, max: 5, step: 0.1, "aria-label": "Overcut in mm",
    onchange: () => change({ overcut_mm: Number(overcut.value) }) });
  material.addEventListener("change", () => {
    if (material.value) change({ material: material.value, blade: undefined, force: undefined, speed: undefined, passes: undefined, overcut_mm: undefined, mirror: undefined });
    else {
      delete adj.material;
      preview();
    }
  });
  // Mat and piece.
  const matNames = [...new Set([start.mat, ...catalog.mats.map((m) => m.name), ...catalog.standard_mats])];
  const mat = h("select", { "aria-label": "Mat", onchange: () => change({ mat: mat.value }) },
    ...matNames.map((n) => h("option", { value: n, selected: n === start.mat }, n)));
  const pieceOn = h("input", { type: "checkbox" });
  const pieceW = h("input", { inputmode: "decimal", "aria-label": "Piece width in inches" });
  const pieceH = h("input", { inputmode: "decimal", "aria-label": "Piece height in inches" });
  const pieceChange = () => change({ piece_mm: pieceOn.checked && pieceW.value && pieceH.value ? [fromIn(pieceW.value), fromIn(pieceH.value)] : null });
  // The piece's size shows only when there is a piece.
  const pieceRow = h("div", { class: "row" }, h("div", {}, h("label", {}, "Width (in)"), pieceW), h("div", {}, h("label", {}, "Height (in)"), pieceH));
  pieceOn.addEventListener("change", () => { pieceRow.hidden = !pieceOn.checked; });
  pieceOn.addEventListener("change", pieceChange);
  pieceW.addEventListener("change", pieceChange);
  pieceH.addEventListener("change", pieceChange);
  // Weeding box and registration marks.
  const weeding = h("input", { type: "checkbox", onchange: () => change({ weeding_box: weeding.checked }) });
  const margin = h("input", { inputmode: "decimal", "aria-label": "Weeding margin in inches",
    onchange: () => change({ weeding_margin_mm: fromIn(margin.value) }) });
  const marks = h("input", { type: "checkbox", onchange: () => change({ layering_marks: marks.checked }) });

  // The inputs show what the last preview made, except the one being typed in.
  const sync = () => {
    const set = (el, value) => { if (document.activeElement !== el) el.value = value; };
    const [w, hgt] = shown.design_size_mm || [0, 0];
    set(width, toIn(w));
    height.textContent = `× ${toIn(hgt)} in`;
    mirror.checked = shown.mirror;
    set(blade, shown.material.blade);
    set(force, shown.material.force);
    set(speed, shown.material.speed);
    set(passes, shown.material.passes);
    set(overcut, shown.material.overcut_mm);
    if (document.activeElement !== pieceOn) {
      pieceOn.checked = !!shown.piece_mm;
      pieceRow.hidden = !pieceOn.checked;
    }
    if (shown.piece_mm) {
      set(pieceW, toIn(shown.piece_mm[0]));
      set(pieceH, toIn(shown.piece_mm[1]));
    }
    weeding.checked = shown.weeding_box;
    set(margin, toIn(shown.weeding_margin_mm));
    marks.checked = shown.layering_marks;
  };
  width.addEventListener("change", () => {
    delete adj.scale;
    change({ width_mm: fromIn(width.value) });
  });

  const saveWith = async (then) => {
    try {
      const saved = await once(`adjust/${id}/${revision}/${JSON.stringify(adj)}`, (key) =>
        api("POST", `/api/v1/jobs/${encodeURIComponent(id)}/adjust`, { revision, adjustments: adj, key }));
      jobs.delete(id);
      await then(saved);
    } catch (err) {
      showError(message, err);
    }
  };
  const save = h("button", { class: "primary", onclick: () => saveWith(() => { location.hash = `#/job/${encodeURIComponent(id)}`; }) }, "Save");
  const cutButtons = state.cutters.map((c) => h("button", {
    onclick: () => saveWith(async (saved) => {
      await once(`queue/${c.id}/${id}/${saved.current_revision}`, (key) =>
        api("POST", `/api/v1/cutters/${encodeURIComponent(c.id)}/queue`, { job_id: id, revision: saved.current_revision, key }));
      location.hash = `#/cutter/${encodeURIComponent(c.id)}`;
    }),
  }, state.cutters.length > 1 ? `Save and cut on ${c.view.name}` : "Save and cut"));

  const senders = catalog.senders || [];
  const fromWhere = senders.length
    ? `Materials from ${senders.map((x) => `${x.from} (${when(x.at)})`).join(", ")}.`
    : "No material list from CutCraft yet: the job's own material can still be changed.";
  where.replaceChildren(h("section", { class: "card" },
    pageHeader("Adjust", d.name, () => { location.hash = `#/job/${encodeURIComponent(id)}`; }),
    h("p", { class: "muted small" }, `Changes make version ${revision + 1}; version ${revision} stays as it is.`),
    canvas, summary, problems,
    h("h3", {}, "Size and place"),
    h("label", {}, "Width (in)"),
    h("div", { class: "row" }, width, height, h("button", { onclick: () => { delete adj.width_mm; change({ scale: 1 }); } }, "Original size")),
    h("label", {}, "Place"),
    h("div", { class: "row" }, ...anchors.map(([a, icon]) => h("button", { "aria-label": a, onclick: () => { delete adj.offset_mm; change({ anchor: a }); } }, icon))),
    h("div", { class: "row spaced" },
      h("button", { "aria-label": "Left", onclick: () => nudge(-1, 0) }, "←"),
      h("button", { "aria-label": "Up", onclick: () => nudge(0, -1) }, "↑"),
      h("button", { "aria-label": "Down", onclick: () => nudge(0, 1) }, "↓"),
      h("button", { "aria-label": "Right", onclick: () => nudge(1, 0) }, "→")),
    h("div", { class: "row spaced" },
      h("button", { onclick: () => change({ rotation_deg: shown.rotation_deg - 90 }) }, "⟲ Rotate"),
      h("button", { onclick: () => change({ rotation_deg: shown.rotation_deg + 90 }) }, "⟳ Rotate"),
      h("label", { class: "check" }, mirror, "Mirror")),
    h("h3", {}, "Material"),
    material,
    h("p", { class: "muted small" }, fromWhere),
    h("div", { class: "row" }, h("div", {}, bladeLabel, blade), h("div", {}, forceLabel, force), h("div", {}, speedLabel, speed), h("div", {}, passesLabel, passes)),
    h("label", {}, "Overcut (mm)"), overcut,
    h("h3", {}, "Mat and piece"),
    mat,
    h("label", { class: "check" }, pieceOn, "My piece is smaller than the mat"),
    pieceRow,
    h("h3", {}, "Weeding"),
    h("label", { class: "check" }, weeding, "Weeding box"),
    h("label", {}, "Margin (in)"), margin,
    h("label", { class: "check" }, marks, "Registration marks"),
    message,
    h("div", { class: "row spaced" }, save, cutButtons),
  ));
  shown = start;
  sync();
  preview();
}

// --- Settings -----------------------------------------------------------------------------------

/**
 * A Settings section: its title and a one-line summary always shown, its content when open. Like
 * CutCraft's right panel, only one is open at a time; the phone remembers which.
 */
const OPEN_SECTION = "cpi_settings_open";
function fold(key, title) {
  const line = h("span", { class: "muted small fold-summary" });
  const body = h("div", { class: "fold-body" });
  const el = h("details", { class: "card fold" }, h("summary", {}, h("span", { class: "fold-title" }, title), line), body);
  el.dataset.key = key;
  el.addEventListener("toggle", () => {
    try {
      if (el.open) localStorage.setItem(OPEN_SECTION, key);
      else if (localStorage.getItem(OPEN_SECTION) === key) localStorage.removeItem(OPEN_SECTION);
    } catch (_) { /* a private window: nothing remembered */ }
    if (!el.open) return;
    for (const other of document.querySelectorAll("details.fold")) {
      if (other !== el) other.open = false;
    }
  });
  return { el, body, summary: (text) => { line.textContent = text; } };
}

async function renderSettings() {
  const me = state.me;
  const sections = [];
  const device = h("input", { value: cookie("cpi_device") || "", placeholder: "for example Kitchen phone", "aria-label": "This device's name" });
  const deviceFold = fold("device", "This device");
  const nameDevice = () => deviceFold.summary(cookie("cpi_device") || "No name yet");
  nameDevice();
  sections.push(deviceFold.el);
  put(deviceFold.body,
    h("label", {}, "Its name, shown in the queue and history"),
    h("form", {
      class: "row",
      onsubmit: (e) => {
        e.preventDefault();
        const value = device.value.trim().replace(/[^\p{L}\p{N} _'-]/gu, "").slice(0, 40);
        document.cookie = `cpi_device=${encodeURIComponent(value)}; Path=/; Max-Age=31536000; SameSite=Strict; Secure`;
        device.value = value;
        nameDevice();
      },
    }, device, h("button", { type: "submit" }, "Save")),
  );

  if (me.open) {
    const message = h("div");
    const name = h("input", { autocomplete: "username", autocapitalize: "none" });
    const password = h("input", { type: "password", autocomplete: "new-password" });
    const keyList = h("div");
    const keys = await api("GET", "/api/v1/keys").catch(() => []);
    const keep = new Set();
    keyList.replaceChildren(...(keys.length ? [h("p", {}, "Programs approved so far. Tick the ones to keep:"),
      ...keys.map((k) => h("label", { class: "check" }, h("input", { type: "checkbox", onchange: (e) => e.target.checked ? keep.add(k.id) : keep.delete(k.id) }), k.name))] : []));
    const first = fold("first-user", "Add the first user");
    first.summary("Anyone on your network can use cameo-pi");
    sections.push(first.el);
    put(first.body,
      h("p", { class: "muted" }, "After this, everyone needs to log in, and only programs you keep can connect. You'll be the admin."),
      h("label", {}, "User name"), name,
      h("label", {}, "Password (at least 8 characters)"), password,
      keyList,
      message,
      h("p", {}, h("button", {
        class: "primary",
        onclick: async () => {
          try {
            await api("POST", "/api/v1/users/first", { name: name.value.trim(), password: password.value, keep_keys: [...keep] });
            await start();
          } catch (err) {
            showError(message, err);
          }
        },
      }, "Add user and require logins")),
    );
  } else {
    const account = fold("account", `You: ${me.user}`);
    account.summary(me.admin ? "Admin" : "User");
    sections.push(account.el);
    const password = showChangePassword(false);
    password.classList.remove("card");
    put(account.body,
      h("div", { class: "row" }, h("button", {
        onclick: async () => {
          await api("POST", "/api/v1/logout").catch(() => {});
          await start();
        },
      }, "Log out")),
      password,
    );
  }

  // Programs: approvals waiting, and the keys there are (spec §14.2).
  const programsFold = fold("programs", "Programs");
  const programs = programsFold.body;
  sections.push(programsFold.el);
  const loadPrograms = async () => {
    const [waiting, keys] = await Promise.all([api("GET", "/api/v1/pair/waiting"), api("GET", "/api/v1/keys")]);
    const message = h("div");
    programsFold.summary(`${keys.length} connected${waiting.length ? ` · ${waiting.length} asking to connect` : ""}`);
    // A program waiting for an answer opens the section.
    if (waiting.length && !programsFold.el.open) programsFold.el.open = true;
    swap(programs,
      waiting.length ? h("h3", {}, "Asking to connect") : null,
      ...waiting.map((w) => h("div", { class: "card" },
        h("p", {}, h("strong", {}, w.name)),
        h("p", { class: "muted small" }, "Check the program shows this code:"),
        h("p", { class: "code" }, w.code),
        h("div", { class: "row" },
          h("button", { class: "primary", onclick: async () => { await api("POST", "/api/v1/pair/decide", { code: w.code, allow: true }).catch((e) => showError(message, e)); loadPrograms(); } }, "Allow"),
          h("button", { onclick: async () => { await api("POST", "/api/v1/pair/decide", { code: w.code, allow: false }).catch((e) => showError(message, e)); loadPrograms(); } }, "Deny"),
        ))),
      message,
      h("h3", {}, "Connected"),
      keys.length ? h("ul", { class: "list" }, keys.map((k) => h("li", {}, h("div", { class: "item" },
        h("div", { class: "grow" }, h("div", { class: "name" }, k.name), h("div", { class: "muted small" }, `approved by ${k.approved_by}${k.user ? ` · acts as ${k.user}` : ""}`)),
        h("button", { class: "danger", onclick: async () => { if (confirm(`Disconnect ${k.name}?`)) { await api("DELETE", `/api/v1/keys/${encodeURIComponent(k.id)}`).catch((e) => showError(message, e)); loadPrograms(); } } }, "Remove"))))) : h("p", { class: "muted" }, "None yet. Connect CutCraft from its cutter list."),
      h("p", {}, h("button", { onclick: loadPrograms }, "Refresh")),
      aiKey,
    );
  };
  // A key shown once, for an AI tool that can't ask for access itself (spec §13, §14.2).
  const aiKey = h("div");
  const showAiKey = () => {
    const name = h("input", { placeholder: "for example Claude on the MacBook", "aria-label": "The AI tool's name" });
    const out = h("div");
    swap(aiKey,
      h("h3", {}, "An AI tool"),
      h("p", { class: "muted small" }, "AI tools use cameo-pi through MCP, as the person who makes the key. The key is shown once."),
      h("form", { class: "row", onsubmit: async (e) => {
        e.preventDefault();
        try {
          const made = await api("POST", "/api/v1/keys", { name: name.value.trim() });
          const box = h("textarea", { readonly: true, rows: 2, "aria-label": "The new key" });
          box.value = made.key;
          swap(out,
            h("p", {}, "Give the tool this address and key now; the key isn't shown again:"),
            h("p", {}, h("code", {}, `${location.origin}/mcp`)),
            box,
            h("p", { class: "muted small" }, "It goes in an Authorization header: Bearer, then the key."),
            h("p", {}, h("button", { type: "button", onclick: (e) => copyText(e.currentTarget, made.key) }, "Copy key")));
          name.value = "";
          loadPrograms();
        } catch (err) {
          showError(out, err);
        }
      } }, name, h("button", { type: "submit" }, "Make a key")),
      out);
  };
  showAiKey();
  loadPrograms().catch((err) => showError(programs, err));

  if (me.admin && !me.open) {
    const usersFold = fold("users", "Users");
    const users = usersFold.body;
    sections.push(usersFold.el);
    const loadUsers = async () => {
      const list = await api("GET", "/api/v1/users");
      const message = h("div");
      const name = h("input", { autocapitalize: "none", "aria-label": "New user's name" });
      const password = h("input", { type: "password", "aria-label": "Their first password" });
      const role = h("select", { "aria-label": "Role" }, h("option", { value: "user" }, "User"), h("option", { value: "admin" }, "Admin"));
      const call = async (fn) => {
        try {
          await fn();
          loadUsers();
        } catch (err) {
          showError(message, err);
        }
      };
      usersFold.summary(`${list.length} user${list.length === 1 ? "" : "s"}`);
      swap(users,
        // The name on its own line, the actions under it: a phone has no room for both side by side.
        h("ul", { class: "list" }, list.map((u) => h("li", {}, h("div", { class: "item stack" },
          h("div", {}, h("div", { class: "name" }, u.name), h("div", { class: "muted small" }, u.role === "admin" ? "Admin" : "User", u.name === me.user ? " · you" : "", u.must_change ? " · must choose a password" : "")),
          h("div", { class: "actions" },
            h("button", { onclick: () => call(() => api("PATCH", `/api/v1/users/${encodeURIComponent(u.name)}`, { role: u.role === "admin" ? "user" : "admin" })) }, u.role === "admin" ? "Make user" : "Make admin"),
            h("button", { onclick: () => { const p = prompt(`A temporary password for ${u.name} (at least 8 characters):`); if (p) call(() => api("PATCH", `/api/v1/users/${encodeURIComponent(u.name)}`, { password: p })); } }, "Reset password"),
            u.name !== me.user ? h("button", { class: "danger", onclick: () => { if (confirm(`Remove ${u.name}?`)) call(() => api("DELETE", `/api/v1/users/${encodeURIComponent(u.name)}`)); } }, "Remove") : null,
          ),
        )))),
        message,
        h("h3", {}, "Add a user"),
        h("label", {}, "Name"), name,
        h("label", {}, "First password (they choose their own at first login)"), password,
        h("label", {}, "Role"), role,
        h("p", {}, h("button", { class: "primary", onclick: () => call(() => api("POST", "/api/v1/users", { name: name.value.trim(), password: password.value, role: role.value })) }, "Add user")),
      );
    };
    loadUsers().catch((err) => showError(users, err));
  }

  const backupFold = fold("backup", "Library backup");
  sections.push(backupFold.el);
  renderBackup(backupFold);

  const about = fold("about", "About");
  about.summary(`cameo-pi ${state.version}`);
  put(about.body, h("p", { class: "muted" }, `cameo-pi ${state.version}`),
    h("p", { class: "muted small" }, "A network cut server for Silhouette Cameo cutters."));
  sections.push(about.el);
  view().replaceChildren(...sections);
  // The section open last time on this phone, if it's still here.
  let remembered = null;
  try {
    remembered = localStorage.getItem(OPEN_SECTION);
  } catch (_) { /* nothing remembered */ }
  const again = sections.find((el) => el.dataset.key === remembered);
  if (again && !document.querySelector("details.fold[open]")) again.open = true;
}

/** The setup guides, in a dialog of their own so the card stays short (spec §15). */
const GUIDES = [
  ["Mac", [
    "System Settings ▸ General ▸ Sharing: turn on Remote Login.",
    "Allow access for your user, or better, a user made just for backups.",
    "Host: the Mac's name (shown under Remote Login, ending in .local). User: that user. Folder: for example cameo-pi-backup (inside its home folder).",
  ]],
  ["Windows", [
    "Settings ▸ System ▸ Optional features ▸ Add a feature: OpenSSH Server. Then in Services, start OpenSSH SSH Server and set it to Automatic.",
    "Host: the PC's name or address. User: your Windows user name. Folder: for example C:/Users/you/cameo-pi-backup.",
    "For an administrator account, Windows reads keys from C:/ProgramData/ssh/administrators_authorized_keys instead of your own .ssh folder.",
  ]],
  ["Linux", [
    "Install and start the OpenSSH server (on NixOS: services.openssh.enable).",
    "Put the backup key in the user's ~/.ssh/authorized_keys (on NixOS: users.users.<name>.openssh.authorizedKeys.keys).",
    "Folder: for example /home/you/cameo-pi-backup, or a path on a big disk.",
  ]],
  ["Synology / NAS", [
    "Control Panel ▸ Terminal & SNMP: enable SSH. Control Panel ▸ File Services ▸ FTP: enable SFTP.",
    "Use a user with access to a shared folder; the folder is then that share's path, for example /backup/cameo-pi.",
    "Some NAS accounts can't use keys over SFTP: choose Password then.",
  ]],
  ["The key", [
    "Copy the backup key from the card into the other computer's authorized_keys for the backup user, one line.",
    "A user made only for backups limits what the Pi's key can reach if the Pi were ever lost or stolen.",
    "Make a new key if the old one may have leaked; then replace the line on the other computer.",
  ]],
  ["Without a Pi", [
    "Plain backups are ordinary folders: open any job's project.cutcraft (in library/…/revisions/<n>/) with CutCraft's File ▸ Open, design included.",
    "Encrypted backups use standard age files. The newest record in commits names its encrypted key and manifest under generations. age -d on that key file (with the password) gives key.txt; age -d -i key.txt opens the manifest and objects. Retained files from before a password change may need their earlier key/password. Easier: set up a new Pi and Restore.",
  ]],
];

function showGuides() {
  const tabs = h("div", { class: "row guide-tabs" });
  const page = h("div", { class: "guide-page" });
  const open = (i) => {
    for (const [n, b] of [...tabs.children].entries()) b.classList.toggle("primary", n === i);
    swap(page, h("ol", { class: "steps" }, GUIDES[i][1].map((step) => h("li", {}, step))));
  };
  GUIDES.forEach(([name], i) => tabs.append(h("button", { type: "button", onclick: () => open(i) }, name)));
  const dialog = h("dialog", { class: "guide" },
    h("div", { class: "row" }, h("h2", { class: "grow" }, "Setting up the backup computer"), h("button", { type: "button", onclick: () => dialog.close() }, "Close")),
    tabs, page);
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  open(0);
  dialog.showModal();
}

/** How the backup stands, in one line, and its tone. */
function backupHealth(b) {
  switch (b.health) {
    case "off": return ["⚠ Not set up: the designs are only on this Pi's SD card", "danger"];
    case "never": return [b.identity ? "Not backed up yet" : "Not backed up yet: use Test connection to accept the computer", "warn"];
    case "retrying": return [`⚠ Backup failing since ${when(b.failing_since)}: trying again`, "warn"];
    case "warning": return [`⚠ ${b.maintenance_warning}`, "warn"];
    case "failed": return [`⚠ Backup needs attention: ${b.last_attempt?.message || "check the backup settings"}. Repair the setup, then Test connection or Back up now.`, "danger"];
    default: return [`Backed up ${when(b.last_success)}`, "ok"];
  }
}

/** Backup to another computer (spec §15): where and how, the computer's identity, Test, encryption,
 *  how often, Back up now and Restore. */
/** Copies text and says so on the button for a moment. */
async function copyText(button, text) {
  const label = button.textContent;
  try {
    await navigator.clipboard.writeText(text);
    button.textContent = "Copied ✓";
  } catch (_) {
    button.textContent = "Couldn't copy: select it instead";
  }
  setTimeout(() => { button.textContent = label; }, 2000);
}

function renderBackup(section) {
  const card = section.body;
  // Kept across redraws, so a Test result survives the save that precedes it.
  const message = h("div");
  const result = h("div");
  const load = async () => {
    let b;
    try {
      b = await api("GET", "/api/v1/backup");
    } catch (err) {
      showError(card, err);
      return;
    }
    state.backup = b;
    showBanner();
    const [line, tone] = backupHealth(b);
    section.summary(line);
    section.el.classList.toggle("warn", tone === "warn");
    section.el.classList.toggle("danger", tone === "danger");
    const call = async (fn) => {
      try {
        const out = await fn();
        await load();
        return out;
      } catch (err) {
        showError(message, err);
      }
    };
    const field = (label, value, attrs) => {
      const input = h("input", { value: value ?? "", autocapitalize: "none", ...attrs });
      return [h("label", {}, label), input];
    };
    const [hostLabel, host] = field("Computer (name or address)", b.host, { placeholder: "for example mac-mini.local" });
    const [portLabel, port] = field("Port", b.port, { inputmode: "numeric" });
    const [userLabel, user] = field("User", b.user, {});
    const [folderLabel, folder] = field("Folder", b.folder, { placeholder: "for example cameo-pi-backup" });
    const signIn = h("select", { "aria-label": "Sign in with" },
      h("option", { value: "key", selected: b.sign_in === "key" }, "The backup key"),
      h("option", { value: "password", selected: b.sign_in === "password" }, "A password"));
    const password = h("input", { type: "password", autocomplete: "new-password", placeholder: b.has_password ? "saved (type to change)" : "", "aria-label": "Password" });
    const keyBox = h("textarea", { readonly: true, rows: 3, "aria-label": "The backup key's public part" });
    keyBox.value = b.public_key || "";
    const keyPart = h("div", {},
      b.public_key ? [keyBox, h("div", { class: "row" },
        h("button", { type: "button", onclick: (e) => copyText(e.currentTarget, b.public_key) }, "Copy key"),
        h("button", { type: "button", onclick: () => { if (confirm("Make a new key? The other computer then needs the new one.")) call(() => api("POST", "/api/v1/backup/key")); } }, "New key"))]
        : h("p", {}, h("button", { type: "button", onclick: () => call(() => api("POST", "/api/v1/backup/key")) }, "Make a backup key")),
      h("p", { class: "muted small" }, "Add it to that user's authorized_keys on the other computer."));
    const passwordPart = h("div", {}, password, h("p", { class: "muted small" }, "Stored on the Pi so backups can run on their own; anyone with the SD card could read it. Use an account just for backups, or the key where the computer allows it."));
    const showSignIn = () => {
      keyPart.hidden = signIn.value !== "key";
      passwordPart.hidden = signIn.value !== "password";
    };
    signIn.addEventListener("change", showSignIn);
    const every = h("select", { "aria-label": "How often" },
      ...[["change", "A few minutes after every change"], ["daily", "Every day at"], ["weekly", "Every week on"], ["manual", "Only when I press Back up now"]]
        .map(([v, t]) => h("option", { value: v, selected: b.every === v }, t)));
    const weekday = h("select", { "aria-label": "Day" },
      ...["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"].map((d, i) => h("option", { value: i, selected: b.weekday === i }, d)));
    const at = h("input", { type: "time", value: b.at, "aria-label": "Time" });
    const when_ = h("div", { class: "row" }, weekday, at);
    const showWhen = () => {
      when_.hidden = !["daily", "weekly"].includes(every.value);
      weekday.hidden = every.value !== "weekly";
    };
    every.addEventListener("change", showWhen);
    const save = () => (swap(message), call(() => api("PATCH", "/api/v1/backup", {
      host: host.value, port: Number(port.value) || 22, user: user.value, folder: folder.value,
      sign_in: signIn.value, ...(password.value ? { password: password.value } : {}),
      every: every.value, at: at.value || "03:00", weekday: Number(weekday.value),
    })));
    const test = async () => {
      await save();
      swap(result, h("p", { class: "muted" }, "Connecting…"));
      try {
        const t = await api("POST", "/api/v1/backup/test");
        if (t.identity_unknown) {
          swap(result, h("div", { class: "note" },
            h("p", {}, t.message),
            h("p", {}, h("code", {}, `${t.identity_unknown.algorithm} ${t.identity_unknown.fingerprint}`)),
            h("p", { class: "muted small" }, "On the other computer, ssh-keygen -lf on its host key shows the same."),
            h("button", { class: "primary", onclick: async () => { await call(() => api("POST", "/api/v1/backup/identity", t.identity_unknown)); test(); } }, "Accept this identity")));
          return;
        }
        const free = t.free_bytes ? ` ${(t.free_bytes / 1e9).toFixed(1)} GB free there.` : "";
        const there = t.backup ? ` It has this Pi's backup from ${when(t.backup.updated)}.` : "";
        swap(result, h("p", { class: "state ok" }, `✓ ${t.message}${there}${free}`));
      } catch (err) {
        showError(result, err);
      }
    };
    const identity = b.identity
      ? h("p", { class: "small" }, `Identity: ${b.identity.algorithm} ${b.identity.fingerprint} (accepted ${when(b.identity.accepted_at)}) `,
        h("button", { onclick: () => { if (confirm("Forget this identity? The next connection asks again.")) call(() => api("DELETE", "/api/v1/backup/identity", { host: b.identity.host, port: b.identity.port })); } }, "Forget"))
      : h("p", { class: "muted small" }, "The computer's identity is accepted on the first Test connection.");
    // Encryption.
    const encPass = h("input", { type: "password", autocomplete: "new-password", "aria-label": "Encryption password" });
    const encAgain = h("input", { type: "password", autocomplete: "new-password", "aria-label": "The password again" });
    // A switch: on reveals the password fields; off (when encrypted) asks first.
    const encSwitch = h("input", { type: "checkbox", role: "switch", checked: b.encrypted, "aria-label": "Encrypt the backup" });
    const encFields = h("div", { class: "reveal", hidden: true },
      h("p", { class: "muted small" }, "Without the password, nobody can read the backup, including you: keep it somewhere safe. The next backup encrypts the existing one in place."),
      h("label", {}, "Password (at least 10 characters)"), encPass,
      h("label", {}, "The password again"), encAgain,
      h("p", {}, h("button", { class: "primary", onclick: () => {
        if (encPass.value !== encAgain.value) return showError(message, new Error("The two passwords differ."));
        call(() => api("POST", "/api/v1/backup/encryption", { password: encPass.value }));
      } }, "Turn encryption on")));
    encSwitch.addEventListener("change", () => {
      if (b.encrypted) {
        if (confirm("Turn encryption off? The next backup turns the copy back into plain folders, and the encrypted copies of removed files are deleted.")) call(() => api("POST", "/api/v1/backup/encryption", { password: null }));
        else encSwitch.checked = true;
        return;
      }
      encFields.hidden = !encSwitch.checked;
      encState.hidden = encSwitch.checked;
      if (encSwitch.checked) encPass.focus();
    });
    const encState = h("p", { class: "muted small" }, b.encrypted
        ? "🔒 On. The password is needed to restore; the Pi doesn't keep it."
        : "Off: the copy is plain folders that anyone with access to that computer can open.");
    const encryption = h("div", {},
      h("label", { class: "switch" }, h("span", { class: "grow" }, "Encrypt the backup"), encSwitch, h("span", { class: "track", "aria-hidden": "true" })),
      encState, encFields);
    // Restore.
    const restoreBox = h("div");
    const restore = async () => {
      let pass = null;
      swap(restoreBox, h("p", { class: "muted" }, "Fetching and checking the whole backup…"));
      const watch = setInterval(async () => {
        const v = await api("GET", "/api/v1/backup").catch(() => null);
        if (v && v.running && v.running.what === "restore") swap(restoreBox, h("p", { class: "muted" }, `Fetching: ${v.running.done_files} of ${v.running.total_files} files`));
      }, 1500);
      try {
        let r;
        for (;;) {
          try {
            r = await api("POST", "/api/v1/backup/restore", { password: pass });
            break;
          } catch (err) {
            if (err.code !== "password") throw err;
            pass = prompt(`${err.message}\nThe backup's password:`);
            if (pass === null) { clearInterval(watch); swap(restoreBox); return; }
          }
        }
        clearInterval(watch);
        swap(restoreBox, h("div", { class: "note" },
          h("p", {}, `The backup of ${r.from} (${when(r.updated)}) has ${r.jobs} job${r.jobs === 1 ? "" : "s"}.`),
          h("p", {}, "Replace this Pi's library with it? The current library goes to Recently deleted."),
          r.replace_allowed ? null : h("p", { class: "muted small" }, "A cutter is in use: Replace waits until no cutter has a turn or a queue."),
          h("div", { class: "row" },
            h("button", { class: "danger", disabled: !r.replace_allowed, onclick: async () => {
              try {
                const applied = await api("POST", "/api/v1/backup/restore/apply");
                if (!applied || applied.restored !== true) throw new Error("The library was not restored.");
                await load();
                swap(result, h("p", { class: "state ok" }, "The library is restored."));
              } catch (err) { showError(restoreBox, err); }
            } }, "Replace the library"),
            h("button", { onclick: async () => { await api("DELETE", "/api/v1/backup/restore").catch(() => {}); swap(restoreBox); } }, "Cancel"))));
      } catch (err) {
        clearInterval(watch);
        showError(restoreBox, err);
      }
    };
    const running = b.running && b.running.what === "backup";
    const progress = running ? h("p", { class: "muted" }, `Backing up: ${b.running.done_files} of ${b.running.total_files} files`) : null;
    if (running) setTimeout(load, 1500);
    swap(card,
      b.last_attempt && !b.last_attempt.ok ? h("p", { class: "error" }, b.last_attempt.message) : null,
      progress,
      h("p", { class: "muted small" }, "Every job is copied to another computer over SFTP, so a dead SD card loses nothing. ",
        h("a", { href: "#", onclick: (e) => { e.preventDefault(); showGuides(); } }, "How to set up the other computer")),
      hostLabel, host, h("div", { class: "row" }, h("div", {}, portLabel, port), h("div", { class: "grow" }, userLabel, user)),
      folderLabel, folder,
      h("label", {}, "Sign in with"), signIn, keyPart, passwordPart,
      identity,
      h("label", {}, "How often"), every, when_,
      h("div", { class: "row spaced" },
        h("button", { class: "primary", onclick: save }, "Save"),
        h("button", { onclick: test }, "Test connection"),
        h("button", { disabled: running || b.health === "off", onclick: async () => { await call(() => api("POST", "/api/v1/backup/run")); setTimeout(load, 800); } }, running ? "Backing up…" : "Back up now")),
      result,
      message,
      h("h3", {}, "Encryption"),
      encryption,
      h("h3", {}, "Restore"),
      h("p", { class: "muted small" }, "Brings the whole library back from the backup, for example on a new Pi after an SD card failure."),
      h("p", {}, h("button", { disabled: b.health === "off", onclick: restore }, "Restore from the backup…")),
      restoreBox,
    );
    showSignIn();
    showWhen();
  };
  load();
}

// --- Start --------------------------------------------------------------------------------------

async function start() {
  try {
    const me = await loadMe();
    showBanner();
    if (me.locked) {
      document.getElementById("tabs").hidden = true;
      view().replaceChildren(h("p", { class: "note danger" }, "cameo-pi's account files are damaged. Run sudo cameo-pi reset-admin on the Pi."));
      return;
    }
    if (me.needsLogin) {
      showLogin();
      return;
    }
    if (me.mustChange) {
      document.getElementById("tabs").hidden = true;
      view().replaceChildren(showChangePassword(true));
      return;
    }
    document.getElementById("tabs").hidden = false;
    state.backup = await api("GET", "/api/v1/backup").catch(() => null);
    showBanner();
    await refreshState();
    // ?nolive shows one snapshot without the live stream (for screenshots and slow links).
    if (!new URLSearchParams(location.search).has("nolive")) connect();
  } catch (err) {
    showError(view(), err);
  }
}

document.addEventListener("visibilitychange", () => {
  // Back on the page after the phone slept: catch up at once (spec §12).
  if (document.visibilityState === "visible" && state.me && !state.me.needsLogin) {
    refreshState().catch(() => {});
    if (!source || source.readyState === EventSource.CLOSED) connect();
  }
});

start();
