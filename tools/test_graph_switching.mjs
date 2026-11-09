// Run the shipped loader with controlled, out-of-order responses; no browser needed.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const html=readFileSync(new URL('../crates/gyrfalcon-mcp/src/explorer.html',import.meta.url),'utf8');
const loader=html.slice(html.indexOf('function mapKey('),html.indexOf('\nfunction renderGraph()'));
const elements=new Map(),pending=[];
const context=vm.createContext({AbortController,URLSearchParams,TypeError,
  $:id=>{if(!elements.has(id))elements.set(id,{style:{},hidden:false,textContent:'',replaceChildren(){}});return elements.get(id)},
  fetch:()=>new Promise((resolve,reject)=>pending.push({resolve,reject}))});
vm.runInContext(`let map=null,mapSeq=0,mapAbort=null,mapRoot=0,mapSelected=0,mapKeys=new Map(),mapVisible=[],mapEdges=[],mapEdgesSet=new Set(),mapPositions=[],mapAdj=[],mapParent=[],mapDistance=[],mapGrid=new Map(),mapIncoming=[],mapOutgoing=[];
let painted=null;
function paintMap(){painted=map?.nodes[map.root][0]??null}
function renderGraph(){paintMap()}
function searchMap(){}
${loader}`,context);
const value=expression=>vm.runInContext(expression,context);
const item=field=>({kind:'object',category:'Catalogs',name:'ВариантыОтчетов',field});
const key=field=>`field:object:Catalogs:вариантыотчетов::${field.toLowerCase()}`;
const data=field=>({root:0,nodes:[[key(field)]],edges:[]});
function start(field,force=false){context.nextItem=item(field);context.force=force;return value('loadGraph(nextItem,force)')}
function respond(request,field){request.resolve({ok:true,json:async()=>data(field)})}
async function seed(field){const promise=start(field);respond(pending.shift(),field);await promise;assert.equal(value('painted'),key(field))}

await seed('Автор');
let promise=start('КлючевыеСлова');
assert.equal(value('painted'),null,'old image must clear immediately when the root changes');
assert.equal(elements.get('nodeCount').textContent,'0 узлов');
pending.shift().reject(new TypeError('Failed to fetch'));
await promise;
assert.equal(value('map'),null,'failed switch must not restore another root');
assert.match(elements.get('mapStatus').textContent,/Нет связи/);

await seed('Автор');
const slow=start('КлючевыеСлова'),slowRequest=pending.shift();
const fast=start('Контекст'),fastRequest=pending.shift();
respond(fastRequest,'Контекст');await fast;
respond(slowRequest,'КлючевыеСлова');await slow;
assert.equal(value('painted'),key('Контекст'),'late response must not replace current selection');

promise=start('Контекст',true);
pending.shift().reject(new TypeError('Failed to fetch'));await promise;
assert.equal(value('painted'),key('Контекст'),'refresh may preserve the same root');
assert.match(elements.get('mapStatus').textContent,/текущего узла сохранена/);

promise=start('Автор');
respond(pending.shift(),'Контекст');await promise;
assert.equal(value('map'),null,'wrong-root response must be rejected');
assert.match(elements.get('mapStatus').textContent,/другого узла/);

promise=start('КлючевыеСлова');
const cancelled=pending.shift();value('mapAbort.abort()');
cancelled.reject(Object.assign(new Error('cancelled'),{name:'AbortError'}));await promise;
assert.equal(value('painted'),null,'cancelled switch must keep the canvas empty');
assert.equal(elements.get('cancelGraph').hidden,true);
console.log('OK: failed switch clears image; late response ignored; same-root refresh preserved; wrong-root response rejected; cancelled switch stays empty');
