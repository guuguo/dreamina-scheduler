import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { shouldAutoPlayAudioModal } from './audio-modal-utils.js';

test('shouldAutoPlayAudioModal: audio with local path auto plays on preview open', () => {
  assert.equal(shouldAutoPlayAudioModal({ stored_path: '/tmp/voice.mp3' }), true);
});

test('shouldAutoPlayAudioModal: missing local path does not auto play', () => {
  assert.equal(shouldAutoPlayAudioModal({ stored_path: '' }), false);
  assert.equal(shouldAutoPlayAudioModal(null), false);
});
