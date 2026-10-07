// Chunked upload queue (TELEGRAM_MINIAPP_PLAN.md §6.1). Two files at a time,
// chunks in order, retries with backoff; picking the same file again resumes
// an unfinished upload from the offset the server confirmed.

import { api, authHeader, ApiError, openSession, session } from './api.js';
import { closingConfirmation, haptic } from './tg.js';

const PARALLEL = 2;
const MAX_RETRIES = 6;
const listeners = new Set();
const doneListeners = new Set();
export const queue = [];
let running = 0;
let seq = 0;

export function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Called with the server item after each finished upload. */
export function onUploaded(fn) {
  doneListeners.add(fn);
  return () => doneListeners.delete(fn);
}

let emitScheduled = false;
let guarding = false;
function emit() {
  if (emitScheduled) return;
  emitScheduled = true;
  requestAnimationFrame(() => {
    emitScheduled = false;
    // Evaluated on the final state of this frame, so it switches off reliably.
    const busy = queue.some((i) => i.state === 'up' || i.state === 'wait');
    if (busy !== guarding) {
      guarding = busy;
      closingConfirmation(busy);
    }
    listeners.forEach((fn) => fn(queue));
  });
}

export const active = () => queue.filter((i) => i.state === 'up' || i.state === 'wait');

/** dest: { scope, category (null = by extension), path } */
export function addFiles(files, dest) {
  for (const file of files) {
    queue.unshift({
      key: ++seq,
      file,
      name: file.name,
      size: file.size,
      dest,
      state: 'wait',
      sent: 0,
      inFlight: 0,
      error: '',
      note: '',
      speed: 0,
    });
  }
  emit();
  pump();
}

export function retry(item) {
  if (item.state !== 'err') return;
  item.state = 'wait';
  item.error = '';
  emit();
  pump();
}

export async function cancel(item) {
  const wasRunning = item.state === 'up';
  item.state = 'cancel';
  if (item.xhr) item.xhr.abort();
  emit();
  if (item.id) await api(`/uploads/${item.id}`, { method: 'DELETE' }).catch(() => {});
  const i = queue.indexOf(item);
  if (i >= 0 && !wasRunning) queue.splice(i, 1);
  emit();
}

export function clearFinished() {
  for (let i = queue.length - 1; i >= 0; i--) {
    if (queue[i].state === 'ok' || queue[i].state === 'cancel') queue.splice(i, 1);
  }
  emit();
}

function pump() {
  while (running < PARALLEL) {
    const item = [...queue].reverse().find((i) => i.state === 'wait');
    if (!item) return;
    running++;
    item.state = 'up';
    emit();
    run(item).finally(() => {
      running--;
      const i = queue.indexOf(item);
      if (item.state === 'cancel' && i >= 0) queue.splice(i, 1);
      emit();
      pump();
    });
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function begin(item) {
  const { scope, category, path } = item.dest;
  // Resume: the server may already hold part of this very file.
  const { uploads } = await api('/uploads');
  const previous = uploads.find(
    (u) =>
      u.name === item.name &&
      u.size === item.size &&
      u.scope === scope &&
      u.path === (path || '') &&
      (!category || u.category === category),
  );
  if (previous) {
    item.id = previous.id;
    item.sent = previous.offset;
    item.note = previous.offset ? 'Продолжаем с места остановки' : '';
    return;
  }
  const created = await api('/uploads', {
    method: 'POST',
    json: { scope, category: category || undefined, path: path || '', name: item.name, size: item.size },
  });
  item.id = created.id;
  item.sent = 0;
}

async function run(item) {
  try {
    if (!item.id) await begin(item);
    const chunk = (session().limits && session().limits.chunk_size) || 8 * 1024 * 1024;
    let attempt = 0;
    let windowStart = Date.now();
    let windowBytes = 0;
    while (item.sent < item.size) {
      if (item.state === 'cancel') return;
      const before = item.sent;
      try {
        item.sent = await putChunk(item, item.file.slice(item.sent, item.sent + chunk));
        item.note = '';
        attempt = 0;
      } catch (e) {
        if (item.state === 'cancel') return;
        if (e.code === 'offset_mismatch' && e.body && typeof e.body.offset === 'number') {
          item.sent = e.body.offset;
          continue;
        }
        if (e.status === 401) {
          await openSession().catch(() => null);
        } else if (e.status && e.status < 500 && e.code !== 'busy') {
          throw e; // the server refused this upload: retrying will not help
        }
        if (++attempt > MAX_RETRIES) throw e;
        item.note = 'Связь прервалась — повторяем…';
        emit();
        await sleep(1000 * 2 ** (attempt - 1));
        continue;
      }
      windowBytes += item.sent - before;
      const elapsed = (Date.now() - windowStart) / 1000;
      if (elapsed >= 1) {
        item.speed = windowBytes / elapsed;
        windowStart = Date.now();
        windowBytes = 0;
      }
      emit();
    }
    const done = await api(`/uploads/${item.id}/complete`, { method: 'POST' });
    item.state = 'ok';
    item.result = done.item;
    item.file = null; // release memory held by the File reference
    haptic.ok();
    doneListeners.forEach((fn) => fn(done.item));
  } catch (e) {
    if (item.state === 'cancel') return;
    item.state = 'err';
    item.error = e.message || 'Не удалось загрузить файл';
    if (e.code === 'upload_not_found') item.id = null; // start over next time
    haptic.error();
  } finally {
    item.inFlight = 0;
    item.xhr = null;
    emit();
  }
}

/** PUT one chunk with progress events (fetch has no upload progress). */
function putChunk(item, blob) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    item.xhr = xhr;
    xhr.open('PUT', `/api/v1/uploads/${item.id}`);
    const headers = { ...authHeader(), 'Upload-Offset': String(item.sent), 'Content-Type': 'application/octet-stream' };
    Object.entries(headers).forEach(([k, v]) => xhr.setRequestHeader(k, v));
    xhr.upload.onprogress = (e) => {
      item.inFlight = e.loaded;
      emit();
    };
    xhr.onload = () => {
      item.inFlight = 0;
      let body = {};
      try {
        body = JSON.parse(xhr.responseText || '{}');
      } catch {
        body = {};
      }
      if (xhr.status === 200) resolve(body.offset);
      else reject(new ApiError(body.message || `Ошибка ${xhr.status}`, xhr.status, body.code, body));
    };
    xhr.onerror = () => reject(new ApiError('Нет связи с сервером', 0, 'network'));
    xhr.onabort = () => reject(new ApiError('Отменено', 0, 'aborted'));
    xhr.send(blob);
  });
}
