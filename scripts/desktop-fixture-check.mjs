// Opens every authored issue example in the desktop through real mouse input and compares the
// result with the 1.0.5 reference observations in docs/design-audit/fixtures.json.
// For each fixture: open, compare the initial report, answer each question through its visible
// Issues button, Undo back to the initial report, Redo, then Save to the library.
// Practice service only; no motion. Captures stay in the caller's scratch directory.
import fs from 'node:fs/promises';
import path from 'node:path';
const sleep = ms => new Promise(r => setTimeout(r, ms));
const [url, debugPort, repo, artifacts, only] = process.argv.slice(2);
if (!url || !debugPort || !repo || !artifacts) throw Error('Usage: desktop-fixture-check.mjs URL DEBUG_PORT REPO ARTIFACT_DIRECTORY [FIXTURE_PREFIX]');

const tab = await (await fetch(`http://127.0.0.1:${debugPort}/json/new?about:blank`, {method: 'PUT'})).json();
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise(r => ws.addEventListener('open', r, {once: true}));
let next = 0;
const waiting = new Map();
ws.addEventListener('message', e => {
  const m = JSON.parse(e.data);
  // The desktop asks before replacing unsaved work; accept, as a person continuing would.
  if (m.method === 'Page.javascriptDialogOpening') { void cdp('Page.handleJavaScriptDialog', {accept: true}); return; }
  if (!m.id) return;
  const p = waiting.get(m.id); waiting.delete(m.id);
  m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result);
});
function cdp(method, params = {}) { return new Promise((resolve, reject) => { const id = ++next; waiting.set(id, {resolve, reject}); ws.send(JSON.stringify({id, method, params})); }); }
async function page(expression) { const r = await cdp('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true}); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; }
async function state() { return await page('JSON.parse(CutCraft.state())'); }
async function controls() { return await page('CutCraft.controls()'); }
async function until(fn, why, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { if (await fn()) return; await sleep(100); } throw Error(why + ': ' + JSON.stringify((await state()).message)); }
async function settled() {
  await until(async () => { const s = await state(); return !s.pending && !s.snapshot.busy; }, 'Service settled', 60000);
  await sleep(250);
}
const VIEW = {width: 1600, height: 1200};
async function press(x, y) {
  await cdp('Input.dispatchMouseEvent', {type: 'mouseMoved', x, y});
  await cdp('Input.dispatchMouseEvent', {type: 'mousePressed', x, y, button: 'left', clickCount: 1});
  await cdp('Input.dispatchMouseEvent', {type: 'mouseReleased', x, y, button: 'left', clickCount: 1});
  await sleep(150);
}
// Click a rendered control. If it is laid out below the visible panel, scroll the panel with the
// wheel, as a person would; report it as unreachable if scrolling does not bring it into view.
async function click(label) {
  await until(async () => !!(await controls())[label], 'Control rendered: ' + label, 8000);
  for (let attempt = 0; attempt < 12; attempt++) {
    const r = (await controls())[label];
    if (!r) throw Error('Control disappeared: ' + label);
    const x = (r[0] + r[2]) / 2, y = (r[1] + r[3]) / 2;
    if (y > 0 && y < VIEW.height && x > 0 && x < VIEW.width) { await press(x, y); return; }
    await cdp('Input.dispatchMouseEvent', {type: 'mouseWheel', x: Math.min(Math.max(x, 1), VIEW.width - 1), y: VIEW.height / 2, deltaX: 0, deltaY: y > 0 ? 300 : -300});
    await sleep(150);
  }
  throw Error('Control not reachable in the viewport: ' + label);
}
async function shot(name) {
  const s = await cdp('Page.captureScreenshot', {format: 'png'});
  await fs.writeFile(path.join(artifacts, name), Buffer.from(s.data, 'base64'));
}

// The visible label for an answer, as crates/ui/src/panels.rs `choice_label` draws it.
// Kept in step by hand: a mismatch shows up as "no visible button", which is itself a finding.
function choiceLabel(c, displayUnits) {
  const inches=displayUnits==='inches';
  const length=v=>(Number(v)/(inches?25.4:1)).toFixed(inches?2:1);
  const unit=inches?'in':'mm';
  const d = c.decision;
  const units = {in: 'Inches', mm: 'Millimetres', cm: 'Centimetres', ft: 'Feet', m: 'Metres'};
  const n = v => JSON.stringify(v);
  switch (d) {
    case 'dxf_units': return units[c.unit] || 'Choose units';
    case 'background': return c.cut === true ? 'Cut it' : "Don't cut";
    case 'page': { const m = /^page\/(\d+)$/.exec(c.page || ''); return m ? `Page ${Number(m[1]) + 1}` : (c.page || ''); }
    case 'font_substitute': return 'Use this substitute';
    case 'font_from_file': return 'Use the letter shapes saved in the file';
    case 'exclude_picture': case 'exclude_object': return "Don't cut";
    case 'fit_artwork': return "Use the artwork's area";
    case 'keep_page': return 'Trim the art to the page';
    case 'accept_text_overflow': return "Leave out the text that doesn't fit";
    case 'cut_solid': return c.colour ? `Cut in ${c.colour}` : 'Cut as solid';
    case 'picture_size': return `Use ${length(c.width_mm)} ${unit}`;
    case 'picture_frame': return `Frame ${(c.frame || 0) + 1}`;
    case 'trace': return c.colours === 'several' ? 'Trace in several colours' : 'Trace in one colour';
    case 'trace_photo': return 'Trace it anyway';
    case 'accept_warning': return c.warning === 'duplicate.near' ? 'Keep both' : 'Cut them anyway';
    case 'import_route': return c.route === 'pdf_appearance' ? 'Use the page appearance' : 'Use the editable layers';
    case 'stroke_colour_role': return c.role === 'cut_line' ? 'Cut these along their centre' : c.role === 'part_of_shape' ? 'Make them part of their shapes' : 'Cut around these as shapes';
    case 'stroke_role': return c.role === 'cut_line' ? 'Cut along line' : c.role === 'part_of_shape' ? 'Part of the shape' : 'Cut around shape';
    case 'confirm_size': return `Confirm ${length(c.width_mm)} × ${length(c.height_mm)} ${unit}`;
    default: return 'Use this choice';
  }
}
// Which answer to give: the one a person making the design cuttable would pick.
function preferred(question) {
  const choices = question.choices || [];
  const pick = f => choices.find(f);
  return pick(c => c.decision === 'background' && c.cut !== true)
    || pick(c => c.decision === 'page' && c.page === 'page/0')
    || pick(c => c.decision === 'picture_frame' && (c.frame || 0) === 0)
    || pick(c => c.decision === 'trace' && c.colours !== 'several')
    || choices[0];
}

function summary(report) {
  const sorted = a => [...a].sort();
  return {
    cut_ready: !!report?.cut_ready,
    pending: sorted((report?.pending || []).map(p => typeof p === 'string' ? p : p.decision)),
    diagnostics: sorted((report?.diagnostics || []).map(d => `${d.code}:${d.severity}`)),
    changes: sorted((report?.changes || []).map(c => typeof c === 'string' ? c : c.code)),
  };
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
// After an edit, the source artwork must stay visible while a newer render is fetched (VIEW-01).
// A newly opened design has no earlier image to keep. Either way the render must arrive.
async function artwork(step, finding, hadPicture = false) {
  const s = await state();
  const art = s.snapshot.design?.art;
  if (!art) return false;
  if (step !== 'open' && hadPicture && !s.picture) finding(step, 'artwork pane blank while the new render is fetched (VIEW-01: keep the old image)');
  const start = Date.now();
  try { await until(async () => { const t = await state(); return t.picture && t.picture_id === t.snapshot.design?.art?.id; }, 'artwork rendered', 20000); }
  catch { finding(step, `source artwork not rendered within 20 s`); return; }
  const ms = Date.now() - start;
  if (ms > 3000) finding(step, `source artwork took ${(ms / 1000).toFixed(1)} s to render`);
  return true;
}

await cdp('Runtime.enable');
await cdp('Page.enable');
await cdp('Emulation.setDeviceMetricsOverride', {...VIEW, deviceScaleFactor: 1, mobile: false});
await cdp('Page.navigate', {url: url + '/desktop'});
await until(async () => await page('!!window.CutCraft && Object.keys(CutCraft.controls()).length>0'), 'Desktop ready');

const manifest = JSON.parse(await fs.readFile(path.join(repo, 'docs/design-audit/fixtures.json'), 'utf8'));
const results = [];
for (const fixture of manifest.fixtures) {
  const file = path.basename(fixture.path);
  if (only && !file.startsWith(only)) continue;
  const stem = file.replace(/\.[^.]+$/, '');
  const result = {fixture: file, findings: []};
  const finding = (step, text) => { result.findings.push(`${step}: ${text}`); };
  results.push(result);
  try {
    // 1. Open through the page's own file input.
    const before = (await state()).snapshot;
    const doc = await cdp('DOM.getDocument');
    const input = await cdp('DOM.querySelector', {nodeId: doc.root.nodeId, selector: '#file'});
    await cdp('DOM.setFileInputFiles', {nodeId: input.nodeId, files: [path.join(repo, fixture.path)]});
    await until(async () => { const s = (await state()).snapshot; return s.design && (s.incarnation !== before.incarnation || s.revision !== before.revision) && s.design.name.includes(stem.slice(0, 12)); }, 'Open ' + file, 60000);
    await settled();
    const opened = await state();
    if (opened.message) finding('open', `message shown: ${opened.message}`);
    const initial = summary(opened.snapshot.ui.report);
    result.initial = initial;
    if (fixture.initial_reference) {
      const expected = summary(fixture.initial_reference);
      for (const key of ['cut_ready', 'pending', 'diagnostics', 'changes'])
        if (!same(initial[key], expected[key])) finding('initial', `${key} is ${JSON.stringify(initial[key])}, reference ${JSON.stringify(expected[key])}`);
    } else result.note = 'no reference observation; initial report not compared';
    if (initial.cut_ready !== opened.snapshot.design.cut_ready) finding('initial', 'report and design disagree on cut_ready');
    const hadPicture = await artwork('open', finding);
    await shot(`${stem}-1-opened.png`);

    // 2. Answer each question through its visible button, until none remain.
    let answers = 0;
    for (let round = 0; round < 6; round++) {
      const s = (await state()).snapshot;
      const question = (s.ui.report.pending || [])[0];
      if (!question) break;
      const choice = preferred(question);
      if (!choice) { finding('answer', `question ${question.decision} offers no choices`); break; }
      const label = choiceLabel(choice, s.ui.units);
      const revision = s.revision;
      try { await click(label); }
      catch (e) { finding('answer', `${question.decision}: no usable "${label}" button (${e.message.split(':')[0]})`); break; }
      try { await until(async () => (await state()).snapshot.revision !== revision, `Answer ${label} applied`); }
      catch (e) { finding('answer', `${question.decision}: clicking "${label}" changed nothing; message ${JSON.stringify((await state()).message)}`); break; }
      await settled();
      answers++;
      const after = (await state()).snapshot.ui.report;
      if ((after.pending || []).some(p => p.decision === question.decision && JSON.stringify(p.choices) === JSON.stringify(question.choices)))
        { finding('answer', `${question.decision}: still pending after "${label}"`); break; }
    }
    const answered = await state();
    result.answers = answers;
    result.after = summary(answered.snapshot.ui.report);
    if (answers) { await artwork('answer', finding, hadPicture); await shot(`${stem}-2-answered.png`); }
    const blockers = result.after.diagnostics.filter(d => d.endsWith(':blocking'));
    if (!result.after.cut_ready && !result.after.pending.length)
      result.blocked_by = blockers.length ? blockers : ['(not ready, nothing pending, no blocking diagnostic shown)'];

    // 3. Undo every answer, expect the initial report; Redo, expect the answered report.
    if (answers) {
      for (let i = 0; i < answers; i++) {
        const revision = (await state()).snapshot.revision;
        try { await click('Undo'); await until(async () => (await state()).snapshot.revision !== revision, 'Undo applied'); await settled(); }
        catch (e) { finding('undo', `step ${i + 1}: ${e.message}`); break; }
      }
      const undone = summary((await state()).snapshot.ui.report);
      if (!same(undone, initial)) finding('undo', `report after Undo ${JSON.stringify(undone)} differs from opened ${JSON.stringify(initial)}`);
      for (let i = 0; i < answers; i++) {
        const revision = (await state()).snapshot.revision;
        try { await click('Redo'); await until(async () => (await state()).snapshot.revision !== revision, 'Redo applied'); await settled(); }
        catch (e) { finding('redo', `step ${i + 1}: ${e.message}`); break; }
      }
      const redone = summary((await state()).snapshot.ui.report);
      if (!same(redone, result.after)) finding('redo', `report after Redo ${JSON.stringify(redone)} differs from answered ${JSON.stringify(result.after)}`);
    }

    // 4. Save into the library (first Save opens Save As).
    try {
      await click('Save');
      await click('Save project');
      await until(async () => { const d = (await state()).snapshot.design; return d?.saved && !d.unsaved; }, 'Saved');
      await settled();
      result.saved = true;
    } catch (e) {
      finding('save', `${e.message}`);
      // Leave no dialog open for the next fixture.
      await cdp('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27});
      await cdp('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27});
    }
  } catch (e) {
    finding('error', e.message);
    await shot(`${stem}-error.png`).catch(() => {});
  }
  const status = result.findings.length ? 'DIFF' : 'PASS';
  console.log(`${status} ${file}${result.note ? '  (' + result.note + ')' : ''}${result.blocked_by ? '  [blocked: ' + result.blocked_by.join(', ') + ']' : ''}`);
  for (const f of result.findings) console.log('     ' + f);
}
await fs.writeFile(path.join(artifacts, 'fixture-results.json'), JSON.stringify(results, null, 1));
const diffs = results.filter(r => r.findings.length).length;
console.log(`\n${results.length - diffs}/${results.length} fixtures match the reference and complete open/answer/Undo/Redo/Save. Details: ${artifacts}/fixture-results.json`);
ws.close();
process.exitCode = diffs ? 1 : 0;
