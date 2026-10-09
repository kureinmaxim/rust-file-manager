// Exercise the API client with a cached redirect and a renewed session.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const source = (await readFile(new URL('../../miniapp/assets/api.js', import.meta.url), 'utf8'))
  .replace("import { initData } from './tg.js';", "const initData = () => 'synthetic-init-data';");
const { api, openSession } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const calls = [];
let expired = false;
const reply = (status, body) => ({ ok: status < 400, status, json: async () => body });

globalThis.fetch = async (url, options) => {
  calls.push({ url, options });
  // Simulate the stale /uploads -> /uploads/ redirect in a mobile HTTP cache.
  if (options.cache !== 'no-store') return reply(404, {});
  if (url === '/api/v1/tg/session') return reply(200, { access_token: 'synthetic-api-test-token', user: {}, limits: {}, version: 'test' });
  if (expired) { expired = false; return reply(401, { code: 'unauthorized' }); }
  return reply(200, options.method === 'POST' ? { id: 'synthetic-upload-id' } : { uploads: [] });
};

await openSession();
assert.deepEqual(await api('/uploads'), { uploads: [] });
assert.deepEqual(await api('/uploads', { method: 'POST', json: { name: 'image.jpg', size: 2300000 } }), { id: 'synthetic-upload-id' });
assert.equal(calls.at(-1).options.method, 'POST');
assert.deepEqual(JSON.parse(calls.at(-1).options.body), { name: 'image.jpg', size: 2300000 });
expired = true;
assert.deepEqual(await api('/uploads'), { uploads: [] });
assert.ok(calls.every(({ options }) => options.cache === 'no-store'));
assert.equal(calls.at(-1).options.headers.Authorization, 'Bearer synthetic-api-test-token');
console.log('API: cached redirect bypassed; POST preserved; 401 renewed without caching');
