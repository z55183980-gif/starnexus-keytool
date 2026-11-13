import { test } from 'node:test';
import assert from 'node:assert/strict';
import { launchWithConfiguration } from '../src/launchFlow.ts';

const request = { profileId:'key-1', baseUrl:'https://gateway.dkby.com/v1', model:'model', reasoningEffort:'medium', claudeModel:'claude-model' };
const config = (status = 'official', id = null, currentModel = 'model') => ({ configurationStatus:status, activeProfileId:id, currentModel, currentReasoningEffort:'medium' });
const environment = value => ({ codex:value, claude:value, claudeDesktop:value });
function fixture(states = [environment(config())]) {
  const calls = [], configured = [];
  const deps = {
    validateLaunch: async () => { calls.push('validate'); },
    scanEnvironment: async () => { calls.push('scan'); return states.length > 1 ? states.shift() : states[0]; },
    applyConfig: async () => { calls.push('apply'); return { backupId:'backup-1' }; },
    revertToOfficial: async () => { calls.push('revert'); return 'backup-2'; },
    launchClient: async () => { calls.push('launch'); return { message:'requested' }; },
  };
  return { calls, deps, configured, run:(service='current', mode='cli') => launchWithConfiguration('codex', mode, service, 'C:\\project', request, deps, r=>configured.push(r)) };
}
test('desktop launch with current service does not read or write configuration', async () => {
  const f=fixture([environment(config('broken'))]); await f.run('current','desktop'); assert.deepEqual(f.calls,['validate','launch']);
});
test('desktop launch with a chosen key applies and verifies it like the CLI', async () => {
  const f=fixture([environment(config()),environment(config('configured','key-1'))]); await f.run('key-1','desktop');
  assert.deepEqual(f.calls,['validate','scan','apply','scan','launch']); assert.equal(f.configured[0].backupId,'backup-1');
});
test('desktop launch with official service reverts before opening', async () => {
  const f=fixture([environment(config('configured','key-1')),environment(config('official'))]); await f.run('official','desktop');
  assert.deepEqual(f.calls,['validate','scan','revert','scan','launch']);
});
test('installation or directory errors stop before configuration changes', async () => {
  const f=fixture(); f.deps.validateLaunch=async()=>{throw Error('missing directory');};
  await assert.rejects(f.run('key-1'),/missing directory/); assert.deepEqual(f.calls,[]);
});
test('broken configuration blocks even current-service launch', async () => {
  const f=fixture([environment(config('broken'))]); await assert.rejects(f.run(),/修复/); assert.deepEqual(f.calls,['validate','scan']);
});
test('explicit current service preserves custom settings', async () => {
  const f=fixture([environment(config('custom'))]); await f.run(); assert.deepEqual(f.calls,['validate','scan','launch']);
});
test('already matching key and model avoids repeated backups and writes', async () => {
  const f=fixture([environment(config('configured','key-1'))]); await f.run('key-1'); assert.deepEqual(f.calls,['validate','scan','launch']);
});
test('changed profile is applied and verified before spawning', async () => {
  const f=fixture([environment(config()),environment(config('configured','key-1'))]); await f.run('key-1');
  assert.deepEqual(f.calls,['validate','scan','apply','scan','launch']); assert.equal(f.configured[0].backupId,'backup-1');
});
test('failed write never launches', async () => {
  const f=fixture(); f.deps.applyConfig=async()=>{throw Error('keyring locked');};
  await assert.rejects(f.run('key-1'),/keyring locked/); assert.ok(!f.calls.includes('launch')); assert.equal(f.configured.length,0);
});
test('external override after writing blocks launch and preserves rollback result', async () => {
  const f=fixture([environment(config()),environment(config('unknown'))]);
  await assert.rejects(f.run('key-1'),/停止启动/); assert.ok(!f.calls.includes('launch')); assert.equal(f.configured[0].backupId,'backup-1');
});
test('official service must be verified after reverting', async () => {
  const f=fixture([environment(config('configured','key-1')),environment(config('official'))]); await f.run('official');
  assert.deepEqual(f.calls,['validate','scan','revert','scan','launch']); assert.equal(f.configured[0].backupId,'backup-2');
});
test('official service with a remaining external override does not launch', async () => {
  const f=fixture([environment(config('unknown'))]); await assert.rejects(f.run('official'),/停止启动/); assert.ok(!f.calls.includes('launch'));
});
test('spawn failure leaves the successful backup available for undo', async () => {
  const f=fixture([environment(config()),environment(config('configured','key-1'))]); f.deps.launchClient=async()=>{throw Error('spawn failed');};
  await assert.rejects(f.run('key-1'),/spawn failed/); assert.equal(f.configured[0].backupId,'backup-1');
});

function claudeFixture(states = [environment(config())], claudeModel = 'claude-model') {
  const calls = [], configured = [], applied = [], reverted = [];
  const request = { profileId:'key-1', baseUrl:'https://gateway.dkby.com/v1', model:'', reasoningEffort:'medium', claudeModel };
  const deps = {
    validateLaunch: async () => { calls.push('validate'); },
    scanEnvironment: async () => { calls.push('scan'); return states.length > 1 ? states.shift() : states[0]; },
    applyConfig: async req => { calls.push('apply'); applied.push(req); return { backupId:'backup-claude' }; },
    revertToOfficial: async (...args) => { calls.push('revert'); reverted.push(args); return 'backup-claude-official'; },
    launchClient: async () => { calls.push('launch'); return { message:'requested' }; },
  };
  return { calls, deps, configured, applied, reverted, run:(service='current', mode='cli') => launchWithConfiguration('claude', mode, service, 'C:\\project', request, deps, r=>configured.push(r)) };
}

test('Claude desktop launch applies and verifies the selected key and URL', async () => {
  const f=claudeFixture([environment(config('official')),environment(config('configured','key-1','claude-model'))]);
  await f.run('key-1','desktop');
  assert.deepEqual(f.calls,['validate','scan','apply','scan','launch']);
  assert.equal(f.configured[0].backupId,'backup-claude');
});

test('Claude desktop custom service with no model does not rewrite every launch', async () => {
  const f=claudeFixture([environment(config('configured','key-1',null))], '');
  await f.run('key-1','desktop');
  assert.deepEqual(f.calls,['validate','scan','launch']);
});

test('Claude desktop is checked and written through its own config, not the CLI settings', async () => {
  const cli = config('configured','key-1','claude-model'), desktop = config('official');
  const f=claudeFixture([{ codex:cli, claude:cli, claudeDesktop:desktop }, { codex:cli, claude:cli, claudeDesktop:config('configured','key-1','claude-model') }]);
  await f.run('key-1','desktop');
  assert.deepEqual(f.calls,['validate','scan','apply','scan','launch']);
  assert.equal(f.applied[0].configureClaudeDesktop, true);
  assert.equal(f.applied[0].configureClaude, false);
});

test('Claude CLI writes settings.json only and ignores the desktop config', async () => {
  const cli = config('official'), desktop = config('configured','key-1','claude-model');
  const f=claudeFixture([{ codex:cli, claude:cli, claudeDesktop:desktop }, { codex:cli, claude:config('configured','key-1','claude-model'), claudeDesktop:desktop }]);
  await f.run('key-1','cli');
  assert.deepEqual(f.calls,['validate','scan','apply','scan','launch']);
  assert.equal(f.applied[0].configureClaude, true);
  assert.equal(f.applied[0].configureClaudeDesktop, false);
});

test('Claude desktop official service reverts only the desktop config', async () => {
  const f=claudeFixture([environment(config('configured','key-1')),environment(config('official'))]);
  await f.run('official','desktop');
  assert.deepEqual(f.reverted[0],[false,false,true]);
});
