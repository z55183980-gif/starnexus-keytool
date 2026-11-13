import { test } from 'node:test';
import assert from 'node:assert/strict';
import { profileServiceUrl, serviceUrlFields } from '../src/serviceUrl.ts';

const defaultUrl = 'https://gateway.dkby.com/v1';

test('project URL keeps built-in service behavior for root and versioned forms', () => {
  for (const url of [defaultUrl, `${defaultUrl}/`, ' https://gateway.dkby.com/ ']) {
    assert.deepEqual(serviceUrlFields(url, defaultUrl), { codexBaseUrl: '', claudeBaseUrl: '' });
  }
});

test('one custom URL maps to the endpoints expected by both clients', () => {
  for (const url of ['https://gateway.example', 'https://gateway.example/v1/']) {
    assert.deepEqual(serviceUrlFields(url, defaultUrl), {
      codexBaseUrl: 'https://gateway.example/v1', claudeBaseUrl: 'https://gateway.example',
    });
  }
  assert.deepEqual(serviceUrlFields('https://gateway.example/proxy/v1', defaultUrl), {
    codexBaseUrl: 'https://gateway.example/proxy/v1', claudeBaseUrl: 'https://gateway.example/proxy',
  });
});

test('editing a legacy profile without changing its visible URL preserves both endpoints', () => {
  const profiles = [
    { custom: true, codexBaseUrl: '', claudeBaseUrl: 'https://api.anthropic.com' },
    { custom: true, codexBaseUrl: 'https://openai.example/v1', claudeBaseUrl: 'https://claude.example' },
    { custom: true, codexBaseUrl: 'https://codex.example/v1', claudeBaseUrl: '' },
  ];
  for (const profile of profiles) {
    assert.deepEqual(serviceUrlFields(profileServiceUrl(profile, defaultUrl), defaultUrl, profile), {
      codexBaseUrl: profile.codexBaseUrl, claudeBaseUrl: profile.claudeBaseUrl,
    });
  }
});

test('changing a legacy URL replaces both endpoints and switching to project URL clears them', () => {
  const original = { custom: true, codexBaseUrl: '', claudeBaseUrl: 'https://old.example' };
  assert.deepEqual(serviceUrlFields('https://new.example/v1', defaultUrl, original), {
    codexBaseUrl: 'https://new.example/v1', claudeBaseUrl: 'https://new.example',
  });
  assert.deepEqual(serviceUrlFields(defaultUrl, defaultUrl, original), { codexBaseUrl: '', claudeBaseUrl: '' });
});

test('invalid URLs are rejected before saving credentials', () => {
  for (const url of ['', 'gateway.example', 'file:///tmp', 'https://user:pass@host.test', 'https://host.test?key=secret', 'https://host.test/#fragment']) {
    assert.throws(() => serviceUrlFields(url, defaultUrl));
  }
});
