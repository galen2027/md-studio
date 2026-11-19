import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
const base = new URL('../assets/editor/', import.meta.url);
const manifest = JSON.parse(await readFile(new URL('manifest.json', base), 'utf8'));
assert.equal(manifest.version, '4.0.1');
for (const asset of manifest.assets) {
  const bytes = await readFile(new URL('vditor/' + asset.path, base));
  assert.equal(bytes.length, asset.bytes, asset.path + ' size');
  assert.equal(createHash('sha256').update(bytes).digest('hex'), asset.sha256, asset.path + ' hash');
}
console.log('[editor-assets] OK: pinned Vditor ' + manifest.version + ', ' + manifest.assets.length + ' verified offline files');
