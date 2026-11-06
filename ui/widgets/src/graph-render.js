const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

export function validGraph(g) {
  return g == null || ['plot','top','bottom','start','end'].every(k => typeof g[k] === 'string')
    && /^(?:[A-Za-z0-9_-]{3}|\.{3}){32}$/.test(g.plot)
    && g.plot.match(/.../g).every(t => t === '...' || alphabet.indexOf(t[0]) <= alphabet.indexOf(t[2]) && alphabet.indexOf(t[2]) <= alphabet.indexOf(t[1]));
}

export function renderGraph(g, lang, el, hide) {
  hide('g-box', !g);
  for (const k of ['top','bottom','start','end']) el('g-'+k).textContent = g ? g[k] : '';
  let range = '', line = '', gaps = '', previous = false;
  if (g) g.plot.match(/.../g).forEach((t, i) => {
    const x = i * 10;
    if (t === '...') { gaps += `M${x} 4h10v132h-10z`; previous = false; return; }
    const [low, high, last] = Array.from(t, c => 136 - alphabet.indexOf(c) * 132 / 63);
    range += `M${x+5} ${low}V${high}`;
    line += (previous ? `V${last}` : `M${x} ${last}`) + `H${x+10}`;
    previous = true;
  });
  el('g-range').setAttribute('d', range);
  el('g-line').setAttribute('d', line);
  el('g-gaps').setAttribute('d', gaps);
  hide('g-stage', !line); hide('g-empty', !!line);
  el('g-empty').textContent = g ? lang === 'ru' ? 'Недостаточно данных' : 'Not enough data' : '';
}
