import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { BRAND } from '../src/brand.ts';

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

test('0.1.2 upgrades retain the StarNexus identity, callback, keyring and data directories', () => {
  const config = JSON.parse(read('src-tauri/tauri.conf.json'));
  assert.equal(config.identifier, 'com.dkby.starnexus.keytool');
  assert.equal(config.productName, 'StarNexus Key Tool');
  assert.equal(config.mainBinaryName, 'starnexus-keytool');
  assert.deepEqual(config.plugins['deep-link'].desktop.schemes, [config.identifier]);
  assert.ok(read('src-tauri/src/state.rs').includes(`KEYRING_SERVICE: &str = "${config.identifier}"`));
  const paths = read('src-tauri/src/paths.rs');
  for (const name of ['.starnexus-keytool', '.starnexus-keytool-backups', '.starnexus-keytool-ownership.json']) assert.ok(paths.includes(`"${name}"`));
  assert.equal(BRAND.apiBaseUrl, 'https://gateway.dkby.com/v1');
});

test('production builds do not inherit another product’s signing key or diagnostic IPC', () => {
  assert.equal(JSON.parse(read('src-tauri/tauri.conf.json')).plugins.updater.pubkey, '');
  assert.ok(read('src-tauri/src/updates.rs').includes('option_env!("STARNEXUS_UPDATER_PUBLIC_KEY")'));
  assert.ok(!read('src-tauri/src/lib.rs').includes('ai_diagnose'));
  assert.ok(read('src-tauri/src/brand.rs').includes('/starnexus-keytool/main/updates/config.json'));
});
