import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runTrayAction, TrayConsumer } from '../src/trayFlow.ts';

const preferences = { claudeMode: 'desktop', codexMode: 'cli', claudeService: 'key-a', codexService: 'current', projectDirectory: 'C:\\项目', recentDirectories: ['C:\\other'], customPaths: {} };
function fixture(overrides = {}) {
  const calls = [];
  const deps = {
    consented: () => true,
    showTool: (tool, picker) => calls.push(['show', tool, picker]),
    validateDirectory: async path => calls.push(['validate', path]),
    save: async patch => { calls.push(['save', patch]); return true; },
    launch: async (...args) => { calls.push(['launch', ...args]); return true; },
    ...overrides,
  };
  return { calls, run: action => runTrayAction(action, preferences, deps) };
}
test('tray launch uses saved tool mode, the live configuration and directory through existing checked launch callback', async () => {
  const f = fixture();
  await f.run({ kind: 'launch', tool: 'claude' });
  assert.deepEqual(f.calls, [['show', 'claude', false], ['launch', 'claude', 'desktop', 'current', 'C:\\项目']]);
});
test('CLI tray launch adds history only after successful launch', async () => {
  const f = fixture(); await f.run({ kind: 'launch', tool: 'codex' });
  assert.deepEqual(f.calls[1], ['launch', 'codex', 'cli', 'current', 'C:\\项目']);
  assert.deepEqual(f.calls[2], ['save', { recentDirectories: ['C:\\项目', 'C:\\other'] }]);
  const failed = fixture({ launch: async () => false });
  await failed.run({ kind: 'launch', tool: 'codex' });
  assert.ok(!failed.calls.some(c => c[0] === 'save'));
});
test('tray cannot bypass first-use configuration consent', async () => {
  const f = fixture({ consented: () => false });
  await assert.rejects(f.run({ kind: 'launch', tool: 'claude' }), /先读取/);
  assert.deepEqual(f.calls, [['show', 'claude', false]]);
});
test('switch key opens the matching picker without writing credentials or launching', async () => {
  const f = fixture(); await f.run({ kind: 'switchKey', tool: 'codex' });
  assert.deepEqual(f.calls, [['show', 'codex', true]]);
});
test('recent directory is validated before saving and never implicitly launches', async () => {
  const f = fixture(); await f.run({ kind: 'directory', path: "C:\\新 & '项目'" });
  assert.equal(f.calls[0][0], 'validate'); assert.equal(f.calls[1][0], 'save');
  assert.equal(f.calls[1][1].projectDirectory, "C:\\新 & '项目'");
  const bad = fixture({ validateDirectory: async () => { throw Error('missing'); } });
  await assert.rejects(bad.run({ kind: 'directory', path: 'missing' }), /missing/);
  assert.deepEqual(bad.calls, []);
});
test('failed preference write is reported', async () => {
  const f = fixture({ save: async () => false });
  await assert.rejects(f.run({ kind: 'directory', path: 'C:\\valid' }), /未保存/);
});
test('pending native action survives busy UI and duplicate drain events execute it once', async () => {
  let ready = false, takes = 0, runs = 0, release;
  const queued = [{ kind: 'launch', tool: 'claude' }];
  const gate = new Promise(resolve => { release = resolve; });
  const consumer = new TrayConsumer(() => ready, async () => { takes++; return queued.shift() ?? null; }, async () => { runs++; await gate; }, e => { throw e; });
  await consumer.drain(); assert.equal(takes, 0);
  ready = true;
  const first = consumer.drain(); await Promise.resolve();
  await consumer.drain(); assert.equal(runs, 1);
  release(); await first;
  assert.equal(runs, 1); assert.equal(takes, 2);
});
test('failed tray action releases the consumer for later actions', async () => {
  const queued = [{ kind: 'launch', tool: 'claude' }], errors = [];
  const consumer = new TrayConsumer(() => true, async () => queued.shift() ?? null, async () => { throw Error('not installed'); }, e => errors.push(e.message));
  await consumer.drain(); queued.push({ kind: 'launch', tool: 'codex' }); await consumer.drain();
  assert.deepEqual(errors, ['not installed', 'not installed']);
});
