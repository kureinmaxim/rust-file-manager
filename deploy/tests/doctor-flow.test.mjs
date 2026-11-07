import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const source = await readFile(new URL('../../miniapp/assets/doctor-flow.js', import.meta.url), 'utf8');
const { caseGuide } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const item = (documents = {}, extra = {}) => ({ kind: 'case', status: 'ready', documents, ...extra });
for (const [record, action, step] of [
  [item(), 'complaint', 0],
  [item({ complaint: 'Жалоба' }), 'explanation', 1],
  [item({ complaint: 'Жалоба', explanation: 'Пояснения' }), 'generate', 2],
  [item({ complaint: 'Жалоба', explanation: 'Пояснения', draft: 'Черновик' }), 'review', 2],
  [item({ complaint: 'Жалоба', explanation: 'Пояснения', draft: 'Черновик', final: 'Ответ' }), 'save', 2],
  [item({ final: 'Ответ' }, { status: 'saved' }), 'deliver', null],
  [item({}, { status: 'generating' }), 'waiting', 2],
  [item({}, { status: 'archived' }), 'restore', null],
  [item({}, { read_only: true, status: 'archived' }), null, null],
  [item({ final: 'Ответ' }, { read_only: true }), 'deliver', null],
  [item({ template: 'Шаблон' }, { kind: 'template', read_only: true }), 'template', null],
]) {
  const guide = caseGuide(record);
  assert.equal(guide.action, action);
  assert.equal(guide.step, step);
  assert.ok(guide.title && guide.hint);
}
assert.equal(caseGuide(item({ draft: 'Stale' })).action, 'complaint', 'Missing prerequisites must be filled before reviewing a stale draft');
console.log('Doctor workflow: one next step for every state; archived/generating/read-only cases do not offer edit/generate.');
