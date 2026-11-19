import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { buildManifest } from '../scripts/build-updater-manifest.mjs';

function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zai-update-manifest-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const files = ['StarNexus-Key-Tool_Windows_x64-setup.exe', 'StarNexus-Key-Tool_Windows_x64.msi', 'StarNexus-Key-Tool_macOS.app.tar.gz'];
  for (const name of files) {
    fs.writeFileSync(path.join(dir, name), 'fixture');
    // Manifest assembly only; real cryptographic verification runs in the Rust tests and release job.
    fs.writeFileSync(path.join(dir, `${name}.sig`), Buffer.from('untrusted comment: fixture').toString('base64'));
  }
  return { dir, files };
}

test('manifest covers Windows install types and both Mac architectures with immutable URLs', t => {
  const { dir } = fixture(t);
  const manifest = buildManifest('0.3.0', dir, '2026-11-14T00:00:00Z');
  assert.deepEqual(Object.keys(manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64', 'windows-x86_64', 'windows-x86_64-msi', 'windows-x86_64-nsis']);
  assert.equal(manifest.pub_date, '2026-11-14T00:00:00Z');
  for (const entry of Object.values(manifest.platforms)) {
    assert.match(entry.url, /^https:\/\/github.com\/z55183980-gif\/starnexus-keytool\/releases\/download\/v0\.3\.0\//);
    assert.ok(entry.signature);
  }
});

test('missing platform or signature blocks manifest publication', t => {
  const { dir, files } = fixture(t);
  fs.unlinkSync(path.join(dir, files[2]));
  assert.throws(() => buildManifest('0.3.0', dir));
  fs.writeFileSync(path.join(dir, files[2]), 'fixture');
  fs.unlinkSync(path.join(dir, `${files[0]}.sig`));
  assert.throws(() => buildManifest('0.3.0', dir));
});

test('empty artifacts, corrupt signatures and unsafe versions cannot be published', t => {
  const { dir, files } = fixture(t);
  assert.throws(() => buildManifest('../wrong', dir), /Invalid release version/);
  fs.writeFileSync(path.join(dir, `${files[0]}.sig`), 'garbage');
  assert.throws(() => buildManifest('0.3.0', dir), /Invalid signature/);
  fs.writeFileSync(path.join(dir, files[0]), '');
  assert.throws(() => buildManifest('0.3.0', dir), /Empty update artifact/);
});
