let spec,draft,locked,rail,paint,drag;
export const sliderValue=()=>draft;
export function validSlider(s) {
  return s&&['min','max','step','value'].every(k=>Number.isSafeInteger(s[k])&&Math.abs(s[k])<=2147483648)
    &&s.min<s.max&&s.step>0&&s.step<=s.max-s.min&&s.value>=s.min&&s.value<=s.max&&typeof s.unit==='string';
}
export function sliderSync(s) {
  if(JSON.stringify(s)!==JSON.stringify(spec)){draft=s.value;drag=null;}
  spec=s;
}
export function sliderLock(value) {
  locked=value;
  if(rail)rail.setAttribute('aria-disabled',value);
  if(value)drag=null;
}
export function renderSlider(node,s,lang,disabled,changed) {
  sliderSync(s);
  const box=node('div','s-editor'),value=node('strong','s-value'),confirmed=node('p','s-label'),
    scale=node('div','s-scale'),fill=node('div','s-fill'),thumb=node('div','s-thumb'),mark=node('div','s-mark');
  rail=node('div','s-rail');rail.setAttribute('role','slider');rail.setAttribute('aria-label',lang==='ru'?'Температура':'Temperature');
  rail.setAttribute('aria-valuemin',s.min/1000);rail.setAttribute('aria-valuemax',s.max/1000);
  const format=n=>n/1000+s.unit;
  confirmed.textContent=(lang==='ru'?'Задано в HA: ':'Confirmed: ')+format(s.value);
  mark.style.left=(s.value-s.min)/(s.max-s.min)*100+'%';
  rail.append(node('div','s-track'),fill,mark,thumb);
  scale.append(node('span','',format(s.min)),node('span','',format(s.max)));
  paint=()=>{value.textContent=(lang==='ru'?'Цель: ':'Target: ')+format(draft);rail.setAttribute('aria-valuenow',draft/1000);
    const p=(draft-s.min)/(s.max-s.min)*100+'%';fill.style.width=p;thumb.style.left=p;};
  const set=n=>{draft=s.min+Math.max(0,Math.min(Math.floor((s.max-s.min)/s.step),Math.round((n-s.min)/s.step)))*s.step;paint();changed();};
  const move=e=>{if(drag&&e.pointerId===drag.id&&!locked)set(s.min+(e.clientX-drag.left)/drag.width*(s.max-s.min));};
  rail.onpointerdown=e=>{if(locked||drag||e.button>0)return;const r=rail.getBoundingClientRect();
    drag={id:e.pointerId,left:r.left,width:r.width,old:draft};rail.setPointerCapture(e.pointerId);move(e);};
  rail.onpointermove=move;
  rail.onpointerup=e=>{if(drag&&e.pointerId===drag.id){move(e);drag=null;}};
  rail.onpointercancel=e=>{if(drag&&e.pointerId===drag.id){draft=drag.old;drag=null;paint();changed();}};
  box.append(value,confirmed,rail,scale);paint();sliderLock(disabled);return box;
}
