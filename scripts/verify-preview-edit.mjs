import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtemp, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temp = await mkdtemp(resolve(root, 'target/rich-browser-'));
const pagePath = resolve(temp, 'page.html');
const dump = spawnSync('cargo', ['test', 'dump_preview_edit_page_when_requested'], {
  cwd: root, env: { ...process.env, MD_PREVIEW_DUMP_PAGE_EDIT: pagePath }, encoding: 'utf8',
});
assert.equal(dump.status, 0, dump.stdout + dump.stderr);
const source = (await readFile(resolve(root, 'tests/fixtures/structured-edit.md'), 'utf8')).replace(/\r\n/g, '\n');
const browser = await chromium.launch(process.env.MD_PREVIEW_TEST_BROWSER ? {
  executablePath: process.env.MD_PREVIEW_TEST_BROWSER,
} : {});
try {
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
  const errors = [], remote = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('http://mdpreview-editor.localhost/**', async route => {
    const path = new URL(route.request().url()).pathname.slice(1);
    let local = resolve(root, 'assets/editor/vditor', path);
    if (path.includes('/mermaid/')) local = resolve(root, 'assets/mermaid/mermaid.min.js');
    if (path.endsWith('katex.min.js')) local = resolve(root, 'assets/katex/katex.min.js');
    if (path.endsWith('katex.min.css')) local = resolve(root, 'assets/katex/katex.inline.css');
    if (path.includes('/highlight.js/styles/')) local = resolve(root, 'assets/hljs', path.split('/').at(-1));
    const body = await readFile(local);
    await route.fulfill({ body, contentType: path.endsWith('.css') ? 'text/css' : 'text/javascript',
      headers: { 'Access-Control-Allow-Origin': '*' } });
  });
  page.on('request', request => {
    if (/^https?:/.test(request.url()) && !request.url().includes('.localhost/') && !request.url().startsWith('https://api.github.com/repos/vorojar/md-preview/releases')) remote.push(request.url());
  });
  await page.addInitScript(() => {
    window.__messages = []; window.__backupFail = false;
    window.ipc = { postMessage: value => {
      window.__messages.push(value);
      if (value.startsWith('prepare-rich-edit:')) {
        const token = Number(value.split(':')[1]);
        setTimeout(() => window.__mdPreviewRichEditPrepared(token, !window.__backupFail,
          window.__backupFail ? 'backup denied' : 'test backup'), 0);
      }
    } };
  });
  await page.goto(pathToFileURL(pagePath).href);
  assert.equal(await page.evaluate(() => typeof window.Vditor), 'undefined', 'lazy editor');
  await page.evaluate(() => { window.__backupFail = true; });
  await page.locator('#btn-preview-edit').evaluate(button => button.click());
  await page.waitForSelector('body.rich-edit-error');
  assert.equal(await page.locator('#editor').inputValue(), source);
  assert.equal(await page.evaluate(() => typeof window.Vditor), 'undefined', 'backup first');
  await page.evaluate(() => { window.__backupFail = false; });
  await page.locator('#btn-preview-edit').evaluate(button => button.click());
  await page.waitForSelector('body.preview-editing');
  const edit = '#rich-editor .vditor-wysiwyg [contenteditable="true"]';
  await page.waitForTimeout(1000);
  assert.equal(await page.locator('#editor').inputValue(), source, 'opening does not rewrite');
  for (const selector of ['h1', 'p strong', 'p a', 'p code:not([data-type])', 'li', 'blockquote strong', 'td strong']) {
    const element = page.locator(edit + ' ' + selector).first();
    await element.click();
    await element.evaluate(element => {
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      const node = walker.nextNode();
      const range = document.createRange(); range.setStart(node, 0); range.setEnd(node, 2);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
    });
    await page.keyboard.insertText('修改');
    await page.keyboard.press('Control+s');
    await page.waitForTimeout(160);
    assert.ok(await element.innerText().then(text => text.includes('修改')), selector);
  }
  let raw = await page.locator('#editor').inputValue();
  const paragraph=page.locator(edit + ' > p').filter({hasText:'中间的纯文字段落'});
  await paragraph.click();
  await page.keyboard.press('End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('New paragraph ', {delay:15});
  // Reproduce a stale toolbar highlight after leaving a bold table cell.
  await page.locator('#rich-editor button[data-type="bold"]').evaluate(button=>button.classList.add('vditor-menu--current'));
  await page.keyboard.press('Control+b');
  const boldState = await page.evaluate(()=>({
    command:document.queryCommandState('bold'),selection:getSelection().anchorNode?.parentElement.outerHTML,
    button:document.querySelector('#rich-editor button[data-type="bold"]').className,
    active:document.activeElement.className
  }));
  await page.keyboard.type('New bold', {delay:15});
  await page.keyboard.press('Control+b');
  await page.keyboard.press('Control+s');
  await page.waitForFunction(()=>document.getElementById('editor').value.includes('**New bold**'),null,{timeout:2000}).catch(async error=>{
    console.log('[structured-browser] format diagnostic '+JSON.stringify(boldState)+'\n'+await page.locator('#editor').inputValue());
    throw error;
  });
  assert.ok(await page.locator(edit + ' p').filter({hasText:'New paragraph'}).count());
  raw = await page.locator('#editor').inputValue();
  assert.ok(raw.startsWith('---\ntitle: 结构化编辑验证\ncustom: 保留元数据\n---'));
  assert.ok(raw.includes('**修改'));
  assert.ok(raw.includes('[修改'));
  assert.ok(raw.includes('](other.md)'));
  assert.ok(raw.includes('`修改'));
  assert.ok(raw.includes('flowchart LR\n  A[开始] --> B[验证文字编辑]'));
  await page.keyboard.press('Control+f');
  await page.locator('#find-input').fill('修改');
  await page.waitForFunction(() => document.getElementById('find-state').textContent.includes('/'));
  assert.equal(await page.locator('body').evaluate(body => body.classList.contains('preview-editing')), true);
  assert.equal(await page.locator('#rich-editor mark.search-hit').count(), 0);
  await page.locator('#find-close').evaluate(button => button.click());
  await page.locator(edit + ' > p').first().click();
  await page.keyboard.insertText('并发保存输入');
  const connected = await page.evaluate(raw => {
    const element = document.querySelector('#rich-editor [contenteditable="true"]');
    window.__setPreview('<p>obsolete</p>', false, false, 'stale raw');
    window.__markSaved(raw);
    return element.isConnected && document.body.classList.contains('preview-editing');
  }, raw);
  assert.equal(connected, true);
  assert.ok((await page.locator('#editor').inputValue()).includes('并发保存输入'));
  assert.equal(await page.evaluate(()=>window.__messages.at(-1)), 'dirty:1');
  await page.evaluate(() => scrollTo(0, 0));
  await page.screenshot({path: resolve(temp,'desktop.png'), fullPage: false});
  await page.setViewportSize({width:390,height:844});
  await page.evaluate(() => scrollTo(0, 0));
  await page.screenshot({path: resolve(temp,'narrow.png'), fullPage: false});
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), JSON.stringify(await page.evaluate(()=>[...document.querySelectorAll('body *')].filter(el=>el.getBoundingClientRect().right>innerWidth+1).slice(0,12).map(el=>({tag:el.tagName,cls:el.className,right:el.getBoundingClientRect().right,style:el.getAttribute('style')})))));
  await page.locator('#print-direct').evaluate(button=>button.click());
  const renderMessage=await page.evaluate(()=>window.__messages.findLast(value=>value.startsWith('render-source:')));
  assert.ok(renderMessage.includes('并发保存输入'));
  assert.equal(await page.evaluate(()=>window.__messages.includes('print')), false);
  await page.evaluate(message=>{
    const token=Number(message.slice('render-source:'.length).split('\n')[0]);
    const raw=message.slice(message.indexOf('\n')+1);
    window.__mdPreviewSourceRendered(token,'<h1 id="fresh">并发保存输入</h1>',raw,false,false);
  },renderMessage);
  await page.waitForFunction(()=>window.__messages.includes('print'));
  assert.equal(await page.locator('#preview h1').innerText(),'并发保存输入');
  await page.locator('#btn-toggle').evaluate(button => button.click());
  assert.equal(await page.locator('#editor').isVisible(), true);
  assert.equal(await page.locator('#rich-editor').isVisible(), false);
  assert.deepEqual(errors, []);
  assert.deepEqual(remote, []);
  console.log('[structured-browser-verify] OK: lazy/offline load, failed-backup guard, full rich editing, save, search without DOM rewrites, stale ACK, source mode, narrow layout');
  console.log('[structured-browser-verify] screenshots: ' + temp);
} finally {
  await browser.close();
}
