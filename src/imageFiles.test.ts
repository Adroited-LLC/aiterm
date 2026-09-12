import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isImage, fitImageScale } from './imageFiles.ts';

test('image tabs recognize image filenames without mistaking code or folders for images', () => {
  for (const suffix of ['PNG', 'jpg', 'jpeg', 'svg', 'bmp', 'gif', 'webp', 'ico']) {
    assert.equal(isImage(`/home/matt/a picture.${suffix}`), true);
    assert.equal(isImage(`C:\\Users\\Matt\\a.${suffix}`), true);
  }
  for (const path of ['/a.png/readme.md', '/a.svg.ts', '/png', '/a.pdf', '/a.png.bak']) assert.equal(isImage(path), false);
});
test('fit preserves aspect ratio, contains both dimensions, and does not enlarge small images', () => {
  assert.equal(fitImageScale(2400, 1200, 600, 500), .25);
  assert.equal(fitImageScale(1200, 2400, 600, 500), 500 / 2400);
  assert.equal(fitImageScale(32, 32, 600, 500), 1);
  assert.equal(fitImageScale(1200, 2400, 0, 0), 1);
});
