import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html=readFileSync(new URL('../crates/gyrfalcon-mcp/src/explorer.html',import.meta.url),'utf8');
const declaration=html.slice(html.indexOf('let map=null'),html.indexOf('\nconst nodeKinds'));
const render=html.slice(html.indexOf('function renderGraph(){'),html.indexOf('\nfunction applyView()'));
const label=html.slice(html.indexOf('function graphLabel('),html.indexOf('\nfunction showGroup('));
const elements=new Map();
const context=vm.createContext({$:id=>{if(!elements.has(id))elements.set(id,{checked:true,style:{},value:'all'});return elements.get(id)}});
const nodes=[
 ['object:Catalogs:а',0,'А','Catalogs',null,0,null,'',0,0,0,'Catalogs.А',null,null],
 ['field:object:Catalogs:а::поле1',4,'Поле1','Catalogs',null,0,0,'Поле1',1,null,1,'Catalogs.А.Поле1','object:Catalogs:а','Catalogs.А'],
 ['field:object:Catalogs:а::поле2',4,'Поле2','Catalogs',null,0,0,'Поле2',1,null,1,'Catalogs.А.Поле2','object:Catalogs:а','Catalogs.А'],
 ['field:object:Catalogs:б::поле1',4,'Поле1','Catalogs',null,0,null,'Поле1',1,null,1,'Catalogs.Б.Поле1','object:Catalogs:б','Catalogs.Б']
];
context.scene={root:0,nodes,edges:[[0,1,0,null,0,1],[0,2,0,null,0,1],[0,3,0,null,0,1]],kinds:[['contains_field','Поле','metadata','structure']],warnings:[],unresolved_calls_in_index:0,note:''};
vm.runInContext(`${declaration}
const number=n=>String(n),nodeKinds=['Объект','Модуль','Процедура','Форма','Поле'];
let root={},direction='both';
function centerMap(){} function paintMap(){}
${label}
${render}
map=scene;mapRoot=0;mapAdj=map.nodes.map(()=>[]);map.edges.forEach((e,i)=>{mapAdj[e[0]].push(i);mapAdj[e[1]].push(i)});renderGraph();`,context);
const get=expression=>vm.runInContext(expression,context);
assert.equal(get('mapVisible.length'),4,'canonical nodes must be retained');
assert.equal(get('mapDisplay.length'),3,'same-owner fields should collapse together');
assert.equal(get('mapEdges.length'),3,'canonical relations must be retained');
assert.equal(get('mapRenderEdges.length'),2,'visual links aggregate only the collapsed group');
assert.notEqual(get('graphLabel(1)'),get('graphLabel(3)'),'equal short names must remain distinguishable');
assert.equal(get('mapPositions[0].x'),0);
assert.equal(get('mapPositions[1].x'),420);
assert.notEqual(get('mapDisplayOf[1]'),1);
vm.runInContext('expandedGroups.add(mapSynthetic.get(mapDisplayOf[1]).key);renderGraph()',context);
assert.equal(get('mapDisplay.length'),4);
assert.equal(get('mapDisplayOf[1]'),1);
assert.notEqual(get('mapPositions[1].y'),get('mapPositions[2].y'));
vm.runInContext('expandedGroups.clear();renderGraph()',context);
assert.equal(get('mapDisplay.length'),3,'collapse must restore the group');
assert.equal(get('mapEdges.length'),3);
console.log('OK: grouping preserves nodes/edges; owners disambiguated; expansion restores members; root stays centered');
