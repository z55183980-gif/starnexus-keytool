import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clientUpdateState } from '../src/clientUpdateState.ts';
const version = (updateAvailable, extra={}) => ({current:'0.169.0',latest:'0.169.0',updateAvailable,warning:null,...extra});
test('installed clients only offer installation after a successful newer-version check', () => {
  assert.equal(clientUpdateState(true,null,false,false),'unknown');
  assert.equal(clientUpdateState(true,version(false),false,false),'current');
  assert.equal(clientUpdateState(true,version(true,{latest:'0.170.0'}),false,false),'available');
  assert.equal(clientUpdateState(true,version(null,{latest:null,warning:'offline'}),false,false),'unknown');
  assert.equal(clientUpdateState(true,version(null),false,false),'unknown');
});
test('checking and pending system installation suppress previously available updates', () => {
  assert.equal(clientUpdateState(true,version(true),true,false),'checking');
  assert.equal(clientUpdateState(true,version(true),false,true),'pending');
  assert.equal(clientUpdateState(true,version(true,{warning:'failed'}),false,false),'unknown');
  assert.equal(clientUpdateState(false,null,false,false),'missing');
});
