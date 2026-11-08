import test from 'node:test';
import assert from 'node:assert/strict';
import { compactRelease, releaseExpression, releaseNames } from '../private-names.mjs';

test('private release names preserve protocol, semantic attributes and dynamic suffixes',()=>{
  assert.equal(compactRelease('r-'+'nav'), 'nav');
  assert.equal(compactRelease('r-'+'read'), 'read');
  assert.equal(compactRelease('aria-label data-kind data-key r1:revision:key --tg-theme-text-color'), 'aria-label data-kind data-key r1:revision:key --tg-theme-text-color');
  assert.equal(new Set(Object.values(releaseNames)).size,Object.keys(releaseNames).length);
});
test('readable tests target the shipped DOM without rewriting fixture data',()=>{
  assert.equal(releaseExpression(`d.getElementById('release')`),`d.getElementById('${releaseNames['r-root']}')`);
  assert.equal(releaseExpression(`d.getElementById('release-frame')`),`d.getElementById('release-frame')`);
  assert.equal(releaseExpression(`d.querySelectorAll('.r-bounded:not([data-kind="home"]) .r-row')`),`d.querySelectorAll('.${releaseNames['r-bounded']}:not([data-kind="home"]) .${releaseNames['r-row']}')`);
  assert.equal(releaseExpression(`w.previewSetData({title:'r-title',kind:'room'})`),`w.previewSetData({title:'r-title',kind:'room'})`);
  assert.equal(releaseExpression(`d.getElementById('r-title').classList.contains('r-hidden')`),`d.getElementById('${releaseNames['r-title']}').classList.contains('${releaseNames['r-hidden']}')`);
});
