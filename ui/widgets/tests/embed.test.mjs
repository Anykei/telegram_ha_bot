import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { embed } from '../preview/embed.mjs';

test('preview embedding preserves minified dollars, markup and marker-like text literally', () => {
  const value = 'key===$&&(busy||waiting); $` $\' $$ $1 </script> __MOCK__';
  for (const key of ['PAYLOAD','MOCK','MANIFEST','FIXTURES']) {
    const encoded = embed(`__${key}__`, {[key]:value});
    assert.equal(JSON.parse(encoded), value);
    assert.ok(!encoded.includes('</script>'), 'Embedded content cannot close the outer script');
  }
  assert.throws(() => embed('__PAYLOAD__', {}), /Missing preview value/);
});

test('every generated preview contains exactly the shipped widget HTML', async () => {
  for (const [screen, preview] of [['switch','preview'],['panel','panel-preview'],['release','release-preview'],['graph','graph-preview']]) {
    const html = await readFile(new URL(`../dist/${screen}.html`, import.meta.url), 'utf8');
    const page = await readFile(new URL(`../dist/${preview}.html`, import.meta.url), 'utf8');
    const payload = page.match(/const payload\s*=\s*("(?:[^"\\]|\\.)*")/);
    assert.ok(payload, `${screen}: embedded payload found`);
    assert.equal(JSON.parse(payload[1]), html, `${screen}: payload preserved without replacement expansion`);
  }
});
