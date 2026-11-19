import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { platform, tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temp = await mkdtemp(resolve(tmpdir(), 'md-preview-mermaid-'));
const pagePath = resolve(temp, 'page.html');

const dump = spawnSync('cargo', ['test', 'dump_mermaid_page_when_requested'], {
  cwd: root,
  env: { ...process.env, MD_PREVIEW_DUMP_PAGE_MERMAID: pagePath },
  encoding: 'utf8',
});
if (dump.status !== 0) {
  throw new Error(`could not build Mermaid verification page:\n${dump.stdout}\n${dump.stderr}`);
}
await readFile(pagePath, 'utf8');

const launchOptions = {};
if (process.env.MD_PREVIEW_TEST_BROWSER) {
  launchOptions.executablePath = process.env.MD_PREVIEW_TEST_BROWSER;
} else if (platform() === 'win32') {
  launchOptions.executablePath = [
    'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  ].find(existsSync);
}
const browser = await chromium.launch(launchOptions);
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 760 } });
  await page.addInitScript(() => {
    window.__ipcMessages = [];
    window.ipc = { postMessage: (message) => window.__ipcMessages.push(message) };
  });
  await page.goto(pathToFileURL(pagePath).href);

  const wordResult = await page.evaluate(async () => {
    window.mermaid = {
      initialize() {},
      render() {
        return Promise.resolve({
          svg: '<svg xmlns="http://www.w3.org/2000/svg" width="100%" viewBox="0 0 1000 500"><rect width="1000" height="500" fill="#fff"/><text x="30" y="80">清晰流程图</text></svg>',
        });
      },
    };
    const markdown = await window.__mdPreviewExportMermaidToImages(
      '```mermaid\ngraph LR\n  A --> B\n```',
    );
    const url = markdown.match(/\((data:image\/png;base64,[^ ]+) /)?.[1] || '';
    const logical = markdown.match(/mdp-logical-size=([0-9]+x[0-9]+)/)?.[1] || '';
    const prepared = window.__mdPreviewPrepareSvgForExport(
      '<svg width="100%" viewBox="0 0 1000 500"><rect width="1000" height="500"/></svg>',
      3125,
      1563,
    );
    const preparedNode = new DOMParser().parseFromString(prepared, 'image/svg+xml').documentElement;
    const dimensions = await new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
      image.onerror = () => reject(new Error('Word fallback PNG could not be decoded'));
      image.src = url;
    });
    return {
      markdown,
      logical,
      dimensions,
      preparedWidth: preparedNode.getAttribute('width'),
      preparedHeight: preparedNode.getAttribute('height'),
    };
  });

  if (wordResult.logical !== '1000x500') {
    throw new Error(`Word export lost logical SVG size: ${wordResult.logical}`);
  }
  if (wordResult.dimensions.width !== 3125 || wordResult.dimensions.height !== 1563) {
    throw new Error(`Word fallback is not 300 PPI: ${JSON.stringify(wordResult.dimensions)}`);
  }
  if (wordResult.preparedWidth !== '3125' || wordResult.preparedHeight !== '1563') {
    throw new Error(`raster source has wrong intrinsic size: ${wordResult.preparedWidth}x${wordResult.preparedHeight}`);
  }

  if (process.env.MD_PREVIEW_SAMPLE_DOCUMENT) {
    const sampleMarkdown = await readFile(process.env.MD_PREVIEW_SAMPLE_DOCUMENT, 'utf8');
    const expected = (sampleMarkdown.match(/(^|\n)```mermaid[ \t]*\r?\n/g) || []).length;
    await page.addScriptTag({ path: resolve(root, 'assets/mermaid/mermaid.min.js') });
    page.setDefaultTimeout(120000);
    const sampleResult = await page.evaluate(async (markdown) => {
      const output = await window.__mdPreviewExportMermaidToImages(markdown);
      const logicalSizes = Array.from(
        output.matchAll(/mdp-logical-size=([0-9]+)x([0-9]+)/g),
        (match) => [Number(match[1]), Number(match[2])],
      );
      return {
        logicalSizes,
        pngCount: (output.match(/data:image\/png;base64,/g) || []).length,
        remainingFences: (output.match(/(^|\n)```mermaid[ \t]*\r?\n/g) || []).length,
      };
    }, sampleMarkdown);
    if (!expected || sampleResult.pngCount !== expected ||
        sampleResult.logicalSizes.length !== expected || sampleResult.remainingFences !== 0 ||
        sampleResult.logicalSizes.some(([width, height]) => width < 1 || height < 1)) {
      throw new Error(`sample Mermaid export incomplete: expected=${expected}, result=${JSON.stringify(sampleResult)}`);
    }
    console.log(`[mermaid-export-verify] sample: ${expected}/${expected} diagrams rasterized at 300 PPI`);
  }

  await page.evaluate(() => {
    const preview = document.getElementById('preview');
    preview.innerHTML = '<div id="diagram" class="mdp-mermaid"><svg id="source-svg" width="100%" viewBox="0 0 1000 500"><rect width="1000" height="500" fill="#fff"/><text x="30" y="80">矢量原图</text></svg></div>';
    window.__enhancePreview();
  });
  await page.click('#diagram');
  await page.waitForSelector('.mdp-lightbox.open');
  await page.click('[data-lb="in"]');
  await page.click('[data-lb="in"]');
  await page.click('[data-lb="in"]');
  const lightboxBody = page.locator('.mdp-lightbox-body');
  const lightboxBounds = await lightboxBody.boundingBox();
  if (!lightboxBounds) throw new Error('lightbox body has no bounds');
  const panStartX = lightboxBounds.x + lightboxBounds.width * 0.7;
  const panStartY = lightboxBounds.y + lightboxBounds.height * 0.7;
  await page.keyboard.down('Space');
  await page.mouse.move(panStartX, panStartY);
  await page.mouse.down();
  await page.mouse.move(panStartX - 180, panStartY - 120, { steps: 6 });
  await page.mouse.up();
  await page.keyboard.up('Space');
  const panResult = await lightboxBody.evaluate((body) => ({
    left: body.scrollLeft,
    top: body.scrollTop,
    open: document.querySelector('.mdp-lightbox')?.classList.contains('open'),
    panning: body.classList.contains('panning'),
    hint: document.querySelector('.mdp-lightbox-pan-hint')?.textContent || '',
  }));
  if (panResult.left <= 0 || panResult.top <= 0 || !panResult.open || panResult.panning ||
      !panResult.hint.includes('空格')) {
    throw new Error(`Space + drag panning failed: ${JSON.stringify(panResult)}`);
  }
  await page.click('[data-lb="svg"]');
  await page.click('[data-lb="png"]');
  await page.waitForFunction(() => window.__ipcMessages.some((message) => message.startsWith('download-png\n')));

  const lightboxResult = await page.evaluate(async () => {
    const svgMessage = window.__ipcMessages.find((message) => message.startsWith('download-svg\n')) || '';
    const pngMessage = window.__ipcMessages.find((message) => message.startsWith('download-png\n')) || '';
    const svg = svgMessage.slice('download-svg\n'.length);
    const svgNode = new DOMParser().parseFromString(svg, 'image/svg+xml').documentElement;
    const dimensions = await new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
      image.onerror = () => reject(new Error('lightbox PNG could not be decoded'));
      image.src = pngMessage.slice('download-png\n'.length);
    });
    return {
      dimensions,
      svgWidth: svgNode.getAttribute('width'),
      svgHeight: svgNode.getAttribute('height'),
      viewBox: svgNode.getAttribute('viewBox'),
      hasVectorRect: Boolean(svgNode.querySelector('rect')),
      hasEmbeddedPng: svg.includes('data:image/png'),
      svgLabel: document.querySelector('[data-lb="svg"]')?.textContent || '',
      pngLabel: document.querySelector('[data-lb="png"]')?.textContent || '',
    };
  });

  if (lightboxResult.svgWidth !== '1000' || lightboxResult.svgHeight !== '500' ||
      lightboxResult.viewBox !== '0 0 1000 500' || !lightboxResult.hasVectorRect ||
      lightboxResult.hasEmbeddedPng) {
    throw new Error(`SVG export is not the natural vector original: ${JSON.stringify(lightboxResult)}`);
  }
  if (lightboxResult.dimensions.width !== 3000 || lightboxResult.dimensions.height !== 1500) {
    throw new Error(`lightbox PNG is not 3x: ${JSON.stringify(lightboxResult.dimensions)}`);
  }
  if (!lightboxResult.svgLabel.includes('矢量原图') || !lightboxResult.pngLabel.includes('3×')) {
    throw new Error(`export format labels are ambiguous: ${JSON.stringify(lightboxResult)}`);
  }
} finally {
  await browser.close();
  await rm(temp, { recursive: true, force: true });
}

console.log('[mermaid-export-verify] OK: vector SVG, 3x PNG, 300-PPI Word fallback');
