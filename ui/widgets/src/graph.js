import {validGraph, renderGraph} from './graph-render.js';
// Shared transport compiled into separate immutable graph and range assets.
// Only server-issued keys leave the widget; dropdown navigation is local.
(() => {
  const api=window.TelegramWidget, el=id=>document.getElementById(id), card=el('release');
  let data, busy=false, waiting=false, uncertain=false, denied=!!api?.isForward, local=0, parts=[], timer, requested=-1, reading=false, note='', picker=-1, count=1, motion, frame;
  let lang=navigator.language.startsWith('ru')?'ru':'en';
  const words={ru:['Назад','Обновить','Нет ответа. Обновите.','Доступ закрыт','Откройте /start.','Отмена','Пн Вт Ср Чт Пт Сб Вс','Часы','Минуты'],en:['Back','Refresh','No response.','Access denied','Open /start.','Cancel','Mon Tue Wed Thu Fri Sat Sun','Hours','Minutes']};
  const w=i=>words[lang][i], hide=(id,on)=>el(id).classList.toggle('r-hidden',on);
  const text=(id,value)=>el(id).textContent=value;
  const node=(tag,style,value='')=>{const n=document.createElement(tag);n.className=style;n.textContent=value;return n;};
  const key=s=>typeof s==='string' && /^[A-Za-z0-9_-]{1,28}$/.test(s);
  const number=Number.isSafeInteger, string=s=>typeof s==='string';
  function accept(v) {
    if(!v || v.v!==1 || !string(v.rev) || !/^[a-f0-9]{32}$/.test(v.rev) || !number(v.seq) || v.seq<0
      || !['ru','en'].includes(v.lang) || !(CHART?['chart']:['range','calendar','time']).includes(v.kind)
      || !['title','context','body','notice','alert'].every(k=>string(v[k])) || !key(v.back) || v.read!=='read' || (CHART?!validGraph(v.graph):v.graph!=null)
      || !Array.isArray(v.b) || v.b.length>8 || !v.b.every(r=>Array.isArray(r)&&r.length>0&&r.length<=4&&r.every(b=>Array.isArray(b)&&b.length===3&&string(b[0])&&key(b[1])&&['nav','selected'].includes(b[2])))) return false;
    if(!CHART) {
      const c=v.c;
      if(v.kind==='calendar' ? !Array.isArray(c)||c.length!==5||!c.every(number)||c[0]<0||c[0]>6||c[1]<28||c[1]>31||c[2]<0||c[2]>c[1]||c[3]<1||c[4]>c[1]||c[3]>c[4] : c!=null) return false;
      const t=v.t;
      if(v.kind==='time' ? !Array.isArray(t)||t.length!==2||!t.every(number)||t[0]<0||t[0]>23||t[1]<0||t[1]>59 : t!=null) return false;
      if(data && v.seq<data.seq) return true;
      if(data?.t && t && v.seq>data.seq) {
        const i=t.findIndex((n,i)=>n!==data.t[i]);
        if(i>=0){motion=[i,t[i]>data.t[i]?1:-1,Date.now()];picker=-1;}
      }
      if(data?.s!==v.s) picker=-1;
      if(!data || data.s!==v.s || data.kind!==v.kind || data.title!==v.title) local=c?Math.floor((c[0]+Math.max(1,c[2])-1)/7):0;
    } else if(data&&v.seq<data.seq)return true;
    data=v;lang=v.lang;return true;
  }
  const blocked=k=>denied||!data||!api||busy||waiting||(uncertain&&k!=='read');
  function button(label,k,selected=false,off=false) {
    const b=node('button','r-button',label);
    b.setAttribute('data-key',k); if(selected) b.setAttribute('aria-pressed','true');
    b.disabled=off||blocked(k);
    b.onclick=()=>!off&&send(k);return b;
  }
  function mount(blocks) { el('r-content').replaceChildren(...blocks); }
  function layout() {
    const blocks=[];
    for(const [value,style] of [[data.alert,'r-alert'],[data.notice,''],[data.body,'']])
      for(const part of value.match(/.{60,99}\s|.{1,100}/gsu)||[]) blocks.push(node('p','r-text '+style,part));
    if(!CHART&&data.t) {
      const fields=node('div','r-row');
      data.t.forEach((n,i)=>{
        const b=button(w(7+i)+' ▾','u'+i), value=node('b','t-value',String(n).padStart(2,'0'));
        value.id='t'+i;b.appendChild(value);b.setAttribute('aria-expanded',String(picker===i));b.setAttribute('aria-controls','t-options');
        b.onclick=()=>{if(!blocked('')){picker=picker===i?-1:i;local=Math.floor(n/(card.clientWidth<320?6:12));render();}};
        fields.appendChild(b);
      });blocks.push(fields);
      if(picker>=0) {
        const grid=node('div','t-grid'), size=card.clientWidth<320?6:12, max=picker?60:24;
        grid.id='t-options';
        count=Math.ceil(max/size);local=Math.min(local,count-1);
        for(let n=local*size;n<Math.min(max,(local+1)*size);n++) {
          const b=button(String(n).padStart(2,'0'),(picker?'m':'h')+n,n===data.t[picker]);
          if(n===data.t[picker]) b.onclick=()=>{if(!blocked('')){picker=-1;render();}};
          grid.appendChild(b);
        }
        blocks.push(grid);
      }
    }
    data.b.forEach(row=>{const r=node('div','r-row');row.forEach(b=>r.appendChild(button(b[0],b[1],b[2]==='selected')));blocks.push(r);});
    hide('r-footer',true);
    if(!CHART&&data.t){mount(blocks);if(picker>=0)pager(count);return;}
    if(!CHART&&data.c) {
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
  function pager(total) {
    count=total;
    hide('r-footer',count<=1);text('r-local-count',(local+1)+'/'+count);
    el('r-local-prev').disabled=local===0;el('r-local-next').disabled=local+1>=count;
  }
  function render() {
    const locked=denied||!data||!api, a=api?.contentSafeAreaInset, corners=a&&a.cornerLeft+a.cornerRight;
    card.dataset.kind=locked?'chart':data.kind;
    const corner=CHART&&!locked&&data.b.at(-1)?.length===1&&corners>0
      &&!(a.cornerLeft&&a.cornerRight)&&corners<=card.clientWidth/2&&card.clientWidth-corners>=200;
    if(CHART)card.classList.toggle('r-left',corner&&a.cornerLeft>0);
    card.classList.toggle('r-corner',corner);card.style.paddingBottom=corner?'16px':'';
    text('r-title',locked?w(3):data.title);text('r-context',locked?'':data.context);
    text('r-back','‹ '+w(data?.kind==='range'?5:0));text('r-read',busy||waiting?'…':'↻');el('r-read').setAttribute('aria-label',w(1));
    hide('r-back',locked);hide('r-read',locked);
    el('r-back').disabled=blocked(data?.back);el('r-read').disabled=blocked('read');
    text('r-feedback',locked?w(4):note);hide('r-feedback',!locked&&!note);
    if(CHART)renderGraph(locked?null:data.graph,lang,el,hide);
    const position=local;
    hide('r-footer',true);mount([]);if(!locked) layout();
    // Full-width arrows stay above the clock; only an unpaged graph action
    // may share its free corner. Re-measure once, preserving the chosen part.
    if(CHART&&corner&&!el('r-footer').classList.contains('r-hidden')) {
      card.classList.remove('r-corner','r-left');card.style.paddingBottom='';local=position;layout();
    }
    if(!CHART) {
      cancelAnimationFrame(frame);
      if(!locked&&data.t&&motion) {
        const tick=()=>{
          const p=document.hidden||matchMedia('(prefers-reduced-motion: reduce)').matches?1:Math.min(1,(Date.now()-motion[2])/240);
          el('t'+motion[0]).style.transform='translateY('+(1-p)*(1-p)*12*motion[1]+'px)';
          if(p<1)frame=requestAnimationFrame(tick);
        };tick();
      }
    }
  }
  function stop() {waiting=false;clearTimeout(timer);}
  function close() {
    if(denied)return;
    denied=true;data=null;stop();
    api.request('r1:home').catch(()=>{});
  }
  function complete() {if(reading) uncertain=false;reading=false;stop();note='';}
  function fail() {reading=false;stop();uncertain=true;note=w(2);}
  async function send(k) {
    if(!key(k)||blocked(k)) return;
    const rev=data.rev, seq=data.seq;requested=seq;reading=k==='read';busy=true;note='';render();
    try {
      const answer=JSON.parse((await api.request('r1:'+rev+':'+k)).result);
      if(answer?.denied)close();
      else if(answer?.page){if(!accept(answer.page))throw Error();if(answer.page.seq>seq)complete();else{reading=false;stop();}}
      else if(answer?.accepted){if(data&&data.seq>seq)complete();else{waiting=true;timer=setTimeout(()=>{fail();render();},12000);}}
      else throw Error();
    } catch {fail();}
    busy=false;render();
  }
  function update() {
    if(denied)return;
    try {if(!accept(JSON.parse(api.chatData)))throw Error();if(waiting&&data.seq>requested)complete();}
    catch {close();}render();
  }
  const theme=()=>document.documentElement.setAttribute('data-theme',api?.colorScheme==='dark'?'dark':'light');
  el('r-back').onclick=()=>send(data?.back);el('r-read').onclick=()=>send('read');
  el('r-local-prev').onclick=()=>{if(local>0){local--;render();}};
  el('r-local-next').onclick=()=>{if(local+1<count){local++;render();}};
  if(api&&!denied){api.onEvent('chatDataChanged',update);api.onEvent('contentSafeAreaChanged',render);api.onEvent('themeChanged',()=>{theme();render();});update();api.ready();}
  if(typeof ResizeObserver==='function'){let width=0;new ResizeObserver(()=>{if(width!==card.clientWidth){width=card.clientWidth;render();}}).observe(card);}
  theme();render();
})();
