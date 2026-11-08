import {validGraph, renderGraph} from './graph-render.js';

// Compile-time capabilities produce the shipping subsets and the unified build
// from this single state machine. Screen changes never install new listeners.
(() => {
  const api=window.TelegramWidget, el=id=>document.getElementById(id), card=el('r-root');
  const words={
    ru:['Назад',MENU?'Далее':'','Обновить','Нет ответа. Обновите.','Доступ закрыт','/start',MENU||RANGE?'Отмена':'',...(RANGE?['Пн Вт Ср Чт Пт Сб Вс','Часы','Минуты']:[])],
    en:['Back',MENU?'Next':'','Refresh',MENU?'No response. Refresh.':'No response.','Access denied','/start',MENU||RANGE?'Cancel':'',...(RANGE?['Mon Tue Wed Thu Fri Sat Sun','Hours','Minutes']:[])]
  };
  const mediaKinds=['chart','range','calendar','time'];
  let data, lang=navigator.language.startsWith('ru')?'ru':'en', lastKind=MENU?'menu':CHART?'chart':'range';
  let busy=false, waiting=false, uncertain=false, denied=!api||api.isForward;
  let local=0, deck=[], count=0, picker=-1, motion, frame, painted=0;
  let timer, requested=-1, reading=false, activeKey='', note='', tail=false, ticket=0, closedSeq=-1;
  const media=v=>MEDIA&&(!MENU||mediaKinds.includes(v?v.kind:lastKind));
  const chart=v=>CHART&&(!MENU&&!RANGE||(v?v.kind:lastKind)==='chart');
  const w=i=>words[lang][i], text=(id,value)=>el(id).textContent=value;
  const hide=(id,on)=>el(id).hidden=on;
  const node=(tag,style='',value='')=>{const n=document.createElement(tag);n.className=style;n.textContent=value;return n;};
  const string=v=>typeof v==='string', integer=Number.isSafeInteger;
  const list=(v,max,test)=>Array.isArray(v)&&v.length<=max&&v.every(test);
  const key=v=>string(v)&&/^[\w-]{1,28}$/.test(v);
  const blocked=k=>denied||!data||busy||waiting||(uncertain&&k!=='read');
  const external=i=>data[i?'next':'prev']||data.q?.[i+1];
  function valid(v) {
    if(!v||v.v!==1||!string(v.rev)||!/^[a-f0-9]{32}$/.test(v.rev)||!integer(v.seq)||v.seq<0
      ||!['ru','en'].includes(v.lang)||!['title','context','body','notice','alert'].every(k=>string(v[k]))
      ||v.read!=='read'||!list(v.rows,media(v)?8:64,row=>list(row,media(v)?4:2,
        b=>Array.isArray(b)&&b.length===3&&string(b[0])&&key(b[1])&&(media(v)?['nav','selected']:['action','nav','danger','selected','wait','uncertain','status']).includes(b[2]))&&row.length))return false;
    if(MEDIA&&media(v)) {
      if(!(chart(v)?v.kind==='chart':RANGE&&['range','calendar','time'].includes(v.kind))||!key(v.back)
        ||(chart(v)?!validGraph(v.graph):v.graph!=null))return false;
      if(RANGE&&!chart(v)) {
        const c=v.c,t=v.t;
        if(v.kind==='calendar'?!list(c,5,integer)||c.length!==5||c[0]<0||c[0]>6||c[1]<28||c[1]>31||c[2]<0||c[2]>c[1]||c[3]<1||c[4]>c[1]||c[3]>c[4]:c!=null)return false;
        if(v.kind==='time'?!list(t,2,integer)||t.length!==2||t[0]<0||t[0]>23||t[1]<0||t[1]>59:t!=null)return false;
      }
    } else if(MENU) {
      if(v.kind!==undefined&&!string(v.kind)||v.status!==undefined&&!list(v.status,2,s=>list(s,3,string)&&s.length===3)
        ||v.hint!==undefined&&!string(v.hint)||v.q!==undefined&&(!list(v.q,3,(k,i)=>i?k===null||key(k):string(k))||v.q.length!==3)
        ||v.l!==undefined&&v.l!==true||v.back!=null&&!key(v.back)||![v.prev,v.next].every(k=>k===null||key(k))
        ||!integer(v.page)||!integer(v.pages)||v.page<=0||v.page>v.pages)return false;
    }
    return true;
  }
  function accept(v) {
    if(v?.b!==undefined) {
      if(v.rows!==undefined)return false;
      v.rows=v.b;
    }
    else if(MENU&&Array.isArray(v?.rows))v.rows=v.rows.map(row=>Array.isArray(row)?row.map(b=>[b?.text,b?.key,b?.kind]):row);
    if(!valid(v))return false;
    if(data&&v.seq<data.seq)return true;
    if(UNIFIED&&data&&(media(v)!==media(data)||chart(v)!==chart(data))) {
      ++ticket;stop();busy=false;uncertain=false;reading=false;note='';activeKey='';
      local=0;picker=-1;motion=null;tail=false;
    }
    if(RANGE&&media(v)&&!chart(v)) {
      if(data?.t&&v.t&&v.seq>data.seq) {
        const i=v.t.findIndex((n,i)=>n!==data.t[i]);
        if(i>=0){motion=[i,v.t[i]>data.t[i]?1:-1,Date.now()];picker=-1;}
      }
      if(data?.s!==v.s)picker=-1;
      if(!data||data.s!==v.s||data.kind!==v.kind||data.title!==v.title)local=v.c?Math.floor((v.c[0]+Math.max(1,v.c[2])-1)/7):0;
    } else if(MENU&&!media(v)&&(!data||!v.s||v.s!==data.s||v.page!==data.page)) {
      local=tail?9999:0;tail=false;
    }
    data=v;lang=v.lang;if(UNIFIED)lastKind=v.kind||'menu';return true;
  }
  function disable(button,value) {
    if(MEDIA&&media(data))button.disabled=value;
    else button.setAttribute('aria-disabled',value);
    if(UNIFIED) {
      if(media(data))button.removeAttribute('aria-disabled');
      else button.disabled=false;
    }
  }
  function action(b,off=false) {
    const awaiting=MENU&&!media(data)&&(b[2]==='wait'||b[1]===activeKey&&(busy||waiting));
    const unavailable=RANGE&&off||MENU&&!media(data)&&(awaiting||b[2]==='uncertain');
    const button=node('button','r-button'+(media(data)?'':' r-'+b[2]+(awaiting?' r-wait':'')));
    button.dataset.key=b[1];
    if(b[2]==='selected')button.setAttribute('aria-pressed','true');
    if(MEDIA&&media(data))button.textContent=b[0];
    else {
      button.setAttribute('aria-busy',awaiting);
      button.append(node('span','',b[0]));
      if(awaiting)button.append(node('i'));
    }
    disable(button,unavailable||blocked(b[1]));
    button.onclick=()=>!unavailable&&send(b[1]);return button;
  }
  const choice=(label,k,selected=false,off=false)=>action([label,k,selected?'selected':'nav'],off);
  function mount(nodes) {
    const content=el('r-content');content.textContent='';let previous;
    nodes.forEach(block=>{
      const paragraph=MENU&&!media(data)&&block.tagName==='P';
      if(!paragraph||!previous||previous.className!==block.className) {
        previous=paragraph?node('p',block.className):block;content.append(previous);
      }
      if(paragraph)previous.textContent+=block.textContent;
    });
  }
  function pager(reserve) {
    if(MEDIA&&media(data)) {
      hide('r-server',count<=1);text('r-count',(local+1)+'/'+count);
    } else {
      count=deck.length;
      hide('r-server',!data.q&&!reserve&&count<=1&&data.pages<=1);
      text('r-count',(data.q?data.q[0]+'\n':'')+data.page+'.'+(local+1));
    }
    ['prev','next'].forEach((key,i)=>disable(el('r-'+key),
      (i?local===count-1:!local)&&(media(data)||!external(i))||!media(data)&&blocked('')));
  }
  function layout(reserve=false) {
    const blocks=[];
    let statuses;
    const paragraphs=[[data.alert,'r-alert'],[data.notice,media(data)?'':'r-notice'],[data.body,media(data)?'':'r-body']];
    if(MENU&&!media(data)) {
      paragraphs.push([data.hint||'','r-hint']);
      for(const id of ['r-title','r-context'])if(el(id).scrollWidth>el(id).clientWidth)paragraphs.unshift([el(id).textContent,id]);
    }
    for(const [value,style] of paragraphs)for(const part of value.match(media(data)?/.{60,99}\s|.{1,100}/gsu:/.{20,39}\s|.{1,40}/gsu)||[])blocks.push(node('p','r-text '+style,part));
    if(MENU&&!media(data)&&data.status?.length) {
      statuses=node('div','r-statuses');
      data.status.forEach(status=>{
        const tile=node('div','r-button r-status');
        status.forEach((value,i)=>tile.append(node(['span','strong','small'][i],'',value)));
        statuses.append(tile);
      });blocks.push(statuses);
    }
    if(RANGE&&media(data)&&data.t) {
      const fields=node('div','r-row');
      data.t.forEach((n,i)=>{
        const b=choice(w(8+i)+' ▾','u'+i),value=node('b','t-value',('0'+n).slice(-2));
        value.id='t'+i;b.append(value);b.setAttribute('aria-expanded',picker===i);b.setAttribute('aria-controls','t-options');
        b.onclick=()=>{if(!blocked('')){picker=picker===i?-1:i;local=Math.floor(n/(card.clientWidth<320?6:12));render();}};
        fields.append(b);
      });blocks.push(fields);
      if(picker>=0) {
        const grid=node('div','t-grid'),size=card.clientWidth<320?6:12,max=picker?60:24;
        grid.id='t-options';count=Math.ceil(max/size);local=Math.min(local,count-1);
        for(let n=local*size;n<Math.min(max,(local+1)*size);n++) {
          const b=choice(('0'+n).slice(-2),(picker?'m':'h')+n,n===data.t[picker]);
          if(n===data.t[picker])b.onclick=()=>{if(!blocked('')){picker=-1;render();}};
          grid.append(b);
        }blocks.push(grid);
      }
    }
    data.rows.forEach(row=>{
      if(MENU&&!media(data))row=row.filter(b=>b[1]!==data.back);
      if(!row.length)return;
      if(MENU&&statuses&&row[0][2]==='status'){row.forEach(b=>statuses.append(action(b)));return;}
      const group=node('div','r-row');row.forEach(b=>group.append(action(b)));blocks.push(group);
    });
    if(MEDIA&&media(data))hide('r-server',true);
    if(RANGE&&media(data)&&data.t){mount(blocks);if(picker>=0)pager();return;}
    if(RANGE&&media(data)&&data.c) {
      const [offset,days,selected,first,last]=data.c,narrow=card.clientWidth<320;
      count=Math.ceil((offset+days)/7);local=Math.min(local,count-1);
      const grid=node('div','g-days'),from=narrow?local*7:0,to=narrow?(local+1)*7:offset+days;
      for(let i=from;i<to;i++) {
        const d=i-offset+1;grid.append(d<1||d>days?node('span'):choice(w(7).split(' ')[i%7]+'\n'+d,'d'+d,d===selected,d<first||d>last));
      }
      blocks.push(grid);mount(blocks);if(narrow)pager();return;
    }
    if(MEDIA&&media(data)) {
      mount(blocks);deck=[blocks];
      if(card.scrollHeight<=600){local=0;return;}
      reserve=true;
    }
    deck=[];let part=[];
    if(media(data))hide('r-server',false);else pager(reserve);
    for(const block of blocks) {
      part.push(block);mount(part);
      if(card.scrollHeight>600&&(!media(data)||part.length>1)) {
        if(!reserve)return layout(true);
        part.pop();if(part.length)deck.push(part);part=[block];mount(part);
        if(!media(data)&&card.scrollHeight>600)return true;
      }
    }
    deck.push(part);count=deck.length;local=Math.min(local,count-1);mount(deck[local]);pager(reserve);
  }
  function inset(corner) {
    card.classList.toggle('r-corner',corner);
    card.style.paddingBottom=(corner?16:Math.max(16,api?.contentSafeAreaInset?.bottom||0))+'px';
  }
  function render() {
    card.style.height='';
    const locked=denied||!data,a=api?.contentSafeAreaInset,corners=a&&a.cornerLeft+a.cornerRight;
    if(UNIFIED)document.documentElement.dataset.mode=media(data)?'media':'menu';
    card.dataset.kind=locked?media(data)?'chart':'menu':data.kind||'menu';
    if(MENU)card.classList.toggle('r-bounded',!locked&&!media(data)&&(data.l||data.kind==='room'));
    text('r-context',locked?'':data.context);text('r-title',locked?w(4):data.title);
    text('r-read',busy||waiting?'…':'↻');el('r-read').setAttribute('aria-label',w(2));
    text('r-back','‹ '+w((RANGE&&data?.kind==='range'||MENU&&['form','confirmation'].includes(data?.kind))?6:0));
    ['read','back'].forEach(k=>{
      const id='r-'+k,button=el(id),missing=locked||!data[k];
      hide(id,missing);disable(button,missing||blocked(data[k]));
      button.onclick=()=>data&&send(data[k]);
    });
    const feedback=locked?w(5):note||(!media(data)&&data.rows.flat().some(b=>b[2]==='uncertain')?w(3):'');
    text('r-feedback',feedback);hide('r-feedback',!feedback);
    const corner=!feedback&&corners>0&&card.clientWidth-corners>=200;
    const png=CHART&&chart(data)&&!locked&&data.rows.at(-1)?.length===1&&corners>0&&!(a.cornerLeft&&a.cornerRight)&&corners<=card.clientWidth/2&&card.clientWidth-corners>=200;
    if(CHART)card.classList.toggle('r-left',png&&a.cornerLeft>0);
    inset(png);
    if(CHART)renderGraph(locked||!chart(data)?null:data.graph,lang,el,hide);
    if(MENU&&!media(data))['r-prev','r-next'].forEach((id,i)=>el(id).setAttribute('aria-label',w(i)));
    hide('r-server',true);mount([]);
    if(!locked) {
      const position=local;let overflow=layout();
      if(!el('r-server').hidden&&(media(data)||corner)) {
        inset(corner);if(CHART&&media(data)&&!corner)card.classList.remove('r-left');
        local=position;overflow=layout(!media(data));
      }
      if(MENU&&!media(data)&&overflow){mount([]);hide('r-server',true);text('r-feedback',w(5));hide('r-feedback',false);}
    }
    // Keep the menu's intrinsic denied layout and media's fixed-height contract.
    if(!locked||media(data))card.style.height='600px';
    if(RANGE&&media(data)){cancelAnimationFrame(frame);frame=0;}
    if(RANGE&&!locked&&media(data)&&data.t&&motion)animate(0);
    else if(MENU&&!media(data)&&!frame)frame=requestAnimationFrame(animate);
  }
  function animate(now) {
    frame=0;
    if(RANGE&&media(data)&&data?.t&&motion) {
      const p=document.hidden||matchMedia('(prefers-reduced-motion: reduce)').matches?1:Math.min(1,(Date.now()-motion[2])/240);
      el('t'+motion[0]).style.transform='translateY('+(1-p)*(1-p)*12*motion[1]+'px)';
      if(p>=1)return;
    } else if(MENU&&!media(data)) {
      const rings=card.querySelectorAll('.r-wait i');if(!rings.length)return;
      if(now-painted>=32&&!matchMedia('(prefers-reduced-motion:reduce)').matches){rings.forEach(r=>{r.style.transform='rotate('+now%1000*.36+'deg)';});painted=now;}
    } else return;
    frame=requestAnimationFrame(animate);
  }
  function stop(){waiting=false;clearTimeout(timer);}
  function complete(){if(reading)uncertain=false;reading=false;stop();note='';}
  function fail(){reading=false;if(MENU)tail=false;stop();uncertain=true;note=w(3);}
  function recover(page) {
    if(UNIFIED&&!api.isForward&&page?.kind==='home'&&page.seq>closedSeq&&accept(page)) {
      denied=false;busy=false;uncertain=false;note='';render();
    }
  }
  function close() {
    if(denied)return;
    if(UNIFIED){closedSeq=data?.seq??-1;++ticket;}
    denied=true;data=null;stop();
    let home=api.request('r1:home');
    if(UNIFIED)home=home.then(result=>recover(JSON.parse(result.result).page));
    home.catch(()=>{});
  }
  async function send(k,last=false) {
    if(!key(k)||blocked(k))return;
    const rev=data.rev,seq=data.seq,id=UNIFIED?++ticket:0;
    requested=seq;reading=k==='read';if(MENU){activeKey=k;tail=last;}busy=true;note='';render();
    try {
      const answer=JSON.parse((await api.request('r1:'+rev+':'+k)).result);
      if(UNIFIED&&id!==ticket)return;
      if(answer?.denied)close();
      else if(answer?.page) {
        if(!accept(answer.page))throw Error();
        if(answer.page.seq>seq)complete();else{reading=false;stop();if(!media(data))note=uncertain?w(3):'';}
      } else if(answer?.accepted) {
        if(data&&data.seq>seq)complete();else{waiting=true;timer=setTimeout(()=>{fail();render();},12000);}
      } else throw Error();
    } catch {if(UNIFIED&&id!==ticket)return;fail();}
    busy=false;render();
  }
  function update() {
    if(denied){if(UNIFIED)try{recover(JSON.parse(api.chatData));}catch{}return;}
    try {if(!accept(JSON.parse(api.chatData)))throw Error();if(waiting&&data.seq>requested)complete();}catch{close();}
    render();
  }
  const theme=()=>document.documentElement.dataset.theme=api?.colorScheme==='dark'?'dark':'light';
  ['prev','next'].forEach((key,i)=>{el('r-'+key).onclick=()=>{
    if(!media(data)&&blocked(''))return;
    const next=local+(i?1:-1);
    if(next>=0&&next<count){local=next;render();}else if(!media(data))send(external(i),!i);
  };});
  theme();
  if(api&&!denied){api.onEvent('chatDataChanged',update);api.onEvent('contentSafeAreaChanged',render);api.onEvent('themeChanged',()=>{theme();render();});update();api.ready();}
  else render();
  if(typeof ResizeObserver==='function'){let width=card.clientWidth;new ResizeObserver(()=>{if(width!==card.clientWidth){width=card.clientWidth;render();}}).observe(card);}
})();
