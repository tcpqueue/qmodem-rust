import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
const file = 'packaging/openwrt/luci-app-qmodem-rust/htdocs/luci-static/resources/view/qmodem-rust/service.js';
const source = fs.readFileSync(new URL('../'+file, import.meta.url), 'utf8');
// LuCI's E()/dom.attr() omits only null/undefined attributes. Boolean false
// becomes a present HTML attribute, which disables/selects just like true.
function render(writable, configured) {
  const nodes=[];
  function E(tag, attrs={}, children=[]) {
    const node={tag,attrs:Object.fromEntries(Object.entries(attrs).filter(([,v])=>v!=null)),children};
    nodes.push(node); return node;
  }
  const rpc={declare:()=>()=>Promise.resolve({})};
  const ui={createHandlerFn:(_,fn)=>fn,addNotification(){}};
  const view = new Function('view','rpc','fs','ui','poll','L','E','_','window',source)(
    {extend:x=>x},rpc,{},ui,{add(){}},{hasViewPermission:()=>writable},E,x=>x,{location:{hostname:'192.168.123.1'}});
  view.render([{listen:'192.168.123.1',port:8088,interface:'br-lan',log_level:'info',log_format:'text',auth_configured:configured},
    [{'qmodem-rust':{enabled:true}},{'qmodem-rust':{instances:{one:{running:true}}}}],
    {interfaces:[{name:'br-lan',addresses:['192.168.123.1']},{name:'eth2',addresses:[]}]}]);
  return nodes;
}
test('administrator controls remain enabled and selects have exactly one selected option',()=>{
  const nodes=render(true,true);
  for(const label of ['Start','Stop','Restart','Enable autostart','Disable autostart','Regenerate access token','Save']) {
    const node=nodes.find(n=>n.tag==='button'&&n.children===label);
    assert.ok(node,label); assert.ok(!('disabled' in node.attrs),label);
  }
  for(const node of nodes.filter(n=>n.tag==='input'||n.tag==='select')) assert.ok(!('disabled' in node.attrs));
  const choices=nodes.filter(n=>n.tag==='select').map(n=>n.children.filter(o=>'selected' in o.attrs).map(o=>o.attrs.value));
  assert.deepEqual(choices,[['br-lan'],['info'],['text']]);
});
test('initialization is available only before a token exists; readonly controls stay disabled',()=>{
  const fresh=render(true,false).find(n=>n.children==='Initialize access token');
  assert.ok(!('disabled' in fresh.attrs));
  const existing=render(true,true).find(n=>n.children==='Initialize access token');
  assert.ok('disabled' in existing.attrs);
  for(const node of render(false,true).filter(n=>['input','select'].includes(n.tag)||n.tag==='button'&&['Start','Save','Regenerate access token'].includes(n.children))) assert.ok('disabled' in node.attrs);
});
