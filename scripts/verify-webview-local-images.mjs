import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');

const endpoint = process.env.MD_PREVIEW_WEBVIEW_ENDPOINT;
const expected = Number.parseInt(process.env.MD_PREVIEW_EXPECTED_IMAGES || '0', 10);
if (!endpoint || !Number.isInteger(expected) || expected < 1) {
  throw new Error('set MD_PREVIEW_WEBVIEW_ENDPOINT and MD_PREVIEW_EXPECTED_IMAGES');
}

let browser;
let lastError;
for (let attempt = 0; attempt < 40; attempt += 1) {
  try {
    browser = await chromium.connectOverCDP(endpoint);
    break;
  } catch (error) {
    lastError = error;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}
if (!browser) throw lastError;

try {
  let page;
  for (let attempt = 0; attempt < 40 && !page; attempt += 1) {
    const pages = browser.contexts().flatMap((context) => context.pages());
    for (const candidate of pages) {
      if (await candidate.locator('#preview').count()) {
        page = candidate;
        break;
      }
    }
    if (!page) await new Promise((resolve) => setTimeout(resolve, 250));
  }
  if (!page) throw new Error('MD Preview WebView page was not exposed over CDP');

  await page.waitForFunction((count) => {
    const images = [...document.querySelectorAll('#preview img')];
    return images.length === count && images.every((image) =>
      image.complete && image.naturalWidth > 0 && image.naturalHeight > 0);
  }, expected, { timeout: 20_000 });

  const result = await page.locator('#preview img').evaluateAll((images) => ({
    count: images.length,
    transportUrls: images.filter((image) =>
      image.src.startsWith('http://mdpreview-image.localhost/')).length,
    dimensions: images.map((image) => `${image.naturalWidth}x${image.naturalHeight}`),
  }));
  if (result.count !== expected || result.transportUrls !== expected) {
    throw new Error(`unexpected image transport result: ${JSON.stringify(result)}`);
  }

  await page.locator('#btn-split').evaluate((button) => button.click());
  await page.waitForFunction(() => document.body.classList.contains('split-view'));
  const split = await page.evaluate(() => {
    const preview = document.querySelector('#preview').getBoundingClientRect();
    const editor = document.querySelector('#editor').getBoundingClientRect();
    const button = document.querySelector('#btn-split');
    return {
      previewVisible: preview.width > 0 && preview.height > 0,
      editorVisible: editor.width > 0 && editor.height > 0,
      previewLeftOfEditor: preview.right <= editor.left + 1,
      pressed: button.getAttribute('aria-pressed'),
    };
  });
  if (!split.previewVisible || !split.editorVisible ||
      !split.previewLeftOfEditor || split.pressed !== 'true') {
    throw new Error(`unexpected split-view layout: ${JSON.stringify(split)}`);
  }
  await page.locator('#btn-split').evaluate((button) => button.click());
  await page.waitForFunction(() => !document.body.classList.contains('split-view'));

  console.log(`[webview-local-images-verify] OK: ${result.count}/${expected} loaded via scoped Windows transport (${result.dimensions.join(', ')}); split view shows preview left/editor right`);
} finally {
  await browser.close();
}
