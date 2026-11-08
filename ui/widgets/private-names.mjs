import {tokenizer} from 'acorn';

// Private DOM names shared by all widget capabilities. Protocol keys and text
// keep their names; readable source and fixtures do not need minified selectors.
const names = ['context','content','feedback','server','bounded','footer','pager','button','row','count','hidden','statuses','corner','title','text','body','notice','alert','hint','root','top'];
const dynamic = ['read','prev','next','back','action','nav','danger','selected','wait','uncertain','status'];
export const releaseNames = Object.fromEntries([
  ['r-',''], ...names.map((name,i)=>['r-'+name,String.fromCharCode(97+i)]),
  ...dynamic.map(name=>['r-'+name,name]),
  ['g-',''], ...['top','bottom','start','end'].map(name=>['g-'+name,name]),
  ...['box','stage','axis','gaps','range','line','empty','time'].map((name,i)=>['g-'+name,'G'+i]),
  ...['value','grid','options'].map((name,i)=>['t-'+name,'T'+i])
]);
export const compactRelease = source => source.replace(/\b[rgt]-[a-z-]*/g, name=>releaseNames[name] ?? name);
// Tests exercise the compact artifact while keeping readable selector literals.
export const releaseExpression = source => source.replace(
  /(getElementById|querySelectorAll|querySelector|contains|rect)\((['"])(.*?)\2\)/g,
  (_,method,quote,value)=>method+'('+quote+compactRelease(method==='getElementById'&&value==='release'?'r-root':value)+quote+')'
);
export const mediaExpression = source => releaseExpression(source
  .replaceAll('r-local-prev','r-prev').replaceAll('r-local-next','r-next')
  .replaceAll('r-local-count','r-count').replaceAll('r-footer','r-server'));

// Rename literal DOM names before JS minification. Replacing identifiers in a
// complete HTML artifact would also match arithmetic such as `g-1` and `r-1`.
export function compactScript(source) {
  let result='',previous=0;
  for(const token of tokenizer(source,{ecmaVersion:'latest',sourceType:'module'})) {
    if(token.type.label!=='string')continue;
    const value=compactRelease(token.value);
    if(value===token.value)continue;
    result+=source.slice(previous,token.start)+JSON.stringify(value);
    previous=token.end;
  }
  return result+source.slice(previous);
}
