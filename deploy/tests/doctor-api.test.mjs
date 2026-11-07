import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { webcrypto } from 'node:crypto';
const calls = [];
let posted = false;
let polled = false;
const stub = async (path, options) => {
  calls.push({ path, options });
  if (options?.method === 'POST') {
    if (!posted) { posted = true; throw Object.assign(new Error('lost POST response'), { status: 0 }); }
    return { job: 'synthetic-job' };
  }
  if (!polled) { polled = true; return { state: 'running' }; }
  return { state: 'done', result: { ok: true, data: { id: 'w1234567' } } };
};
globalThis.crypto ||= webcrypto;
globalThis.__doctorApi = stub;
const source = (await readFile(new URL('../../miniapp/assets/doctor-api.js', import.meta.url), 'utf8'))
 .replace("import { api, ApiError } from './api.js';", "const api = globalThis.__doctorApi; class ApiError extends Error {}")
 .replace('const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));', 'const pause = async () => {};');
const { doctorRequest, readDoctorFile } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
assert.deepEqual(await doctorRequest('generate', { case_id: 'w1234567', revision: 1 }), { id: 'w1234567' });
const posts = calls.filter((c) => c.options?.method === 'POST');
assert.equal(posts.length, 2);
assert.equal(posts[0].options.json.request_id, posts[1].options.json.request_id);
assert.equal(calls.filter((c) => c.path.endsWith('synthetic-job')).length, 2);
assert.throws(() => readDoctorFile({ name: 'image.jpg', size: 20 }), /docx/);
assert.throws(() => readDoctorFile({ name: 'large.docx', size: 6 * 1024 * 1024 }), /5 МБ/);
console.log('Doctor client: lost POST is idempotent; polling does not repeat generation; file validation passed.');
