import {validGraph, renderGraph} from './graph-render.js';
// One immutable chart/calendar asset. Only server-issued keys leave the widget.
(() => {
  const api=window.TelegramWidget, el=id=>document.getElementById(id), card=el('release');
  let data, busy=false, waiting=false, uncertain=false, denied=!!api?.isForward, local=0, parts=[], timer, requested=-1, reading=false, note='';
  let lang=navigator.language.startsWith('ru')?'ru':'en';
  const words={ru:['Назад','Обновить','Нет ответа. Обновите.','Доступ закрыт','Откройте /start.','Отмена','Пн Вт Ср Чт Пт Сб Вс'],en:['Back','Refresh','No response.','Access denied','Open /start.','Cancel','Mon Tue Wed Thu Fri Sat Sun']};
  const w=i=>words[lang][i], hide=(id,on)=>el(id).classList.toggle('r-hidden',on);
  const text=(id,value)=>el(id).textContent=value;
  const node=(tag,style,value='')=>{const n=document.createElement(tag);n.className=style;n.textContent=value;return n;};
  const key=s=>typeof s==='string' && /^[A-Za-z0-9_-]{1,28}$/.test(s);
  const number=Number.isSafeInteger, string=s=>typeof s==='string';
  function accept(v) {
    if(!v || v.v!==1 || !string(v.rev) || !/^[a-f0-9]{32}$/.test(v.rev) || !number(v.seq) || v.seq<0
      || !['ru','en'].includes(v.lang) || !['chart','range','calendar','hours','minutes'].includes(v.kind)
      || !['title','context','body','notice','alert'].every(k=>string(v[k])) || !key(v.back) || v.read!=='read' || !validGraph(v.graph)
      || !Array.isArray(v.b) || v.b.length>8 || !v.b.every(r=>Array.isArray(r)&&r.length>0&&r.length<=4&&r.every(b=>Array.isArray(b)&&b.length===3&&string(b[0])&&key(b[1])&&['nav','selected'].includes(b[2])))) return false;
    const c=v.c;
    if(v.kind==='calendar' ? !Array.isArray(c)||c.length!==5||!c.every(number)||c[0]<0||c[0]>6||c[1]<28||c[1]>31||c[2]<0||c[2]>c[1]||c[3]<1||c[4]>c[1]||c[3]>c[4] : c!=null) return false;
    if(data && v.seq<data.seq) return true;
    if(!data || data.s!==v.s || data.kind!==v.kind || data.title!==v.title) local=c?Math.floor((c[0]+Math.max(1,c[2])-1)/7):0;
    data=v;lang=v.lang;return true;
  }
  const blocked=k=>denied||!data||!api||busy||waiting||(uncertain&&k!=='read');
  function button(label,k,selected=false,off=false) {
    const b=node('button','r-button',label);
    b.setAttribute('data-key',k); if(selected) b.setAttribute('aria-pressed','true');
    b.disabled=off||blocked(k);
    b.addEventListener('click',()=>!off&&send(k));return b;
  }
  function mount(blocks) { el('r-content').replaceChildren(...blocks); }
  function layout() {
    const blocks=[];
    for(const [value,style] of [[data.alert,'r-alert'],[data.notice,''],[data.body,'']])
      for(const part of value.match(/.{60,99}\s|.{1,100}/gsu)||[]) blocks.push(node('p','r-text '+style,part));
    data.b.forEach(row=>{const r=node('div','r-row');row.forEach(b=>r.appendChild(button(b[0],b[1],b[2]==='selected')));blocks.push(r);});
    hide('r-footer',true);
    if(data.c) {
      const [offset,days,selected,first,last]=data.c, narrow=card.clientWidth<320;
      const count=Math.ceil((offset+days)/7);local=Math.min(local,count-1);
      const grid=node('div','g-days'), from=narrow?local*7:0, to=narrow?(local+1)*7:offset+days;
      for(let i=from;i<to;i++) {const d=i-offset+1;grid.appendChild(d<1||d>days?node('span',''):button(w(6).split(' ')[i%7]+'\n'+d,'d'+d,d===selected,d<first||d>last));}
      blocks.push(grid);mount(blocks);
      if(narrow) pager(count);
      return;
    }
    mount(blocks);parts=[blocks];
    if(card.scrollHeight>600) {
      hide('r-footer',false);parts=[];let part=[];
      for(const b of blocks) {part.push(b);mount(part);if(card.scrollHeight>600&&part.length>1){part.pop();parts.push(part);part=[b];}}
      parts.push(part);local=Math.min(local,parts.length-1);mount(parts[local]);pager(parts.length);
    } else local=0;
  }
  function pager(count) {
    hide('r-footer',count<=1);text('r-local-count',(local+1)+'/'+count);
    el('r-local-prev').disabled=local===0;el('r-local-next').disabled=local+1>=count;
  }
  function render() {
    const locked=denied||!data||!api, a=api?.contentSafeAreaInset, corners=a&&a.cornerLeft+a.cornerRight;
    card.dataset.kind=locked?'chart':data.kind;
    const corner=!locked&&data.kind==='chart'&&corners>0&&card.clientWidth-corners>=200;
    card.classList.toggle('r-corner',corner);card.style.paddingBottom=corner?'16px':'';
    text('r-title',locked?w(3):data.title);text('r-context',locked?'':data.context);
    text('r-back','‹ '+w(data?.kind==='range'?5:0));text('r-read',busy||waiting?'…':'↻');el('r-read').setAttribute('aria-label',w(1));
    hide('r-back',locked);hide('r-read',locked);
    el('r-back').disabled=blocked(data?.back);el('r-read').disabled=blocked('read');
    text('r-feedback',locked?w(4):note);hide('r-feedback',!locked&&!note);
    renderGraph(locked?null:data.graph,lang,el,hide);
    hide('r-footer',true);mount([]);if(!locked) layout();
  }
  function stop() {waiting=false;clearTimeout(timer);}
  function complete() {if(reading) uncertain=false;reading=false;stop();note='';}
  function fail() {reading=false;stop();uncertain=true;note=w(2);}
  async function send(k) {
    if(!key(k)||blocked(k)) return;
    const rev=data.rev, seq=data.seq;requested=seq;reading=k==='read';busy=true;note='';render();
    try {
      const answer=JSON.parse((await api.request('r1:'+rev+':'+k)).result);
      if(answer?.denied){denied=true;data=null;stop();}
      else if(answer?.page){if(!accept(answer.page))throw Error();if(answer.page.seq>seq)complete();else{reading=false;stop();}}
      else if(answer?.accepted){if(data&&data.seq>seq)complete();else{waiting=true;timer=setTimeout(()=>{fail();render();},12000);}}
      else throw Error();
    } catch {fail();}
    busy=false;render();
  }
  function update() {
    if(denied)return;
    try {if(!accept(JSON.parse(api.chatData)))throw Error();if(waiting&&data.seq>requested)complete();}
    catch {denied=true;data=null;stop();}render();
  }
  const theme=()=>document.documentElement.setAttribute('data-theme',api?.colorScheme==='dark'?'dark':'light');
  el('r-back').onclick=()=>send(data?.back);el('r-read').onclick=()=>send('read');
  el('r-local-prev').onclick=()=>{if(local>0){local--;render();}};
  el('r-local-next').onclick=()=>{if(local+1<(data?.c?Math.ceil((data.c[0]+data.c[1])/7):parts.length)){local++;render();}};
  if(api&&!denied){api.onEvent('chatDataChanged',update);api.onEvent('contentSafeAreaChanged',render);api.onEvent('themeChanged',()=>{theme();render();});update();api.ready();}
  if(typeof ResizeObserver==='function'){let width=0;new ResizeObserver(()=>{if(width!==card.clientWidth){width=card.clientWidth;render();}}).observe(card);}
  theme();render();
})();
