import test from 'node:test';
import assert from 'node:assert/strict';
import { compactRelease, compactScript, releaseExpression, mediaExpression, releaseNames } from '../private-names.mjs';

test('private release names preserve protocol, semantic attributes and dynamic suffixes',()=>{
  assert.equal(compactRelease('r-'+'nav'), 'nav');
  assert.equal(compactRelease('r-'+'read'), 'read');
  assert.equal(compactRelease('aria-label data-kind data-key r1:revision:key --tg-theme-text-color'), 'aria-label data-kind data-key r1:revision:key --tg-theme-text-color');
  const domNames=Object.entries(releaseNames).filter(([key])=>!key.endsWith('-')).map(([,value])=>value);
  assert.equal(new Set(domNames).size,domNames.length);
});
test('readable tests target the shipped DOM without rewriting fixture data',()=>{
  assert.equal(releaseExpression(`d.getElementById('release')`),`d.getElementById('${releaseNames['r-root']}')`);
  assert.equal(releaseExpression(`d.getElementById('release-frame')`),`d.getElementById('release-frame')`);
  assert.equal(releaseExpression(`d.querySelectorAll('.r-bounded:not([data-kind="home"]) .r-row')`),`d.querySelectorAll('.${releaseNames['r-bounded']}:not([data-kind="home"]) .${releaseNames['r-row']}')`);
  assert.equal(releaseExpression(`w.previewSetData({title:'r-title',kind:'room'})`),`w.previewSetData({title:'r-title',kind:'room'})`);
  assert.equal(releaseExpression(`d.getElementById('r-title').classList.contains('r-hidden')`),`d.getElementById('${releaseNames['r-title']}').classList.contains('${releaseNames['r-hidden']}')`);
});

test('media tests share compact IDs with menu and map legacy pager names',()=>{
  assert.equal(mediaExpression("d.getElementById('g-box')"),`d.getElementById('${releaseNames['g-box']}')`);
  assert.equal(mediaExpression("rect('r-local-next')"),"rect('next')");
  assert.equal(mediaExpression("d.getElementById('r-footer')"),`d.getElementById('${releaseNames['r-server']}')`);
});

test('DOM compaction never rewrites subtraction, regexes or comments in JavaScript',()=>{
  const source=`const count=g-1,offset=r-1; // 'r-title' stays a comment
    const pattern=/g-box/; const id="r-title",prefix='g-';`;
  assert.equal(compactScript(source),`const count=g-1,offset=r-1; // 'r-title' stays a comment
    const pattern=/g-box/; const id="${releaseNames['r-title']}",prefix="";`);
});
