import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const mainRs = await readFile(resolve(root, 'src/main.rs'), 'utf8');
const marker = "var ICON_EDIT";
const markerIndex = mainRs.indexOf(marker);
if (markerIndex < 0) throw new Error('desktop script marker not found');
const scriptStart = mainRs.lastIndexOf('<script>', markerIndex);
const scriptEnd = mainRs.indexOf('</script>', markerIndex);
if (scriptStart < 0 || scriptEnd < 0) throw new Error('desktop script block not found');

const desktopScript = mainRs
  .slice(scriptStart + '<script>'.length, scriptEnd)
  .replaceAll('{{', '{')
  .replaceAll('}}', '}')
  .replaceAll('{replace_done_js}', '{count} replaced')
  .replaceAll('{windows_ime_focus_fix}', 'true');

const browser = await chromium.launch(process.env.MD_PREVIEW_TEST_BROWSER ? {
  executablePath: process.env.MD_PREVIEW_TEST_BROWSER,
} : {});
const page = await browser.newPage({ viewport: { width: 900, height: 700 } });

await page.setContent(`<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <base id="base-href">
    <style id="hljs-light"></style>
    <style id="hljs-dark"></style>
    <style>
      body.finding .findbar { display: flex; }
      .findbar { display: none; }
      .replace-row { display: none; }
      body.replacing .replace-row { display: flex; }
      #editor { display: none; }
      body.editing #preview { display: none; }
      body.editing #editor { display: block; }
      body.split-view #app { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
      body.split-view #preview,
      body.split-view #editor { display: block; min-width: 0; }
      #preview mark.search-hit { background: #fff2a8; }
      #preview mark.search-hit.current { background: #ffcc4d; }
      .spacer { height: 900px; }
    </style>
  </head>
  <body>
    <div class="tabbar">
      <div id="tabs"></div>
      <div id="doc-stats"></div>
      <button id="tab-open"></button>
      <button id="tab-about"></button>
    </div>
    <div class="toolbar">
      <button id="btn-open"></button>
      <button id="btn-search"></button>
      <button id="btn-toggle"></button>
      <button id="btn-split"></button>
      <button id="btn-print"></button>
      <button id="btn-toc"></button>
      <button id="btn-save-as"></button>
      <button id="btn-word"></button>
      <div id="zoom-control">
        <button id="btn-zoom"></button>
        <button id="btn-zoom-out"></button>
        <button id="btn-zoom-reset"></button>
        <button id="btn-zoom-in"></button>
      </div>
      <button id="btn-update" hidden></button>
    </div>
    <div id="print-control"><button id="print-direct"></button><button id="pdf-export"></button></div>
    <div id="word-control"><input id="word-standardize" type="checkbox"><input id="word-strip" type="checkbox"><button id="word-export"></button></div>
    <aside id="toc-panel"><nav id="toc"></nav></aside>
    <div id="tab-menu"><button id="tab-menu-reveal"></button></div>
    <div id="about-popover"></div>
    <div class="findbar">
      <div class="find-row">
        <input id="find-input" type="search">
        <span id="find-state"></span>
        <button id="find-prev"></button>
        <button id="find-next"></button>
        <button id="find-close"></button>
      </div>
      <div class="replace-row">
        <input id="replace-input" type="text">
        <button id="replace-one"></button>
        <button id="replace-all"></button>
      </div>
    </div>
    <div id="app">
      <div id="preview">
        <p>Alpha beta Beta</p>
        <p><code>beta()</code> should be searchable.</p>
        <svg><text>beta in svg should not be rewritten</text></svg>
        <div class="spacer"></div>
        <p>Final beta target</p>
      </div>
      <textarea id="editor">Alpha beta Beta
beta() should be replaceable.
Final beta target
中文替换 中文替换</textarea>
    </div>
    <script>
      window.__messages = [];
      window.ipc = { postMessage(message) { window.__messages.push(message); } };
    </script>
    <script>${desktopScript}</script>
  </body>
</html>`);

await page.locator('#btn-search').click();
await page.locator('#find-input').fill('beta');
await page.waitForFunction(() => document.querySelectorAll('#preview mark.search-hit').length === 4);

let result = await page.evaluate(() => ({
  finding: document.body.classList.contains('finding'),
  activeId: document.activeElement && document.activeElement.id,
  hitCount: document.querySelectorAll('#preview mark.search-hit').length,
  currentCount: document.querySelectorAll('#preview mark.search-hit.current').length,
  state: document.getElementById('find-state').textContent,
  svgRewritten: !!document.querySelector('svg mark.search-hit'),
}));

if (!result.finding ||
    result.activeId !== 'find-input' ||
    result.hitCount !== 4 ||
    result.currentCount !== 1 ||
    result.state !== '1/4' ||
    result.svgRewritten) {
  throw new Error(`initial desktop search failed: ${JSON.stringify(result)}`);
}

await page.locator('#find-input').press('Enter');
result = await page.evaluate(() => ({
  state: document.getElementById('find-state').textContent,
  currentText: document.querySelector('#preview mark.search-hit.current')?.textContent,
}));
if (result.state !== '2/4' || result.currentText !== 'Beta') {
  throw new Error(`desktop search next failed: ${JSON.stringify(result)}`);
}

await page.locator('#find-prev').click();
result = await page.evaluate(() => ({
  state: document.getElementById('find-state').textContent,
  currentText: document.querySelector('#preview mark.search-hit.current')?.textContent,
}));
if (result.state !== '1/4' || result.currentText !== 'beta') {
  throw new Error(`desktop search previous failed: ${JSON.stringify(result)}`);
}

await page.locator('#find-close').click();
result = await page.evaluate(() => ({
  finding: document.body.classList.contains('finding'),
  inputValue: document.getElementById('find-input').value,
  state: document.getElementById('find-state').textContent,
  hitCount: document.querySelectorAll('#preview mark.search-hit').length,
  previewText: document.getElementById('preview').textContent,
}));
if (result.finding ||
    result.inputValue !== '' ||
    result.state !== '' ||
    result.hitCount !== 0 ||
    !result.previewText.includes('Alpha beta Beta')) {
  throw new Error(`desktop search close failed: ${JSON.stringify(result)}`);
}

await page.keyboard.down(process.platform === 'darwin' ? 'Meta' : 'Control');
await page.keyboard.press('F');
await page.keyboard.up(process.platform === 'darwin' ? 'Meta' : 'Control');
await page.waitForFunction(() => document.body.classList.contains('finding'));

await page.locator('#find-close').click();
const replaceApi = await page.evaluate(() => typeof window.__mdPreviewShowReplace);
if (replaceApi !== 'function') {
  throw new Error('desktop replace API is missing');
}

await page.evaluate(() => { window.__messages = []; });
await page.keyboard.press('Control+H');
await page.waitForFunction(() =>
  document.body.classList.contains('finding') &&
  document.body.classList.contains('replacing') &&
  document.body.classList.contains('editing'));
await page.locator('#find-input').fill('beta');
await page.waitForFunction(() => document.getElementById('find-state').textContent === '1/4');

// Editing the source while the replace bar is open must invalidate stale
// offsets before the next replacement.
await page.evaluate(() => {
  const editor = document.getElementById('editor');
  editor.setRangeText('Prefix ', 0, 0, 'end');
  editor.dispatchEvent(new Event('input', { bubbles: true }));
});
await page.waitForFunction(() => document.getElementById('find-state').textContent === '1/4');

result = await page.evaluate(() => ({
  activeId: document.activeElement && document.activeElement.id,
  replaceVisible: getComputedStyle(document.querySelector('.replace-row')).display !== 'none',
  replaceOneDisabled: document.getElementById('replace-one').disabled,
  replaceAllDisabled: document.getElementById('replace-all').disabled,
}));
if (result.activeId !== 'find-input' || !result.replaceVisible ||
    result.replaceOneDisabled || result.replaceAllDisabled) {
  throw new Error(`desktop replace open failed: ${JSON.stringify(result)}`);
}

await page.locator('#replace-input').fill('first $& $1');
await page.locator('#replace-one').click();
await page.waitForFunction(() => document.getElementById('find-state').textContent === '1/3');
result = await page.evaluate(() => ({
  source: document.getElementById('editor').value,
  dirty: window.__messages.includes('dirty:1'),
  state: document.getElementById('find-state').textContent,
  activeId: document.activeElement && document.activeElement.id,
}));
if (!result.source.startsWith('Prefix Alpha first $& $1 Beta') || !result.dirty ||
    result.state !== '1/3' || result.activeId !== 'replace-one') {
  throw new Error(`replace current failed: ${JSON.stringify(result)}`);
}

const allReplacement = 'all $& $1 \\ path';
await page.locator('#replace-input').fill(allReplacement);
await page.locator('#replace-all').click();
await page.waitForFunction(() => document.getElementById('find-state').textContent === '3 replaced');
result = await page.evaluate(() => ({
  source: document.getElementById('editor').value,
  selected: document.getElementById('editor').value.slice(
    document.getElementById('editor').selectionStart,
    document.getElementById('editor').selectionEnd,
  ),
}));
if (result.source.split(allReplacement).length - 1 !== 3 ||
    result.source.toLowerCase().includes('beta') ||
    result.selected !== allReplacement) {
  throw new Error(`replace all literal text failed: ${JSON.stringify(result)}`);
}

// Each replace action should stay in the native textarea undo stack.
await page.locator('#editor').focus();
await page.keyboard.press('Control+z');
await page.waitForFunction(() => document.getElementById('editor').value.toLowerCase().includes('beta'));
await page.keyboard.press('Control+y');
await page.waitForFunction(() => !document.getElementById('editor').value.toLowerCase().includes('beta'));

await page.locator('#find-input').fill('中文替换');
await page.waitForFunction(() => document.getElementById('find-state').textContent === '1/2');
await page.locator('#replace-input').fill('中文完成');
await page.locator('#replace-all').click();
await page.waitForFunction(() => document.getElementById('find-state').textContent === '2 replaced');
result = await page.evaluate(() => document.getElementById('editor').value);
if (result.split('中文完成').length - 1 !== 2 || result.includes('中文替换')) {
  throw new Error(`unicode replace all failed: ${result}`);
}

await page.waitForFunction(() => window.__messages.some((message) => message.startsWith('save:')));
await page.locator('#find-input').fill('not-present');
await page.waitForFunction(() => document.getElementById('find-state').textContent === '0/0');
result = await page.evaluate(() => ({
  replaceOneDisabled: document.getElementById('replace-one').disabled,
  replaceAllDisabled: document.getElementById('replace-all').disabled,
}));
if (!result.replaceOneDisabled || !result.replaceAllDisabled) {
  throw new Error(`replace no-match state failed: ${JSON.stringify(result)}`);
}

await page.locator('#find-input').press('Escape');
result = await page.evaluate(() => ({
  finding: document.body.classList.contains('finding'),
  replacing: document.body.classList.contains('replacing'),
  editing: document.body.classList.contains('editing'),
  findValue: document.getElementById('find-input').value,
  replaceValue: document.getElementById('replace-input').value,
}));
if (result.finding || result.replacing || !result.editing ||
    result.findValue !== '' || result.replaceValue !== '') {
  throw new Error(`desktop replace close failed: ${JSON.stringify(result)}`);
}

// Leaving source mode must also close the replace state, so hidden source text
// cannot still be modified from a visible replace bar.
await page.keyboard.press('Control+H');
await page.waitForFunction(() => document.body.classList.contains('replacing'));
await page.keyboard.press('Control+E');
result = await page.evaluate(() => ({
  finding: document.body.classList.contains('finding'),
  replacing: document.body.classList.contains('replacing'),
  editing: document.body.classList.contains('editing'),
}));
if (result.finding || result.replacing || result.editing) {
  throw new Error(`replace state leaked after leaving edit mode: ${JSON.stringify(result)}`);
}

// Ctrl+H in split view must keep both preview and source visible.
await page.locator('#btn-split').click();
await page.keyboard.press('Control+H');
await page.waitForFunction(() => document.body.classList.contains('replacing'));
result = await page.evaluate(() => ({
  split: document.body.classList.contains('split-view'),
  editing: document.body.classList.contains('editing'),
  previewDisplay: getComputedStyle(document.getElementById('preview')).display,
  editorDisplay: getComputedStyle(document.getElementById('editor')).display,
}));
if (!result.split || result.editing || result.previewDisplay === 'none' || result.editorDisplay === 'none') {
  throw new Error(`replace left split view: ${JSON.stringify(result)}`);
}
await page.locator('#find-close').click();
await page.locator('#btn-split').click();

// The two-row bar must stay within a narrow desktop window.
await page.setViewportSize({ width: 390, height: 844 });
await page.keyboard.press('Control+H');
await page.waitForFunction(() => document.body.classList.contains('replacing'));
result = await page.evaluate(() => {
  const rect = document.querySelector('.findbar').getBoundingClientRect();
  return { left: rect.left, right: rect.right, viewport: innerWidth };
});
if (result.left < 0 || result.right > result.viewport) {
  throw new Error(`replace bar overflowed narrow viewport: ${JSON.stringify(result)}`);
}

// A match near the end of a long document must be selected and scrolled into
// the visible page while focus returns to the find field.
await page.evaluate(() => {
  const editor = document.getElementById('editor');
  editor.value = Array.from({ length: 220 }, (_, index) => `line ${index + 1}`).join('\n') + '\nneedle-tail';
  editor.setSelectionRange(0, 0);
  editor.dispatchEvent(new Event('input', { bubbles: true }));
  scrollTo(0, 0);
});
await page.locator('#find-input').fill('needle-tail');
await page.waitForFunction(() =>
  document.getElementById('find-state').textContent === '1/1' && window.scrollY > 0);
result = await page.evaluate(() => {
  const editor = document.getElementById('editor');
  return {
    selected: editor.value.slice(editor.selectionStart, editor.selectionEnd),
    activeId: document.activeElement && document.activeElement.id,
    scrollY: window.scrollY,
  };
});
if (result.selected !== 'needle-tail' || result.activeId !== 'find-input' || result.scrollY <= 0) {
  throw new Error(`long-document replace navigation failed: ${JSON.stringify(result)}`);
}

// Windows CJK composition must not trigger a partial autosave or let Escape
// leave edit mode while the IME candidate window owns the keystroke.
await page.locator('#find-close').click();
await page.evaluate(() => {
  const editor = document.getElementById('editor');
  window.__markSaved(editor.value);
  window.__setContent('<p>已有内容</p>', '已有内容', '', false, false);
  window.__mdPreviewEnterEdit();
  window.__messages = [];
  editor.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true, data: '' }));
  editor.setRangeText('中文输入', editor.selectionStart, editor.selectionEnd, 'end');
  editor.dispatchEvent(new InputEvent('input', {
    bubbles: true,
    data: '中文输入',
    inputType: 'insertCompositionText',
    isComposing: true,
  }));
  document.dispatchEvent(new KeyboardEvent('keydown', {
    bubbles: true,
    key: 'Escape',
    isComposing: true,
    keyCode: 229,
  }));
});
await page.waitForTimeout(850);
result = await page.evaluate(() => ({
  editing: document.body.classList.contains('editing'),
  source: document.getElementById('editor').value,
  savedMidComposition: window.__messages.some((message) => message.startsWith('save:')),
}));
if (!result.editing || !result.source.includes('中文输入') || result.savedMidComposition) {
  throw new Error(`IME composition lifecycle failed: ${JSON.stringify(result)}`);
}
await page.evaluate(() => {
  const editor = document.getElementById('editor');
  editor.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true, data: '中文输入' }));
});
await page.waitForFunction(() => window.__messages.some((message) => message.startsWith('save:')));
result = await page.evaluate(() => ({
  activeId: document.activeElement && document.activeElement.id,
  save: window.__messages.find((message) => message.startsWith('save:')) || '',
}));
if (result.activeId !== 'editor' || !result.save.includes('中文输入')) {
  throw new Error(`IME composition commit failed: ${JSON.stringify(result)}`);
}

await browser.close();
console.log('[desktop-search-replace-verify] OK');
