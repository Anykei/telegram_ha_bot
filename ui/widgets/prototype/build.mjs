// A single state machine, DOM shell and stylesheet; no renderer adapters.
import {readFile,writeFile,mkdir,rm} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {compileWidget} from '../compile-widget.mjs';
import {embed} from '../preview/embed.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const read=file=>readFile(root+file,'utf8');
const {html,manifest:compiled}=await compileWidget('unified');
const manifest={...compiled,sha256:createHash('sha256').update(html).digest('hex'),
  fitsTelegram:compiled.bytes<=32768&&compiled.zlibBytes<=4096,production:false,liveIntegrationVerified:false};
await mkdir(root+'prototype/dist',{recursive:true});
await writeFile(root+'prototype/dist/unified.html',html);
await writeFile(root+'prototype/dist/manifest.json',JSON.stringify(manifest,null,2)+'\n');
await rm(root+'prototype/dist/browser-report.json',{force:true});
const fixtures = [...JSON.parse(await read('preview/release-fixtures.json')), ...JSON.parse(await read('preview/graph-fixtures.json'))];
// The production fixtures were split across previews; connect the existing
// room history button to the graph fixture without changing the card itself.
for (const fixture of fixtures) if (['room','room_en'].includes(fixture.id)) {
  fixture.links = {...fixture.links, sensor:'numeric'+(fixture.page.lang==='en'?'_en':'')};
}
// Reuse the current mock and fixtures; cross-template buttons now have targets
// in this one preview, with no second iframe or document reload in the widget.
const preview = embed(await read('preview/release.html'), {PAYLOAD:html,FIXTURES:fixtures,MANIFEST:manifest});
await writeFile(root+'prototype/dist/unified-preview.html', preview.replaceAll('../../../docks/', '../../../../docks/').replace('Предпросмотр сообщения', 'Прототип · один документ').replace('Пример данных · реальный дом не подключён',
  'Прототип · '+(manifest.fitsTelegram?'лимит Telegram соблюдён':'превышает лимит Telegram, только браузерная проверка')));
console.log(JSON.stringify(manifest));
