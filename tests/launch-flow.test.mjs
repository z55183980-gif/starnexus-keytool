import { test } from 'node:test';
import assert from 'node:assert/strict';
import { configurationMatches, desktopModels, launchWithConfiguration, saveLaunchConfiguration } from '../src/launchFlow.ts';

const request = { profileId:'key-1', baseUrl:'https://gateway.dkby.com/v1', model:'model', reasoningEffort:'medium', claudeModel:'claude-model' };
const config = (status = 'official', id = null, currentModel = 'model') => ({ configurationStatus:status, activeProfileId:id, currentModel, currentReasoningEffort:'medium' });
const environment = value => ({ codex:value, claude:value, claudeDesktop:value });
function fixture(states = [environment(config())], tool = 'codex', mode = 'cli', patch = {}) {
  const calls = [], configured = [], applied = [], reverted = [];
  const req = { ...request, ...patch };
  const deps = {
    validateLaunch: async launch => { calls.push('validate'); assert.deepEqual(launch,{ target:`${tool}-${mode}`, directory:'C:\\project', service }); },
    scanEnvironment: async (...args) => { calls.push('scan'); assert.deepEqual(args,[req.baseUrl,[tool === 'claude' ? `claude-${mode}` : tool],service]); return states.length > 1 ? states.shift() : states[0]; },
    applyConfig: async req => { calls.push('apply'); applied.push(req); return { backupId:'backup-1' }; },
    revertToOfficial: async (...args) => { calls.push('revert'); reverted.push(args); return 'backup-2'; },
    launchClient: async () => { calls.push('launch'); return { message:'requested' }; },
  };
  let service;
  return { calls, deps, configured, applied, reverted,
    save: (choice='key-1') => { service=choice; return saveLaunchConfiguration(tool, mode, choice, req, deps, r=>configured.push(r)); },
    launch: (choice='key-1') => { service=choice; return launchWithConfiguration(tool, mode, choice, 'C:\\project', req, deps); },
  };
}

for (const tool of ['codex','claude']) for (const mode of ['desktop','cli']) {
  const model = tool === 'codex' ? 'model' : 'claude-model';
  test(`${tool} ${mode}: save verifies configuration without launching; separate launch never writes`, async () => {
    const f=fixture([environment(config()), environment(config('configured','key-1',model))],tool,mode);
    await f.save();
    assert.deepEqual(f.calls,['scan','apply','scan']);
    assert.equal(f.configured[0].backupId,'backup-1');
    assert.equal(f.applied[0].configureCodex,tool==='codex');
    assert.equal(f.applied[0].configureClaude,tool==='claude' && mode==='cli');
    assert.equal(f.applied[0].configureClaudeDesktop,tool==='claude' && mode==='desktop');
    await f.launch();
    assert.deepEqual(f.calls,['scan','apply','scan','validate','scan','launch']);
  });
  test(`${tool} ${mode}: unsaved changes block every launch entry point without writing`, async () => {
    const f=fixture(undefined,tool,mode);
    await assert.rejects(f.launch(),/先保存并写入配置/);
    assert.deepEqual(f.calls,['validate','scan']);
  });
  test(`${tool} ${mode}: switching to official is saved before launching`, async () => {
    const f=fixture([environment(config('configured','key-1',model)),environment(config())],tool,mode);
    await f.save('official');
    assert.deepEqual(f.calls,['scan','revert','scan']);
    assert.deepEqual(f.reverted[0],[tool==='codex',tool==='claude' && mode==='cli',tool==='claude' && mode==='desktop']);
    await f.launch('official');
    assert.deepEqual(f.calls,['scan','revert','scan','validate','scan','launch']);
  });
}

test('current desktop preserves even broken configuration; current CLI checks configuration', async () => {
  const desktop=fixture([environment(config('broken'))],'codex','desktop');
  await desktop.save('current'); assert.deepEqual(desktop.calls,[]);
  await desktop.launch('current'); assert.deepEqual(desktop.calls,['validate','launch']);
  const cli=fixture([environment(config('broken'))]);
  await assert.rejects(cli.launch('current'),/修复/); assert.deepEqual(cli.calls,['validate','scan']);
});

test('current CLI preserves custom configuration', async () => {
  const f=fixture([environment(config('custom'))]); await f.launch('current');
  assert.deepEqual(f.calls,['validate','scan','launch']);
});

test('save is independent of installation and working directory; failed launch validation writes nothing', async () => {
  const f=fixture([environment(config()),environment(config('configured','key-1'))]);
  f.deps.validateLaunch=async()=>{throw Error('missing directory');};
  await f.save(); assert.deepEqual(f.calls,['scan','apply','scan']);
  await assert.rejects(f.launch(),/missing directory/);
  assert.deepEqual(f.calls,['scan','apply','scan']);
});

test('failed write never launches or reports a verified configuration', async () => {
  const f=fixture(); f.deps.applyConfig=async()=>{throw Error('keyring locked');};
  await assert.rejects(f.save(),/keyring locked/);
  assert.deepEqual(f.calls,['scan']); assert.equal(f.configured.length,0);
});

test('verification rejects external overrides and preserves the backup for undo', async () => {
  for (const final of [config('unknown'), {...config('configured','key-1'),currentModel:'unexpected'}, {...config('configured','key-1'),currentReasoningEffort:'high'}]) {
    const f=fixture([environment(config()),environment(final)]);
    await assert.rejects(f.save(),/停止启动/);
    assert.deepEqual(f.calls,['scan','apply','scan']); assert.equal(f.configured[0].backupId,'backup-1');
  }
});

test('configuration changed after saving requires another explicit save', async () => {
  const f=fixture([environment(config()),environment(config('configured','key-1')),environment(config('configured','other-key'))]);
  await f.save(); await assert.rejects(f.launch(),/先保存并写入配置/);
  assert.deepEqual(f.calls,['scan','apply','scan','validate','scan']);
});

test('official service verification catches remaining external overrides', async () => {
  const f=fixture([environment(config('unknown'))]);
  await assert.rejects(f.save('official'),/停止启动/);
  assert.deepEqual(f.calls,['scan','revert','scan']);
});

test('already matching configuration avoids writing, including empty custom Claude model', async () => {
  const f=fixture([environment(config('configured','key-1'))]);
  await f.save(); assert.deepEqual(f.calls,['scan']);
  const claude=fixture([environment(config('configured','key-1',null))],'claude','desktop',{claudeModel:''});
  await claude.save(); await claude.launch(); assert.deepEqual(claude.calls,['scan','validate','scan','launch']);
});

test('spawn failure never writes configuration', async () => {
  const f=fixture([environment(config('configured','key-1'))]); f.deps.launchClient=async()=>{throw Error('spawn failed');};
  await assert.rejects(f.launch(),/spawn failed/); assert.deepEqual(f.calls,['validate','scan']);
});

test('old or unverified Codex rewrites an otherwise matching builtin relay', async () => {
  for (const verifiedBuiltin of [false, true]) {
    const value = config('configured','key-1');
    const first = environment({...value, builtinRelay:true});
    const f = fixture([first,environment({...value,builtinRelay:false})]);
    f.deps.checkClientVersion = async () => ({verifiedBuiltin});
    await f.save();
    assert.equal(f.applied.length, verifiedBuiltin ? 0 : 1);
  }
});

test('Claude alias changes trigger a write even if the main model matches', async () => {
  const value = config('configured','key-1','claude-model');
  const f = fixture([environment({...value,configuredModels:['old','','']}),environment({...value,configuredModels:['new','','']})], 'claude','cli',{claudeOpusModel:'new'});
  await f.save(); assert.equal(f.applied.length,1);
});

test('Claude desktop verifies the written list including gateway extras, and rewrites an old list without them', async () => {
  const extras = { claudeOpusModel:'claude-model', claudeSonnetModel:'sonnet', claudeHaikuModel:'sonnet', claudeDesktopExtraModels:['fable','sonnet'] };
  assert.deepEqual(desktopModels({ claudeModel:'claude-model', ...extras }), ['claude-model','sonnet','fable']);
  const value = config('configured','key-1','claude-model');
  const written = environment({...value,configuredModels:['claude-model','sonnet','fable']});
  // 老版本写的列表缺附加模型：要重写；写完带上附加模型，核对通过（不再误报「被外部配置覆盖」）
  const f = fixture([environment({...value,configuredModels:['claude-model','sonnet']}),written],'claude','desktop',extras);
  await f.save(); assert.deepEqual(f.calls,['scan','apply','scan']);
  assert.deepEqual(f.applied[0].claudeDesktopExtraModels,['fable','sonnet']);
  // 已经是完整列表：不写，直接启动
  const ready = fixture([written],'claude','desktop',extras);
  await ready.launch(); assert.deepEqual(ready.calls,['validate','scan','launch']);
});

test('button readiness uses the correct configuration, model and reasoning effort', () => {
  const matched=config('configured','key-1','claude-model');
  const env={codex:config('configured','key-1'),claude:matched,claudeDesktop:config()};
  assert.equal(configurationMatches('claude','desktop','key-1',env,'claude-model','medium'),false);
  assert.equal(configurationMatches('claude','cli','key-1',env,'claude-model','medium'),true);
  assert.equal(configurationMatches('codex','cli','key-1',env,'model','medium'),true);
  assert.equal(configurationMatches('codex','desktop','key-1',env,'model','high'),false);
  assert.equal(configurationMatches('codex','desktop','key-1',env,'changed','medium'),false);
  assert.equal(configurationMatches('codex','desktop','current',null,'',''),true);
  assert.equal(configurationMatches('codex','desktop','key-1',null,'model','medium'),false);
});
