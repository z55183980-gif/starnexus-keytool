import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { launchWithConfiguration, saveLaunchConfiguration } from '../src/launchFlow.ts';

// Exercise the actual component handlers with mocked IPC/state boundaries.
function handler(file, name, bindings) {
  const text = fs.readFileSync(new URL(file, import.meta.url), 'utf8');
  const ast = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let source;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === name) source = node.initializer.getText(ast);
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(source, `missing handler ${name}`);
  const js = ts.transpileModule(`const handler = ${source}`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  return new Function(...Object.keys(bindings), `${js}; return handler;`)(...Object.values(bindings));
}

test('launch button never waits on a hidden service selection', () => {
  const main = handler('../src/Launcher.tsx', 'main', {
    missing:false, blockerAction:null, blocker:null,
    errors:[], t:s=>s, targetName:()=> 'Codex CLI', target:'codex-cli', mode:'cli', launchDisabled:false,
  });
  assert.equal(main.label,'启动 {name}');
  assert.equal(main.off,false);
  assert.equal(main.title,undefined);
});

test('service list has a single live state: no separate keep-current row or pending-enable state', () => {
  const source = fs.readFileSync(new URL('../src/Launcher.tsx', import.meta.url), 'utf8');
  assert.ok(!source.includes('所选密钥已失效'));
  assert.ok(!source.includes('option("current"'));
  assert.ok(!source.includes('needsSave'));
  assert.ok(!source.includes('invalidService'));
  assert.ok(!source.includes('`${tool}Service`'));
  assert.ok(source.includes('{live !== value && <button type="button" className="service-enable"'));
});

test('row switch writes the clicked key without launching or saving a selection preference', async () => {
  const calls=[];
  const activate = handler('../src/Launcher.tsx','activateService',{
    prefs:{}, savingRef:{current:false}, activatingRef:{current:false}, props:{busy:false,onSaveConfiguration:async(...args)=>calls.push(['write',...args])},
    tool:'codex',mode:'desktop',setActivating:()=>{},setRepair:()=>{},save:async patch=>{calls.push(['select',patch]);return true;},
  });
  await activate('clicked-key');
  assert.deepEqual(calls,[['write','codex','desktop','clicked-key']]);
});

for (const patch of [{codexModel: 'new-model', codexEffort: 'high'}, {claudeModel: 'new-claude'}]) {
  test(`model update preserves both custom endpoints: ${Object.keys(patch)[0]}`, async () => {
    const profile = {id:'custom',name:'relay',codexModel:'old',codexEffort:'medium',claudeModel:'old-claude',codexBaseUrl:'https://relay.example/v1',claudeBaseUrl:'https://anthropic.example'};
    let sent;
    const save = handler('../src/App.tsx', 'saveModel', {
      profiles:[profile], api:{saveProfile:async req=>{sent=req;},listProfiles:async()=>[profile]},
      setBusy:()=>{}, log:()=>{}, t:s=>s, effortLabel:s=>s, setProfiles:()=>{}, setToast:()=>{},
    });
    assert.equal(await save(profile.id, patch), true);
    assert.equal(sent.codexBaseUrl, profile.codexBaseUrl);
    assert.equal(sent.claudeBaseUrl, profile.claudeBaseUrl);
    assert.equal(sent.apiKey, '');
    for (const [key,value] of Object.entries(patch)) assert.equal(sent[key],value);
  });
}

test('launch stays read-only and always uses the live configuration', async () => {
  const source = fs.readFileSync(new URL('../src/Launcher.tsx', import.meta.url), 'utf8');
  assert.ok(source.includes('const LIVE = "current";'));
  assert.ok(source.includes('props.onLaunch(tool, mode, LIVE, prefs.projectDirectory)'));
  const calls=[];
  await launchWithConfiguration('codex','cli','current','C:\fixture', {
    profileId:'saved-key',baseUrl:'https://relay.example/v1',model:'old-preference',reasoningEffort:'medium',
  }, {
    validateLaunch:async()=>{},
    scanEnvironment:async()=>({codex:{configurationStatus:'configured',activeProfileId:'saved-key',currentModel:'manual-model'}}),
    applyConfig:async()=>{throw Error('must not write');},revertToOfficial:async()=>{throw Error('must not revert');},
    launchClient:async()=>{calls.push('launch');return {message:'fixture'};},
  },()=>{throw Error('must not configure');});
  assert.deepEqual(calls,['launch']);
});

test('changing the live key model rewrites its config with the freshly saved profile', async () => {
  const profile = {id:'k',name:'main',codexModel:'old',codexEffort:'medium',claudeModel:''};
  const fresh = {...profile,codexModel:'new'};
  const writes=[];
  const save = handler('../src/App.tsx', 'saveModel', {
    profiles:[profile], api:{saveProfile:async()=>{},listProfiles:async()=>[fresh]},
    setBusy:()=>{}, log:()=>{}, t:s=>s, effortLabel:s=>s, setProfiles:()=>{}, setToast:()=>{},
    runClientAction:async(...args)=>{writes.push(args);return true;},
  });
  assert.equal(await save('k', {codexModel:'new',codexEffort:'medium'}, {tool:'codex',mode:'cli'}), true);
  assert.deepEqual(writes,[['codex','cli','k','','save',fresh]]);
});

test('selected duplicate profile and tool scope are supplied to every launch verification', async () => {
  const scans=[];
  await saveLaunchConfiguration('codex','cli','duplicate-2', {
    profileId:'duplicate-2',baseUrl:'https://relay.example/v1',model:'m',reasoningEffort:'medium',
  }, {
    validateLaunch:async()=>{},
    scanEnvironment:async(...args)=>{scans.push(args);return {codex:{configurationStatus:scans.length===1?'official':'configured',activeProfileId:args[2],currentModel:'m',currentReasoningEffort:'medium'}};},
    applyConfig:async()=>({backupId:'fixture'}),revertToOfficial:async()=>'',launchClient:async()=>({message:'fixture'}),
  },()=>{});
  assert.deepEqual(scans,[['https://relay.example/v1',['codex'],'duplicate-2'],['https://relay.example/v1',['codex'],'duplicate-2']]);
});
