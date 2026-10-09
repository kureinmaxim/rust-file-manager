// API client. The access token lives only in memory: when the Mini App is
// closed it is gone, and the next launch gets a fresh one from initData.

import { initData } from './tg.js';

let token = null;
let current = { user: null, limits: null, version: '' };

export class ApiError extends Error {
  constructor(message, status = 0, code = '', body = null) {
    super(message);
    this.status = status;
    this.code = code;
    this.body = body;
  }
}

export const session = () => current;
export const authHeader = () => (token ? { Authorization: `Bearer ${token}` } : {});

function remember(body) {
  token = body.access_token;
  current = { user: body.user, limits: body.limits, version: body.version };
}

async function postPublic(path, json) {
  let res;
  try {
    res = await fetch(`/api/v1${path}`, {
      method: 'POST',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify({ init_data: initData(), ...json }),
    });
  } catch {
    throw new ApiError('Нет связи с сервером. Проверьте интернет и повторите.', 0, 'network');
  }
  const body = await res.json().catch(() => ({}));
  return { res, body };
}

/** Exchange initData for a session. `{ ok: true }` or `{ ok: false, notLinked }`. */
export async function openSession() {
  const { res, body } = await postPublic('/tg/session', {});
  if (res.ok) {
    remember(body);
    return { ok: true };
  }
  if (res.status === 403 && body.code === 'not_linked') return { ok: false, notLinked: body };
  throw new ApiError(body.message || 'Не удалось войти', res.status, body.code, body);
}

export async function linkAccount(username, password) {
  const { res, body } = await postPublic('/tg/link', { username, password });
  if (!res.ok) throw new ApiError(body.message || 'Не удалось привязать аккаунт', res.status, body.code, body);
  remember(body);
}

export async function register(username, inviteToken) {
  const { res, body } = await postPublic('/tg/register', { username, invite_token: inviteToken || undefined });
  if (!res.ok) throw new ApiError(body.message || 'Не удалось создать аккаунт', res.status, body.code, body);
  remember(body);
}

/** Authenticated JSON request; a 401 renews the session from initData once. */
export async function api(path, { method = 'GET', json, retry = true } = {}) {
  const headers = { Accept: 'application/json', ...authHeader() };
  let body;
  if (json !== undefined) {
    headers['Content-Type'] = 'application/json';
    body = JSON.stringify(json);
  }
  let res;
  try {
    // Always ask the API: an old nginx 301 may still be in the WebView cache.
    res = await fetch(`/api/v1${path}`, { method, headers, body, cache: 'no-store' });
  } catch {
    throw new ApiError('Нет связи с сервером. Проверьте интернет и повторите.', 0, 'network');
  }
  if (res.status === 401 && retry) {
    const renewed = await openSession().catch(() => null);
    if (renewed && renewed.ok) return api(path, { method, json, retry: false });
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(data.message || `Ошибка ${res.status}`, res.status, data.code, data);
  return data;
}

export const qs = (params) =>
  Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join('&');

// Signed links are reused while they have more than a minute left.
const links = new Map();
export async function signedUrl(id, purpose, kind = 'files') {
  const key = `${kind}:${purpose}:${id}`;
  const cached = links.get(key);
  if (cached && cached.expires - Date.now() / 1000 > 60) return cached.url;
  const path = kind === 'exchange' ? `/exchange/items/${id}/link` : `/files/${id}/link`;
  const data = await api(path, { method: 'POST', json: { purpose } });
  links.set(key, { url: data.url, expires: data.expires_at });
  return data.url;
}
export const forgetLinks = () => links.clear();
