import {readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {deflateSync} from 'node:zlib';
import {build,transform} from 'esbuild';
import {minify} from 'terser';
import {compactRelease,compactScript} from './private-names.mjs';

const root=fileURLToPath(new URL('.',import.meta.url));
const read=name=>readFile(root+'src/'+name,'utf8');
export async function compileWidget(screen, compression={passes:2,sequences:1,keep_fargs:false,conditionals:false}) {
  if(!['release','graph','range','unified'].includes(screen))throw Error('Unknown widget capability: '+screen);
  const menu=['release','unified'].includes(screen),chart=['graph','unified'].includes(screen),range=['range','unified'].includes(screen);
  const tokens=JSON.parse(await readFile(root+'design-tokens.json','utf8'));
  let css=await read('widget.css');
  if(menu)css+=await read('menu.css');
  if(chart||range)css+=await read('media.css');
  if(chart)css+=await read('graph.css');
  if(range)css+=await read('range.css');
  css+=await read('widget-utilities.css');
  // A shipping subset has one mode. Resolve its mode constants at build time;
  // the unified artifact keeps the same definitions for runtime transitions.
  if(screen!=='unified') {
    const values={};
    css=css.replace(/:root(?:\[data-mode=media\])?\s*\{([^{}]+)\}/g,(rule,body)=>{
      if(!body.includes('--ui-'))return rule;
      for(const [,key,value] of body.matchAll(/(--ui-[\w-]+)\s*:\s*([^;]+);/g))values[key]=value.trim();
      return '';
    });
    css=css.replace(/var\((--ui-[\w-]+)\)/g,(_,key)=>values[key]);
    css=css.replaceAll(menu?'[data-mode=menu] ':'[data-mode=media] ','');
  }
  for(const [key,value] of Object.entries(tokens.shared))css=css.replaceAll('var('+key+')',value);
  const used=new Set(css.match(/--ha-[a-z0-9-]+/g));
  const values=theme=>Object.entries(theme).filter(([key])=>used.has(key)).map(([key,value])=>key+':'+value+';').join('');
  css=':root{'+values(tokens.themes.light)+'}:root[data-theme=dark]{'+values(tokens.themes.dark)+'}'+css;
  const aliases=[...new Set(css.match(/--(?:ha|ui)-[a-z0-9-]+/g))];
  css=css.replace(/--(?:ha|ui)-[a-z0-9-]+/g,key=>{const i=aliases.indexOf(key);return '--'+String.fromCharCode(97+i%26)+(i>=26?Math.floor(i/26):'');});
  css=(await transform(css,{loader:'css',minify:true,charset:'utf8'})).code.trim();
  const define=Object.fromEntries(Object.entries({MENU:menu,MEDIA:chart||range,CHART:chart,RANGE:range,UNIFIED:screen==='unified'}).map(([k,v])=>[k,String(v)]));
  let source=await read('widget.js');
  if(screen!=='unified') {
    // These pure predicates are constants in a single-capability artifact.
    // Resolve them before bundling so unused renderers and branches tree-shake.
    source=source.replace(/\bmedia\((?:data|v)\)/g,String(!menu))
      .replace(/\bchart\((?:data|v)\)/g,String(chart));
  }
  const bundle=await build({stdin:{contents:source,resolveDir:root+'src',sourcefile:'widget.js'},bundle:true,write:false,format:'esm',target:'es2020',charset:'utf8',define,minifySyntax:true});
  const script=await minify(compactScript(bundle.outputFiles[0].text),{ecma:2020,module:true,compress:compression,format:{ascii_only:true}});
  let body=await read('widget.html');
  if(!range)body=body.replace(' id="r-top"','');
  if(!chart)body=body.replace(/<!-- chart:start -->[\s\S]*?<!-- chart:end -->/,'');
  body=body.replace(/<!--[\s\S]*?-->/g,'').replace(/>\s+</g,'><').replace(/="([A-Za-z0-9_-]+)"/g,'=$1').trim();
  const html='<!doctype html><meta charset=utf-8><style>'+compactRelease(css)+'</style>'+compactRelease(body)+'<script>'+script.code+'</script>';
  const bytes=Buffer.byteLength(html),zlibBytes=deflateSync(html,{level:9}).length;
  return {html,manifest:{designVersion:tokens.version,screen,bytes,zlibBytes,limit:4096,protocol:'r1',transport:'TelegramWidget',liveIntegrationVerified:false}};
}
