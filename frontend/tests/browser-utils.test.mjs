import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { reactive } from 'vue';
import { requestId, cloneConfig } from '../src/browser-utils.ts';
test('LAN HTTP can create distinct SMS request IDs without randomUUID', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
  Object.defineProperty(globalThis, 'crypto', { configurable: true, value: { getRandomValues: webcrypto.getRandomValues.bind(webcrypto) } });
  try {
    const ids = Array.from({length:100}, () => requestId());
    assert.equal(new Set(ids).size, 100);
    for (const id of ids) assert.match(id, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/);
  } finally { Object.defineProperty(globalThis, 'crypto', original); }
});
test('discovered Vue device can be copied into an independent editor', () => {
  const device = reactive({model:'mt5700m-cn', network:{auto_connect:false}, interface:null});
  const copy = cloneConfig(device);
  copy.network.auto_connect = true;
  assert.equal(device.network.auto_connect, false);
  assert.equal(copy.interface, null);
});

test('token survives reload and is removed on logout or invalid authentication', async () => {
  const { savedToken, rememberToken } = await import('../src/browser-utils.ts');
  const values=new Map();
  const original=Object.getOwnPropertyDescriptor(globalThis,'sessionStorage');
  Object.defineProperty(globalThis,'sessionStorage',{configurable:true,value:{getItem:key=>values.get(key)||null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)}});
  try {
    assert.equal(savedToken(),'');
    rememberToken('test-token');
    const fresh=await import('../src/browser-utils.ts?reload');
    assert.equal(fresh.savedToken(),'test-token');
    fresh.rememberToken('');
    assert.equal(savedToken(),'');
    Object.defineProperty(globalThis,'sessionStorage',{configurable:true,get(){throw Error('blocked');}});
    assert.equal(savedToken(),'');
    assert.doesNotThrow(()=>rememberToken('test-token'));
  } finally {
    if(original)Object.defineProperty(globalThis,'sessionStorage',original);
    else delete globalThis.sessionStorage;
  }
});
