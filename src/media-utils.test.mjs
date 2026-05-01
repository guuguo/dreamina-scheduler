import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  fileExt,
  isImagePath,
  isAudioPath,
  isSupportedRoleMedia,
  isVideoPath,
  normalizeFilePathKey,
  uniqueFilePaths,
  splitCsv,
} from './media-utils.js';

test('isImagePath recognizes png/jpg/jpeg/webp', () => {
  assert.equal(isImagePath('/tmp/role.png'), true);
  assert.equal(isImagePath('/tmp/role.jpg'), true);
  assert.equal(isImagePath('/tmp/role.jpeg'), true);
  assert.equal(isImagePath('/tmp/role.webp'), true);
  assert.equal(isImagePath('/tmp/role.mp3'), false);
  assert.equal(isImagePath('/tmp/role.mp4'), false);
  assert.equal(isImagePath(''), false);
});

test('isAudioPath recognizes mp3/wav/m4a/aac', () => {
  assert.equal(isAudioPath('/tmp/voice.mp3'), true);
  assert.equal(isAudioPath('/tmp/voice.wav'), true);
  assert.equal(isAudioPath('/tmp/voice.m4a'), true);
  assert.equal(isAudioPath('/tmp/voice.aac'), true);
  assert.equal(isAudioPath('/tmp/voice.png'), false);
  assert.equal(isAudioPath('/tmp/voice.mp4'), false);
});

test('isSupportedRoleMedia accepts image and audio but not video', () => {
  assert.equal(isSupportedRoleMedia('/tmp/img.png'), true);
  assert.equal(isSupportedRoleMedia('/tmp/voice.mp3'), true);
  assert.equal(isSupportedRoleMedia('/tmp/video.mp4'), false);
  assert.equal(isSupportedRoleMedia('/tmp/file.txt'), false);
});

test('isVideoPath rejects video from role media', () => {
  assert.equal(isVideoPath('/tmp/clip.mp4'), true);
  assert.equal(isVideoPath('/tmp/clip.mov'), true);
  assert.equal(isVideoPath('/tmp/clip.webm'), true);
  assert.equal(isVideoPath('/tmp/clip.mkv'), true);
  assert.equal(isVideoPath('/tmp/clip.png'), false);
});

test('unsupported file extensions are rejected', () => {
  assert.equal(isSupportedRoleMedia('/tmp/file.txt'), false);
  assert.equal(isSupportedRoleMedia('/tmp/file.pdf'), false);
  assert.equal(isSupportedRoleMedia('/tmp/file.doc'), false);
  assert.equal(isSupportedRoleMedia('/tmp/file'), false);
});

test('uniqueFilePaths deduplicates and normalizes paths', () => {
  const paths = ['/tmp/a.png', '/tmp/a.png', '/tmp/b.png', '  /tmp/a.png  ', ''];
  const result = uniqueFilePaths(paths);
  assert.equal(result.length, 2);
  assert.deepEqual(result, ['/tmp/a.png', '/tmp/b.png']);
});

test('splitCsv splits on comma and Chinese comma', () => {
  assert.deepEqual(splitCsv('a,b,c'), ['a', 'b', 'c']);
  assert.deepEqual(splitCsv('a，b，c'), ['a', 'b', 'c']);
  assert.deepEqual(splitCsv('a,，b'), ['a', 'b']);
  assert.deepEqual(splitCsv(''), []);
  assert.deepEqual(splitCsv(null), []);
});

test('fileExt extracts lowercase extension', () => {
  assert.equal(fileExt('/tmp/role.PNG'), 'png');
  assert.equal(fileExt('/tmp/role.JpG'), 'jpg');
  assert.equal(fileExt('noext'), 'noext');
  assert.equal(fileExt(''), '');
});
