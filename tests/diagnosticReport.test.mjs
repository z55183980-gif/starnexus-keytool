import { test } from 'node:test';
import assert from 'node:assert/strict';
import { diagnosticReport } from '../src/diagnosticReport.ts';

const secret = 'private-sentinel-arbitrary-format';
const config = {
  homeExists: true, home: `C:/Users/${secret}`, configState: {broken: `invalid ${secret}`},
  settingsState: {broken: secret}, authState: 'ApiKeyOnly', currentBaseUrl: `https://user:${secret}@example.test/v1?token=${secret}`,
  currentModel: secret, currentReasoningEffort: secret, residue: [secret], managedByUs: true,
  activeProfileId: secret, unsavedGatewayKey: true, configurationStatus: 'broken', statusMessage: secret,
};
test('export preserves useful checks and never exports raw data, even nonstandard secrets', () => {
  const text=diagnosticReport('codex','cli',{codex:config,claude:config,claudeDesktop:config},[
    {id:'K6',level:'error',text:secret,fix:'model'},
    {id:secret,level:secret,text:secret,fix:secret},
    {id:'__proto__',level:'warn',text:secret,fix:'none'},
  ]);
  assert.ok(!text.includes(secret));assert.ok(!text.includes('https://'));assert.ok(!text.includes('C:/Users/'));
  const report=JSON.parse(text);
  assert.equal(report.configuration.fileState,'broken');assert.equal(report.configuration.authState,'ApiKeyOnly');
  assert.deepEqual(report.checks[0],{id:'K6',level:'error',summary:'模型接口不兼容',action:'model'});
  assert.equal(report.checks[1].id,'unknown');assert.equal(report.checks[2].id,'unknown');
});
test('export uses the selected desktop/CLI configuration and treats unknown enum values as private',()=>{
  const env={codex:config,claude:{...config,configurationStatus:'configured',settingsState:'ok'},claudeDesktop:{...config,configurationStatus:secret,settingsState:'missing'}};
  const desktop=JSON.parse(diagnosticReport('claude','desktop',env,[]));
  assert.equal(desktop.configuration.status,'unknown');
  // The real Claude configuration has settingsState, never Codex configState.
  delete env.claude.configState;delete env.claudeDesktop.configState;
  assert.equal(JSON.parse(diagnosticReport('claude','desktop',env,[])).configuration.fileState,'missing');
  assert.equal(JSON.parse(diagnosticReport('claude','cli',env,[])).configuration.fileState,'ok');
  assert.ok(!diagnosticReport('claude','desktop',env,[]).includes(secret));
});
test('empty diagnostics remain local and need no account, key, model or network',()=>{
  const report=JSON.parse(diagnosticReport('codex','cli',null,[]));
  assert.equal(report.configuration,null);assert.deepEqual(report.checks,[]);assert.equal(report.product,'StarNexus Key Tool');
});
