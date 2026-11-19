import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ClientVersionCache, shortClientVersion } from '../src/clientVersionCache.ts';

const local = {current:'0.169.0',latest:null,updateAvailable:null,warning:null,path:'cli'};
test('navigation shares probes and cached failures; changed paths get a fresh probe',async()=>{
  const cache=new ClientVersionCache(); let calls=0;
  const probe=async()=>{calls++;return local;};
  await Promise.all([cache.read('codex-cli','a',probe),cache.read('codex-cli','a',probe)]);
  await cache.read('codex-cli','a',probe); assert.equal(calls,1);
  await cache.read('codex-cli','b',probe); assert.equal(calls,2);
  const failed=async()=>{calls++;throw Error('offline');};
  await cache.read('claude-cli','a',failed); await cache.read('claude-cli','a',failed);
  assert.equal(calls,3);
});
test('explicit checks replace cache and cannot be overwritten by an older local probe',async()=>{
  const cache=new ClientVersionCache(); let finish;
  const pending=cache.read('codex-cli','a',()=>new Promise(resolve=>{finish=resolve;}));
  const updated={...local,current:'0.170.0',latest:'0.170.0',updateAvailable:false};
  cache.set('codex-cli','a',updated); finish(local);
  assert.deepEqual(await pending,updated); assert.deepEqual(cache.peek('codex-cli','a'),updated);
});
test('compact versions preserve significant components and prerelease identifiers',()=>{
  assert.equal(shortClientVersion('5.11492.5.0'),'5.11492.5');
  for(const v of ['0.169.0','26.1111.2228.1','1.0.0-beta.1']) assert.equal(shortClientVersion(v),v);
});
