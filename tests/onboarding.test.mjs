import { test } from 'node:test';
import assert from 'node:assert/strict';
import { needsConnectionGuide } from '../src/onboarding.ts';
import { mockImportTokens, mockProfiles } from '../src/mock.ts';

const empty = { currentBaseUrl: null, currentKeyMasked: null, currentModel: null };

test('offer connection when no service was detected, not while loading', () => {
  assert.equal(needsConnectionGuide(), false);
  for (const configurationStatus of ['unconfigured', 'unknown']) assert.equal(needsConnectionGuide({ ...empty, configurationStatus }), true);
});

test('preserve existing, ambiguous and damaged configurations', () => {
  for (const configurationStatus of ['configured', 'official', 'custom', 'broken']) assert.equal(needsConnectionGuide({ ...empty, configurationStatus }), false);
  for (const key of Object.keys(empty)) assert.equal(needsConnectionGuide({ ...empty, configurationStatus: 'unknown', [key]: 'present' }), false);
});

test('guided import returns the chosen profile ID and reuses it on retry', () => {
  const { profileIds } = mockImportTokens('fresh', [2]);
  assert.equal(profileIds.length, 1);
  assert.equal(mockProfiles('fresh').find(profile => profile.id === profileIds[0]).name, 'Codex 专用');
  assert.deepEqual(mockImportTokens('fresh', [2]).profileIds, profileIds);
  assert.deepEqual(mockImportTokens('fresh', [4]).profileIds, []);
});

test('show the welcome guide only on a true first run', async () => {
  const { needsOnboarding } = await import('../src/onboarding.ts');
  const store = (items = {}) => ({ getItem: key => items[key] ?? null });
  assert.equal(needsOnboarding(store()), true);
  assert.equal(needsOnboarding(store({ 'starnexus.onboarded': '1' })), false);
  assert.equal(needsOnboarding(store({ 'starnexus.lastToolPage': 'claude' })), false);
  assert.equal(needsOnboarding(null), false);
  assert.equal(needsOnboarding(store({ 'starnexus.onboarded': '1' }), '?onboarding=1'), true);
});
