import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UpdateController } from '../src/updateFlow.ts';

const policy = (required = true) => ({ currentVersion: '0.2.6', required, source: 'remote', warning: null, config: { min_client_version: required ? '0.3.0' : '' } });
const update = { version: '0.3.0', notes: 'Compatibility fixes' };
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function fixture(overrides = {}) {
  const views = [], calls = [];
  const dependencies = {
    policy: async () => { calls.push('policy'); return policy(); },
    check: async () => { calls.push('check'); return update; },
    install: async progress => { calls.push('install'); progress({ phase: 'installing', downloaded: 100, total: 100 }); },
    busy: () => false, ...overrides,
  };
  const controller = new UpdateController(dependencies, state => views.push(state));
  return { controller, dependencies, calls, views };
}

test('minimum-version gate becomes effective before updater request finishes', async () => {
  const waiting = deferred();
  const f = fixture({ check: () => waiting.promise });
  const run = f.controller.check();
  await Promise.resolve();
  assert.equal(f.controller.state.policy.required, true);
  assert.equal(f.controller.state.checking, true);
  waiting.reject(Error('release unavailable'));
  await run;
  assert.equal(f.controller.state.policy.required, true);
  assert.equal(f.controller.state.error, 'release unavailable');
  assert.equal(f.controller.state.phase, 'idle');
});

test('policy refresh failure preserves previously required upgrade and retry recovers', async () => {
  const f = fixture();
  await f.controller.check();
  f.dependencies.policy = async () => { throw Error('offline'); };
  await f.controller.check();
  assert.equal(f.controller.state.policy.required, true);
  assert.equal(f.controller.state.update, null);
  f.dependencies.policy = async () => policy(false);
  await f.controller.check();
  assert.equal(f.controller.state.policy.required, false);
  assert.equal(f.controller.state.error, null);
  assert.equal(f.controller.state.update.version, '0.3.0');
});

test('failed signature/download never advances to install and can be retried', async () => {
  const f = fixture({ install: async progress => {
    progress({ phase: 'downloading', downloaded: 5, total: null });
    throw Error('signature invalid');
  } });
  await f.controller.check();
  await f.controller.install();
  assert.equal(f.controller.state.phase, 'idle');
  assert.equal(f.controller.state.policy.required, true);
  assert.equal(f.controller.state.error, 'signature invalid');
  assert.ok(!f.views.some(v => v.phase === 'installing' || v.phase === 'restarting'));
  f.dependencies.install = async progress => progress({ phase: 'installing', downloaded: 100, total: 100 });
  await f.controller.install();
  assert.equal(f.controller.state.phase, 'restarting');
  assert.equal(f.controller.state.error, null);
});

test('duplicate checks and installs do not overlap; refresh cannot disrupt an install', async () => {
  const waiting = deferred();
  let checks = 0, installs = 0;
  const f = fixture({ check: async () => { checks++; return waiting.promise; } });
  const run = f.controller.check();
  await f.controller.check();
  await f.controller.install();
  waiting.resolve(update);
  await run;
  assert.equal(checks, 1);
  const installing = deferred();
  f.dependencies.install = async () => { installs++; return installing.promise; };
  const install = f.controller.install();
  await f.controller.install();
  await f.controller.check();
  assert.equal(installs, 1);
  assert.equal(checks, 1);
  installing.resolve();
  await install;
});

test('busy client operations postpone checks and installations', async () => {
  const f = fixture();
  await f.controller.check();
  f.dependencies.busy = () => true;
  await f.controller.install();
  await f.controller.check();
  assert.deepEqual(f.calls, ['policy', 'check']);
  assert.equal(f.controller.state.phase, 'idle');
});

test('no newer package does not dismiss the mandatory gate', async () => {
  const f = fixture({ check: async () => null });
  await f.controller.check();
  await f.controller.install();
  assert.equal(f.controller.state.policy.required, true);
  assert.equal(f.controller.state.checked, true);
  assert.equal(f.controller.state.update, null);
  assert.ok(!f.calls.includes('install'));
});
