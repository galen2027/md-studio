import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtemp, readFile, writeFile, copyFile, mkdir, readdir } from 'node:fs/promises';
import { dirname, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temp = await mkdtemp(resolve(root, 'target/structured-edit-'));
const fixture = resolve(root, 'tests/fixtures/structured-edit.md');
const inputs = [fixture, ...process.argv.slice(2)];
const paths = [];
const originals = [];
for (const [index, path] of inputs.entries()) {
  const directory = resolve(temp, 'docs', String(index));
  await mkdir(directory, { recursive: true });
  const target = resolve(directory, basename(path));
  const source = await readFile(path, 'utf8');
  originals.push(source);
  await copyFile(path, target);
  paths.push(target);
  for (const image of source.matchAll(/!\[[^\]]*\]\(([^)]+)\)/g)) {
    const relative = decodeURIComponent(image[1]).replace(/\\/g, '/');
    if (/^(https?:|data:|file:)/i.test(relative)) continue;
    const destination = resolve(directory, relative);
    if (!destination.startsWith(directory + '\\') && !destination.startsWith(directory + '/')) continue;
    await mkdir(dirname(destination), { recursive: true });
    if (index === 0) {
      await writeFile(destination, Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6oAAAAABJRU5ErkJggg==', 'base64'));
    } else {
      await copyFile(resolve(dirname(path), relative), destination);
    }
  }
}
const second = resolve(temp, 'other.md');
await writeFile(second, '# Other\n\n标签切换目标\n');
const server = createServer();
await new Promise(done => server.listen(0, '127.0.0.1', done));
const port = server.address().port;
await new Promise(done => server.close(done));
const child = spawn(resolve(root, 'target/debug/md-preview.exe'), [...paths, second], {
  cwd: root, windowsHide: true, stdio: 'ignore', env: { ...process.env,
    MD_PREVIEW_CONFIG_DIR: resolve(temp, 'config'), MD_PREVIEW_TEST_SKIP_REGISTRATION: '1',
    MD_PREVIEW_TEST_DOWNLOAD_DIR: temp,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
});
let browser;
try {
  let lastError;
  for (let i = 0; i < 60; i++) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); break; }
    catch (error) { lastError = error; await new Promise(done => setTimeout(done, 250)); }
  }
  if (!browser) throw lastError;
  let page;
  for (let i = 0; i < 50; i++) {
    page = browser.contexts().flatMap(context => context.pages())[0];
    if (page && await page.locator('.tab').count() === paths.length + 1) break;
    await new Promise(done => setTimeout(done, 200));
  }
  page.setDefaultTimeout(20000);
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.log('[structured-edit] page error '+error.stack); });
  await page.evaluate(()=>{
    window.__richTrace=[];
    for(const name of ['__setContent','__markSaved','__mdPreviewResolveExternalChange']) {
      const original=window[name];
      window[name]=function(...args) {
        window.__richTrace.push({name,rich:document.body.classList.contains('preview-editing'),raw:document.querySelector('#editor').value.slice(-60)});
        return original(...args);
      };
    }
  });
  const requests = [];
  page.on('request', request => {
    if (/^https?:/.test(request.url()) && !request.url().includes('.localhost/') && !request.url().startsWith('https://api.github.com/repos/galen2027/md-studio/releases')) requests.push(request.url());
  });
  async function downloadFile(extension) {
    let error;
    for (let attempt=0;attempt<70;attempt++) {
      try { return await readFile(resolve(temp,'diagram.'+extension)); }
      catch(err) { error=err; await new Promise(done=>setTimeout(done,100)); }
    }
    throw error;
  }
  async function replaceText(selector, marker, last = false, save = true) {
    const candidates = page.locator(selector).filter({ hasText: /\S/ });
    const element = last ? candidates.last() : candidates.first();
    await element.click();
    await page.waitForTimeout(100);
    const selected = await element.evaluate(element => {
      if (!element.isContentEditable) return false;
      element.closest('[contenteditable="true"]').focus();
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      let node;
      while ((node = walker.nextNode())) {
        if (!node.nodeValue.trim() || node.parentElement.closest('[contenteditable="false"]')) continue;
        const range = document.createRange();
        range.setStart(node, 0); range.setEnd(node, Math.min(2, node.length));
        const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
        return true;
      }
      return false;
    });
    assert.equal(selected, true, selector);
    await page.keyboard.insertText(marker);
    if (!save) return;
    await page.waitForFunction(({selector,marker,last})=>{
      const nodes=[...document.querySelectorAll(selector)].filter(node=>node.textContent.trim());
      return (last ? nodes.at(-1) : nodes[0])?.textContent.includes(marker);
    },{selector,marker,last});
    await page.keyboard.press('Control+s');
    await page.waitForFunction(marker => document.getElementById('editor').value.includes(marker), marker);
  }
  for (const [index, path] of paths.entries()) {
    const name = basename(path);
    await page.locator('.tab-name').getByText(name, { exact: true }).click();
    await page.waitForFunction(name => document.querySelector('.tab.active .tab-name')?.textContent === name, name);
    await page.locator('#btn-preview-edit').evaluate(button => button.click());
    await page.waitForSelector('body.preview-editing', { timeout: 20000 });
    const before = originals[index].replace(/\r\n?/g, '\n');
    assert.equal(await page.locator('#editor').inputValue(), before, 'entering editor must not rewrite source');
    const edit = '#rich-editor .vditor-wysiwyg [contenteditable="true"]';
    const coverage = await page.locator(edit).evaluate(element => ({
      headings: [...element.querySelectorAll('h1,h2,h3,h4,h5,h6')].filter(el=>el.isContentEditable).length,
      paragraphs: [...element.querySelectorAll(':scope > p')].filter(el=>el.isContentEditable).length,
      lists: [...element.querySelectorAll('li')].filter(el=>el.isContentEditable).length,
      cells: [...element.querySelectorAll('th,td')].filter(el=>el.isContentEditable).length,
    }));
    console.log('[structured-edit] coverage ' + name + ': ' + JSON.stringify(coverage));
    await replaceText(edit + ' h1', '标题已修改');
    await replaceText(edit + ' > p strong', '粗体已修改');
    await replaceText(edit + ' > p', '末段已修改', true);
    await replaceText(edit + ' li', '列表已修改');
    const hasBoldCell = await page.locator(edit + ' td strong').count() > 0;
    await replaceText(edit + (hasBoldCell ? ' td strong' : ' td'), '表格已修改');
    await page.waitForTimeout(950);
    let saved = await readFile(path, 'utf8');
    for (const marker of ['标题已修改', '粗体已修改', '末段已修改', '列表已修改', '表格已修改']) assert.ok(saved.includes(marker), marker);
    assert.ok(saved.includes('**粗体已修改'));
    if (index === 0) {
      assert.ok(saved.includes('$x_1 + y^2$'));
      assert.ok(saved.includes('E = mc^2'));
    }
    if (hasBoldCell) assert.ok(saved.includes('**表格已修改'));
    const diagrams = source => [...source.matchAll(/^```mermaid\r?\n([\s\S]*?)^```/gm)].map(match=>match[1].replace(/\r\n/g,'\n'));
    assert.deepEqual(diagrams(saved), diagrams(originals[index]));
    assert.ok(!saved.includes('mdpreview-image.localhost'));
    await page.waitForFunction(() => [...document.querySelectorAll('#rich-editor img')].every(image=>image.complete && image.naturalWidth>0));
    const backups = await readdir(resolve(temp, 'config/backups'));
    assert.ok(backups.length >= index + 1);
    const backup = backups.find(file => file.endsWith(name + '.bak'));
    assert.equal(await readFile(resolve(temp, 'config/backups', backup), 'utf8'), originals[index]);
    await page.keyboard.press('Control+f');
    await page.locator('#find-input').fill('修改');
    await page.waitForFunction(() => document.getElementById('find-state').textContent.includes('/'));
    assert.equal(await page.locator('body').evaluate(el=>el.classList.contains('preview-editing')), true);
    await page.locator('#find-close').evaluate(button=>button.click());
    if (index === 0) {
      await replaceText(edit + ' > p a', '链接已修改');
      await replaceText(edit + ' > p code:not([data-type])', '代码已修改');
      await replaceText(edit + ' blockquote strong', '引用已修改');
      await page.waitForTimeout(950);
      saved = await readFile(path, 'utf8');
      assert.ok(saved.includes('[链接已修改'));
      assert.ok(saved.includes('](other.md)'));
      assert.ok(saved.includes('`代码已修改'));
      assert.ok(saved.includes('**引用已修改'));
      await page.locator(edit + ' > p').filter({hasText:'中间的纯文字段落'}).click();
      await page.keyboard.press('End');
      await page.keyboard.press('Enter');
      await page.keyboard.type('New paragraph ',{delay:15});
      await page.keyboard.press('Control+b');
      await page.keyboard.type('New bold',{delay:15});
      await page.keyboard.press('Control+b');
      await page.keyboard.press('Control+s');
      await page.waitForTimeout(350);
      assert.ok((await readFile(path,'utf8')).includes('**New bold**'));
      const emptyCell = page.locator(edit + ' tr').filter({hasText:'空白'}).locator('td').nth(1);
      await emptyCell.click();
      await page.keyboard.insertText('填入空白格');
      await page.keyboard.press('Control+s');
      await page.waitForTimeout(350);
      assert.ok((await readFile(path,'utf8')).includes('填入空白格'));
      await page.keyboard.press('Control+z');
      await page.waitForTimeout(350);
      assert.ok(!(await page.locator('#editor').inputValue()).includes('填入空白格'));
      await page.keyboard.press('Control+y');
      await page.waitForTimeout(350);
      assert.ok((await page.locator('#editor').inputValue()).includes('填入空白格'));
      const editorElement = page.locator(edit);
      await editorElement.dispatchEvent('compositionstart');
      const rawBeforeIme = await page.locator('#editor').inputValue();
      await page.keyboard.insertText('中文组合输入');
      await page.waitForTimeout(1000);
      assert.equal(await page.locator('#editor').inputValue(), rawBeforeIme);
      await editorElement.dispatchEvent('compositionend');
      await page.waitForTimeout(950);
      assert.ok((await readFile(path,'utf8')).includes('中文组合输入'));
      await page.waitForFunction(() => [...document.querySelectorAll('#rich-editor img')].every(img=>img.complete && img.naturalWidth>0));
      for (let i=0;i<10;i++) {
        await emptyCell.click();
        await page.keyboard.insertText('图');
        await page.keyboard.press('Control+s');
        await page.waitForTimeout(100);
        assert.equal(await page.locator('body').evaluate(el=>el.classList.contains('preview-editing')),true,
          JSON.stringify(await page.evaluate(()=>({hint:document.querySelector('#preview-edit-hint').textContent,trace:window.__richTrace.slice(-12)}))));
      }
      saved = await readFile(path,'utf8');
      assert.ok(saved.includes('![本地图片](images/pixel.png)'));
      assert.ok(!saved.includes('mdpreview-image'));
      await page.waitForSelector('#rich-editor .mdp-mermaid svg');
      assert.equal(await page.evaluate(()=>window.mermaid.mermaidAPI.getConfig().securityLevel),'strict');
      await page.locator('#rich-editor .mdp-mermaid').first().click();
      await page.waitForSelector('.mdp-lightbox.open');
      await page.click('[data-lb="svg"]');
      const svgFile = (await downloadFile('svg')).toString('utf8');
      await page.click('[data-lb="png"]');
      const pngFile = await downloadFile('png');
      assert.equal(pngFile.toString('ascii',1,4),'PNG');
      const svgWidth = Number(svgFile.match(/\bwidth="(\d+)"/)[1]);
      assert.ok(pngFile.readUInt32BE(16) >= svgWidth*2.9);
      console.log('[structured-edit] native downloads: SVG '+svgWidth+'px, PNG '+pngFile.readUInt32BE(16)+'px');
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('body').evaluate(el=>el.classList.contains('preview-editing')),true);
      await page.evaluate(()=>scrollTo(0,0));
      await page.screenshot({path:resolve(temp,'desktop.png'),fullPage:false});
      await page.setViewportSize({width:390,height:844});
      await page.evaluate(()=>scrollTo(0,0));
      await page.screenshot({path:resolve(temp,'narrow.png'),fullPage:false});
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await page.setViewportSize({width:1100,height:800});
    }
    // New input immediately followed by tab activation must be flushed.
    await replaceText(edit + ' h1', '切换前修改', false, false);
    await page.locator('.tab-name').getByText('other.md',{exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('#preview').textContent.includes('标签切换目标'));
    assert.ok((await readFile(path,'utf8')).includes('切换前修改'));
    await page.locator('.tab-name').getByText(name,{exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('#preview').textContent.includes('切换前修改'));
    assert.equal(await page.locator('body').evaluate(el=>el.classList.contains('preview-editing')),false);
    console.log('[structured-edit] saved, searched, switched and reopened: '+name);
  }
  assert.deepEqual(errors,[]);
  assert.deepEqual(requests,[]);
  for (const [index,path] of inputs.entries()) assert.equal(await readFile(path,'utf8'),originals[index]);
  console.log('[structured-edit] OK; originals unchanged; screenshots and backups: '+temp);
} finally {
  if (browser) await browser.close();
  if (child.exitCode === null) child.kill();
}
