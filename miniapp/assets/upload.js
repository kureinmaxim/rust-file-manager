// Chunked upload queue (TELEGRAM_MINIAPP_PLAN.md §6.1). Two files at a time,
// chunks in order, retries with backoff; picking the same file again resumes
// an unfinished upload from the offset the server confirmed. Used by the Mini
// App and by the computer's page `/pc` («Загрузить с ПК»).
//
// Before a file is queued it is checked against the server's free space and
// file limit (`/uploads/capacity`); while files upload, the speed is measured
// from progress events to tell how long the rest will take.

import { api, authHeader, ApiError, renewSession, session } from './api.js';
import { fmtBytes } from './format.js';
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

const left = (i) => Math.max(0, i.size - i.sent - i.inFlight);

// --- free space ------------------------------------------------------------
// `{ available_bytes, max_file_size, chunk_size }`; available_bytes is null
// where the server cannot tell. Refreshed when a screen opens and after each
// finished upload.
let cap = null;
export const capacity = () => cap;
export async function refreshCapacity() {
  try {
    cap = await api('/uploads/capacity');
  } catch {
    /* keep the last known value; the server checks every new upload anyway */
  }
  emit();
  return cap;
}

// --- speed -----------------------------------------------------------------
// The last measured speed (all running files together) is kept for an
// estimate before the next upload starts; in localStorage when it works.
const SPEED_KEY = 'upload_speed';
const SPEED_TTL_MS = 12 * 60 * 60 * 1000;
let lastSpeed = (() => {
  try {
    const saved = JSON.parse(localStorage.getItem(SPEED_KEY) || 'null');
    return saved && Date.now() - saved.at < SPEED_TTL_MS ? saved.speed : 0;
  } catch {
    return 0;
  }
})();
let speedSavedAt = 0;

function measure(item) {
  const now = performance.now();
  const total = item.sent + item.inFlight;
  if (!item.mark) {
    item.mark = { t: now, b: total };
    return;
  }
  const dt = (now - item.mark.t) / 1000;
  if (dt < 1) return;
  const current = Math.max(0, total - item.mark.b) / dt;
  item.speed = item.speed ? item.speed * 0.7 + current * 0.3 : current;
  item.mark = { t: now, b: total };
  lastSpeed = queue.reduce((n, i) => n + (i.state === 'up' ? i.speed : 0), 0);
  if (Date.now() - speedSavedAt > 5000) {
    speedSavedAt = Date.now();
    try {
      localStorage.setItem(SPEED_KEY, JSON.stringify({ speed: lastSpeed, at: Date.now() }));
    } catch {
      /* not remembered: the next estimate waits for a fresh measurement */
    }
  }
}

/** What is left in the queue and how long it takes: at the measured speed,
 * or at the last known one before anything is measured (`measured: false`). */
export function estimate() {
  const pending = active();
  const remaining = pending.reduce((n, i) => n + left(i), 0);
  const live = pending.reduce((n, i) => n + (i.state === 'up' ? i.speed || 0 : 0), 0);
  const speed = live || lastSpeed || 0;
  return { count: pending.length, remaining, speed, measured: live > 0, seconds: speed ? remaining / speed : null };
}

/** dest: { scope, category (null = by extension), path } or { scope: 'exchange', with: owner }.
 * Files over the file limit or beyond the free space are shown as errors at
 * once instead of failing after a long upload. */
export function addFiles(files, dest) {
  const max = (cap && cap.max_file_size) || (session().limits && session().limits.max_file_size) || Infinity;
  let room = cap && typeof cap.available_bytes === 'number'
    ? cap.available_bytes - active().reduce((n, i) => n + left(i), 0)
    : Infinity;
  for (const file of files) {
    const item = {
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
    };
    if (file.size > max) {
      item.state = 'err';
      item.error = `Больше лимита: файл до ${fmtBytes(max)}`;
      item.fatal = true;
    } else if (file.size > room) {
      item.state = 'err';
      item.error = `Не хватает места на сервере: свободно ${fmtBytes(Math.max(0, room))}`;
    } else {
      room -= file.size;
    }
    queue.unshift(item);
  }
  emit();
  pump();
}

export function retry(item) {
  if (item.state !== 'err' || item.fatal) return;
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
  // The aborted chunk may hold the server's lock for a moment ("busy"): retry,
  // or the part would stay on the server for a day and count as taken space.
  for (let attempt = 1; item.id && attempt <= 5; attempt++) {
    try {
      await api(`/uploads/${item.id}`, { method: 'DELETE' });
      break;
    } catch (e) {
      if (e.code !== 'busy') break;
      await sleep(300 * attempt);
    }
  }
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
  const exchange = scope === 'exchange';
  // Resume: the server may already hold part of this very file.
  const { uploads } = await api('/uploads');
  const previous = uploads.find(
    (u) =>
      u.name === item.name &&
      u.size === item.size &&
      u.scope === scope &&
      (exchange ? u.with === item.dest.with : u.path === (path || '') && (!category || u.category === category)),
  );
  if (previous) {
    item.id = previous.id;
    item.sent = previous.offset;
    item.note = previous.offset ? 'Продолжаем с места остановки' : '';
    return;
  }
  const created = await api('/uploads', {
    method: 'POST',
    json: exchange
      ? { scope, with: item.dest.with, name: item.name, size: item.size }
      : { scope, category: category || undefined, path: path || '', name: item.name, size: item.size },
  });
  item.id = created.id;
  item.sent = 0;
}

async function run(item) {
  try {
    if (!item.id) await begin(item);
    const chunk = (session().limits && session().limits.chunk_size) || 8 * 1024 * 1024;
    let attempt = 0;
    item.mark = null;
    while (item.sent < item.size) {
      if (item.state === 'cancel') return;
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
          if (!(await renewSession())) throw e; // a computer cannot renew: reconnect
        } else if ((e.status && e.status < 500 && e.code !== 'busy') || e.code === 'no_space') {
          throw e; // the server refused this upload: retrying will not help
        }
        if (++attempt > MAX_RETRIES) throw e;
        item.note = 'Связь прервалась — повторяем…';
        emit();
        await sleep(1000 * 2 ** (attempt - 1));
        continue;
      }
      measure(item);
      emit();
    }
    const done = await api(`/uploads/${item.id}/complete`, { method: 'POST' });
    item.state = 'ok';
    item.result = done.item;
    item.file = null; // release memory held by the File reference
    haptic.ok();
    doneListeners.forEach((fn) => fn(done.item));
    refreshCapacity();
  } catch (e) {
    if (item.state === 'cancel') return;
    item.state = 'err';
    item.error = e.message || 'Не удалось загрузить файл';
    if (e.code === 'upload_not_found') item.id = null; // start over next time
    if (e.code === 'no_space') {
      if (e.body && typeof e.body.available_bytes === 'number') {
        item.error = `Не хватает места на сервере: свободно ${fmtBytes(e.body.available_bytes)}`;
      }
      refreshCapacity();
    }
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
      measure(item);
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
