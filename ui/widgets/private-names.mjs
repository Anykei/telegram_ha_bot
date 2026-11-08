// Private release DOM names only. Protocol keys, text and accessibility fields
// keep their names. Graph/range and the readable source files are unchanged.
const names = ['context','content','feedback','server','bounded','footer','pager','button','row','count','hidden','statuses','corner','title','text','body','notice','alert','hint','root'];
const dynamic = ['read','prev','next','back','action','nav','danger','selected','wait','uncertain','status'];
export const releaseNames = Object.fromEntries([
  ['r-',''], ...names.map((name,i)=>['r-'+name,String.fromCharCode(97+i)]),
  ...dynamic.map(name=>['r-'+name,name])
]);
export const compactRelease = source => source.replace(/\br-[a-z-]*/g, name=>releaseNames[name] ?? name);
// Tests exercise the compact artifact while keeping readable selector literals.
export const releaseExpression = source => source.replace(
  /(getElementById|querySelectorAll|querySelector|contains)\((['"])(.*?)\2\)/g,
  (_,method,quote,value)=>method+'('+quote+compactRelease(method==='getElementById'&&value==='release'?'r-root':value)+quote+')'
);
