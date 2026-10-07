// Mini App «Файлы»: screens and overlays. Preact + htm without a build step;
// user-provided text (file names) is only ever rendered as text nodes.

import { html, render, useEffect, useRef, useState } from './vendor/preact-htm.module.js';
import * as tg from './tg.js';
import { api, ApiError, forgetLinks, linkAccount, openSession, qs, register, session, signedUrl } from './api.js';
import * as up from './upload.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const CATS = {
  Фото: { k: 'var(--c-photo)', icon: 'image' },
  Документы: { k: 'var(--c-doc)', icon: 'file-text' },
  Видео: { k: 'var(--c-video)', icon: 'film' },
  Программы: { k: 'var(--c-prog)', icon: 'package' },
  Другие: { k: 'var(--c-other)', icon: 'file' },
  Серверы: { k: 'var(--c-backup)', icon: 'archive', label: 'Бэкапы › Серверы' },
  HA: { k: 'var(--c-backup)', icon: 'archive', label: 'Бэкапы › HA' },
  Project: { k: 'var(--c-backup)', icon: 'archive', label: 'Бэкапы › Project' },
};
const catMeta = (c) => CATS[c] || { k: 'var(--c-other)', icon: 'file' };
const catLabel = (c) => (CATS[c] && CATS[c].label) || c;
const KIND_ICON = { image: 'image', video: 'film', audio: 'music', pdf: 'file-text', text: 'file-text', archive: 'archive', other: 'file' };
const KIND_COLOR = { image: 'var(--c-photo)', video: 'var(--c-video)', audio: 'var(--c-video)', pdf: 'var(--c-doc)', text: 'var(--c-doc)', archive: 'var(--c-backup)', other: 'var(--c-other)' };
const ZONE_TITLE = { my: 'Мои файлы', shared: 'Общие файлы' };
const MONTHS = ['янв', 'фев', 'мар', 'апр', 'мая', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек'];

function fmtBytes(n) {
  if (!n) return '0 Б';
  const units = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  const value = i === 0 ? String(Math.round(n)) : n.toFixed(n < 10 ? 1 : 0).replace('.', ',');
  return `${value} ${units[i]}`;
}

function fmtDate(sec) {
  if (!sec) return '';
  const d = new Date(sec * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = (day(now) - day(d)) / 86400000;
  if (diff === 0) return `сегодня, ${hm}`;
  if (diff === 1) return `вчера, ${hm}`;
  const date = `${d.getDate()} ${MONTHS[d.getMonth()]}`;
  return d.getFullYear() === now.getFullYear() ? date : `${date} ${d.getFullYear()}`;
}

const plural = (n, one, few, many) => {
  const a = n % 10;
  const b = n % 100;
  return a === 1 && b !== 11 ? one : a >= 2 && a <= 4 && (b < 12 || b > 14) ? few : many;
};
const joinPath = (a, b) => (a ? `${a}/${b}` : b);
const extOf = (name) => {
  const i = name.lastIndexOf('.');
  return i > 0 && name.length - i <= 5 ? name.slice(i + 1) : '';
};
const EXT_KIND = {
  image: ['jpg', 'jpeg', 'png', 'gif', 'webp', 'heic', 'avif', 'bmp'],
  video: ['mp4', 'mov', 'mkv', 'webm', 'avi', 'm4v'],
  audio: ['mp3', 'm4a', 'ogg', 'wav', 'flac'],
  pdf: ['pdf'],
  archive: ['zip', 'gz', 'tgz', 'tar', '7z', 'rar', 'xz'],
  text: ['txt', 'md', 'json', 'yml', 'yaml', 'toml', 'conf', 'log', 'csv'],
};
const guessKind = (name) => {
  const ext = (name.split('.').pop() || '').toLowerCase();
  return Object.keys(EXT_KIND).find((k) => EXT_KIND[k].includes(ext)) || 'other';
};
const greeting = () => {
  const h = new Date().getHours();
  return h < 5 ? 'Доброй ночи' : h < 12 ? 'Доброе утро' : h < 18 ? 'Добрый день' : 'Добрый вечер';
};

/** Signed link for a file of a zone or of the exchange. */
const linkFor = (item, purpose) => signedUrl(item.id, purpose, item.direction ? 'exchange' : 'files');

function Icon({ name, cls = 'icon' }) {
  return html`<svg class=${cls} aria-hidden="true"><use href=${`#i-${name}`} /></svg>`;
}

// ---------------------------------------------------------------------------
// Small global stores: overlays, navigation, data version
// ---------------------------------------------------------------------------

function store(initial) {
  const state = initial;
  const listeners = new Set();
  return {
    state,
    set(patch) {
      Object.assign(state, patch);
      listeners.forEach((fn) => fn());
    },
    use() {
      const [, force] = useState(0);
      useEffect(() => {
        const fn = () => force((x) => x + 1);
        listeners.add(fn);
        return () => listeners.delete(fn);
      }, []);
      return state;
    },
  };
}

const ui = store({ sheet: null, dialog: null, toast: null, viewer: null });
const nav = store({ stack: [{ name: 'home' }] });
const data = store({ version: 0, overview: null, exchange: null });

const overlayOpen = () => Boolean(ui.state.sheet || ui.state.viewer || ui.state.dialog);
const top = () => nav.state.stack[nav.state.stack.length - 1];

function go(screen) {
  nav.set({ stack: [...nav.state.stack, screen] });
  tg.haptic.tap();
}
function replaceTop(patch) {
  const stack = [...nav.state.stack];
  stack[stack.length - 1] = { ...stack[stack.length - 1], ...patch };
  nav.set({ stack });
}
function back() {
  if (ui.state.dialog) {
    ui.state.dialog.resolve(null);
    return setUi({ dialog: null });
  }
  if (ui.state.viewer) return setUi({ viewer: null });
  if (ui.state.sheet) return setUi({ sheet: null });
  if (nav.state.stack.length > 1) nav.set({ stack: nav.state.stack.slice(0, -1) });
}

function changed() {
  data.set({ version: data.state.version + 1, overview: null, exchange: null });
}
up.onUploaded(changed);

let toastTimer;
function toast(text, kind = 'ok') {
  clearTimeout(toastTimer);
  setUi({ toast: { text, kind } });
  toastTimer = setTimeout(() => setUi({ toast: null }), 2800);
}
function showError(e) {
  tg.haptic.error();
  toast((e && e.message) || 'Что-то пошло не так', 'error');
}

// Bottom MainButton: the current screen asks for it, overlays hide it.
let mainCfg = null;
function applyMain() {
  if (overlayOpen() || !mainCfg) tg.mainButton.hide();
  else tg.mainButton.set(mainCfg);
}
function setUi(patch) {
  ui.set(patch);
  applyMain();
}
function useMainButton(cfg, deps) {
  useEffect(() => {
    mainCfg = cfg;
    applyMain();
    return () => {
      if (mainCfg === cfg) {
        mainCfg = null;
        applyMain();
      }
    };
  }, deps);
}

/** Text prompt in an in-app dialog (window.prompt is unreliable in WebViews). */
function ask({ title, message = '', value = '', placeholder = '', ok = 'Готово' }) {
  return new Promise((resolve) => setUi({ dialog: { title, message, value, placeholder, ok, resolve } }));
}

async function loadOverview() {
  if (data.state.overview) return data.state.overview;
  const ov = await api('/overview');
  data.set({ overview: ov });
  return ov;
}
function useOverview() {
  const d = data.use();
  useEffect(() => {
    if (!d.overview) loadOverview().catch(showError);
  }, [d.version]);
  return d.overview;
}
async function loadExchange() {
  if (data.state.exchange) return data.state.exchange;
  const ex = await api('/exchange');
  data.set({ exchange: ex });
  return ex;
}
function useExchange() {
  const d = data.use();
  useEffect(() => {
    if (!d.exchange) loadExchange().catch(() => {});
  }, [d.version]);
  return d.exchange;
}
const incomingCount = (ex) => (!ex ? 0 : ex.role === 'admin' ? ex.partners.reduce((n, p) => n + p.incoming, 0) : ex.incoming.length);
const asFile = (item) => ({ ...item, type: 'file' });

function useQueue() {
  const [, force] = useState(0);
  useEffect(() => up.subscribe(() => force((x) => x + 1)), []);
  return up.queue;
}

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------

function Thumb({ item, cls = 'ftile' }) {
  const [src, setSrc] = useState(null);
  const ref = useRef(null);
  useEffect(() => {
    if (item.type !== 'file' || item.kind !== 'image' || item.local) return undefined;
    let alive = true;
    const io = new IntersectionObserver((entries) => {
      if (!entries.some((e) => e.isIntersecting)) return;
      io.disconnect();
      linkFor(item, 'inline')
        .then((url) => alive && setSrc(url))
        .catch(() => {});
    });
    if (ref.current) io.observe(ref.current);
    return () => {
      alive = false;
      io.disconnect();
    };
  }, [item.id]);
  if (item.type === 'folder') {
    return html`<span class=${`${cls} folder`} style=${`--k:${catMeta(item.category).k}`}><${Icon} name="folder" /></span>`;
  }
  const ext = extOf(item.name);
  const color = KIND_COLOR[item.kind] || 'var(--c-other)';
  return html`<span ref=${ref} class=${`${cls} ${ext && !src ? 'has-ext' : ''}`} style=${`--k:${color}`}>
    <${Icon} name=${KIND_ICON[item.kind] || 'file'} />
    ${ext && !src && html`<em>${ext}</em>`}
    ${src && html`<img src=${src} alt="" loading="lazy" decoding="async" />`}
  </span>`;
}

function ItemRow({ item, showWhere }) {
  const meta =
    item.type === 'folder'
      ? `${item.files} ${plural(item.files, 'файл', 'файла', 'файлов')} · ${fmtBytes(item.bytes)}`
      : `${fmtBytes(item.size)} · ${fmtDate(item.mtime)}`;
  const where = showWhere ? ` · ${catLabel(item.category)}${item.path ? ` › ${item.path}` : ''}` : '';
  return html`<button class="row wide tap" type="button" onClick=${() => openItem(item)}>
    <${Thumb} item=${item} />
    <span class="rt"><b>${item.name}</b><small>${meta}${where}</small></span>
    ${item.type === 'folder' && html`<${Icon} name="right" cls="icon chev" />`}
  </button>`;
}

let currentItems = [];
function openItem(item) {
  if (item.type === 'folder') {
    go({ name: 'browse', zone: item.scope, category: item.category, path: joinPath(item.path, item.name) });
  } else {
    tg.haptic.tap();
    setUi({ sheet: { item } });
  }
}

/** Show a file: images in the viewer, media and PDF inline, the rest downloaded. */
async function viewFile(item) {
  if (item.kind === 'image') {
    const images = (currentItems.length ? currentItems : [item]).filter((i) => i.type === 'file' && i.kind === 'image');
    const index = Math.max(0, images.findIndex((i) => i.id === item.id));
    return setUi({ sheet: null, viewer: { items: images.length ? images : [item], index } });
  }
  if (['video', 'audio', 'pdf'].includes(item.kind)) {
    try {
      tg.openLink(await linkFor(item, 'inline'));
    } catch (e) {
      showError(e);
    }
    return undefined;
  }
  return downloadFile(item);
}
async function downloadFile(item) {
  try {
    tg.download(await linkFor(item, 'download'), item.name);
    toast('Telegram предложит сохранить файл');
  } catch (e) {
    showError(e);
  }
}

function Skeleton({ rows = 4 }) {
  return html`<div aria-label="Загрузка">${Array.from({ length: rows }, (_, i) => html`<div class="skeleton" key=${i}></div>`)}</div>`;
}

function ErrorBox({ error, onRetry }) {
  return html`<div class="error-box" role="alert"><${Icon} name="alert" /><span>${error.message}</span>
    ${onRetry && html`<button type="button" onClick=${onRetry}>Повторить</button>`}</div>`;
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

function Home() {
  const ov = useOverview();
  const ex = useExchange();
  const queue = useQueue();
  const tgu = tg.tgUser();
  const user = session().user || {};
  useMainButton({ text: 'Загрузить файлы', onClick: () => go({ name: 'uploads', dest: { scope: 'my', category: null, path: '' } }) }, []);

  const name = (tgu && tgu.first_name) || user.username || '';
  const activeItems = queue.filter((i) => i.state === 'up' || i.state === 'wait');
  const failed = queue.filter((i) => i.state === 'err');

  if (!ov) return html`<div class="title"><h1>Файлы</h1></div><${Skeleton} rows=${6} />`;
  const my = ov.zones.find((z) => z.scope === 'my');
  const shared = ov.zones.find((z) => z.scope === 'shared');
  const segs = my.categories.filter((c) => c.bytes > 0).sort((a, b) => b.bytes - a.bytes);
  const C = 2 * Math.PI * 40;
  let offset = 0;
  const arcs = segs.map((c) => {
    const len = my.bytes ? (c.bytes / my.bytes) * C : 0;
    const arc = html`<circle cx="50" cy="50" r="40" fill="none" stroke=${catMeta(c.category).k} stroke-width="10"
      stroke-dasharray=${`${Math.max(len - 1.5, 0.5)} ${C}`} stroke-dashoffset=${-offset} />`;
    offset += len;
    return arc;
  });

  let pill = null;
  if (activeItems.length) {
    const cur = activeItems.find((i) => i.state === 'up') || activeItems[0];
    const pct = cur.size ? Math.round(((cur.sent + cur.inFlight) / cur.size) * 100) : 0;
    pill = html`<button class="pill tap" type="button" onClick=${() => go({ name: 'uploads', dest: cur.dest })}>
      <span class="spin" aria-hidden="true"></span>
      <span class="rt">Загружается ${cur.name}<small>${pct} %${activeItems.length > 1 ? ` · ещё ${activeItems.length - 1} в очереди` : ''}</small>
      <span class="bar"><i style=${`width:${pct}%`}></i></span></span>
      <${Icon} name="right" cls="icon chev" /></button>`;
  } else if (failed.length) {
    pill = html`<button class="pill tap" type="button" onClick=${() => go({ name: 'uploads', dest: failed[0].dest })}>
      <span class="tile" style="--k:var(--danger)"><${Icon} name="alert" /></span>
      <span class="rt">Загрузка прервалась: ${failed[0].name}<small>Нажмите, чтобы продолжить — без начала с нуля</small></span>
      <${Icon} name="right" cls="icon chev" /></button>`;
  }

  const incoming = incomingCount(ex);
  const exchangePill = incoming > 0 && html`<button class="pill tap" type="button" onClick=${() => go({ name: 'exchange' })}>
      <span class="tile" style="--k:var(--c-photo)"><${Icon} name="inbox" /></span>
      <span class="rt">${user.is_admin ? 'Участники прислали файлы' : 'Администратор прислал файлы'}<small>${incoming} ${plural(incoming, 'файл', 'файла', 'файлов')} в обмене · сохраните к себе или скачайте</small></span>
      <${Icon} name="right" cls="icon chev" /></button>`;

  return html`
    <div class="hello">
      <span class="avatar" aria-hidden="true">${(name[0] || '?').toUpperCase()}</span>
      <div><b>${greeting()}${name ? `, ${name}` : ''}</b><small>${user.username} · вход через Telegram${user.is_admin ? ' · администратор' : ''}</small></div>
    </div>
    ${pill}
    ${exchangePill}
    <div class="group">
      <div class="storage">
        <div class="ring" role="img" aria-label=${`Мои файлы занимают ${fmtBytes(my.bytes)}`}>
          <svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="var(--fill)" stroke-width="10" />${arcs}</svg>
          <div class="rc"><div><b>${fmtBytes(my.bytes)}</b><br /><small>мои файлы</small></div></div>
        </div>
        <div class="legend">
          ${segs.length === 0 && html`<span>Пока пусто — загрузите первые файлы</span>`}
          ${segs.slice(0, 5).map((c) => html`<div key=${c.category}><span><i style=${`--k:${catMeta(c.category).k}`}></i>${catLabel(c.category)}</span><b>${fmtBytes(c.bytes)}</b></div>`)}
        </div>
      </div>
      <div class="foot">
        <span>Мои · ${my.files} ${plural(my.files, 'файл', 'файла', 'файлов')}</span>
        <span>Общие · ${fmtBytes(shared.bytes)}</span>
        ${ov.disk && html`<span>Свободно ${fmtBytes(ov.disk.free_bytes)}</span>`}
      </div>
    </div>
    <div class="quick">
      <button class="qa tap" type="button" onClick=${() => go({ name: 'uploads', dest: { scope: 'my', category: null, path: '' } })}><${Icon} name="upload" />Загрузить</button>
      <button class="qa tap" type="button" onClick=${() => go({ name: 'browse', zone: 'my', category: null, path: '' })}><${Icon} name="lock" />Мои</button>
      <button class="qa tap" type="button" onClick=${() => go({ name: 'browse', zone: 'shared', category: null, path: '' })}><${Icon} name="users" />Общие</button>
      <button class="qa tap" type="button" onClick=${() => go({ name: 'browse', zone: 'my', category: null, path: '', focusSearch: true })}><${Icon} name="search" />Поиск</button>
    </div>
    <p class="group-title"><span>Разделы</span></p>
    <div class="group">
      <button class="row tap" type="button" onClick=${() => go({ name: 'browse', zone: 'my', category: null, path: '' })}>
        <span class="tile" style="--k:var(--c-doc)"><${Icon} name="lock" /></span><span class="rt"><b>Мои файлы</b></span>
        <span class="rv">${my.files} · ${fmtBytes(my.bytes)}<${Icon} name="right" cls="icon chev" /></span></button>
      <button class="row tap" type="button" onClick=${() => go({ name: 'browse', zone: 'shared', category: null, path: '' })}>
        <span class="tile" style="--k:var(--c-prog)"><${Icon} name="users" /></span><span class="rt"><b>Общие файлы</b></span>
        <span class="rv">${shared.files} · ${fmtBytes(shared.bytes)}<${Icon} name="right" cls="icon chev" /></span></button>
      <button class="row tap" type="button" onClick=${() => go({ name: 'exchange' })}>
        <span class="tile" style="--k:var(--c-photo)"><${Icon} name="swap" /></span><span class="rt"><b>Обмен</b><small>${user.is_admin ? 'С каждым участником отдельно' : 'Только вы и администратор'}</small></span>
        <span class="rv">${incoming > 0 && html`<span class="badge" aria-label=${`Пришло файлов: ${incoming}`}>${incoming}</span>`}<${Icon} name="right" cls="icon chev" /></span></button>
      <button class="row tap" type="button" onClick=${() => go({ name: 'uploads', dest: { scope: 'my', category: null, path: '' } })}>
        <span class="tile" style="--k:var(--accent)"><${Icon} name="upload" /></span><span class="rt"><b>Загрузки</b></span>
        <span class="rv">${activeItems.length ? `${activeItems.length} в работе` : ''}<${Icon} name="right" cls="icon chev" /></span></button>
      <button class="row tap" type="button" onClick=${() => go({ name: 'settings' })}>
        <span class="tile" style="--k:var(--c-other)"><${Icon} name="settings" /></span><span class="rt"><b>Настройки</b></span>
        <span class="rv"><${Icon} name="right" cls="icon chev" /></span></button>
    </div>
    ${ov.recent.length > 0 && html`
      <p class="group-title"><span>Недавние</span></p>
      <div class="recent">
        ${ov.recent.map((item) => html`<button class="rcard tap" type="button" key=${item.id} onClick=${() => openItem(item)}>
          <${Thumb} item=${item} /><b>${item.name}</b><small>${fmtDate(item.mtime)}</small></button>`)}
      </div>`}
  `;
}

const SORTS = [
  { sort: 'name', order: 'asc', label: 'По имени' },
  { sort: 'mtime', order: 'desc', label: 'Сначала новые' },
  { sort: 'size', order: 'desc', label: 'Сначала большие' },
];
function readPref(key, fallback) {
  try {
    return localStorage.getItem(key) || fallback;
  } catch {
    return fallback;
  }
}
function writePref(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage may be unavailable: the preference just is not remembered */
  }
}

function Browse({ screen }) {
  const { zone, category, path } = screen;
  const d = data.use();
  const ov = useOverview();
  const [q, setQ] = useState('');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState(null);
  const [error, setError] = useState(null);
  const [sortIx, setSortIx] = useState(() => Number(readPref('sort', '0')) % SORTS.length);
  const [view, setView] = useState(() => readPref('view', 'list'));
  const searchRef = useRef(null);

  useEffect(() => {
    const t = setTimeout(() => setQuery(q.trim()), 300);
    return () => clearTimeout(t);
  }, [q]);
  useEffect(() => {
    if (screen.focusSearch && searchRef.current) searchRef.current.focus();
  }, []);
  const load = () => {
    let alive = true;
    setItems(null);
    setError(null);
    const s = SORTS[sortIx];
    api(`/files?${qs({ scope: zone, category, path, q: query, sort: s.sort, order: s.order })}`)
      .then((res) => alive && setItems(res.items))
      .catch((e) => alive && setError(e));
    return () => {
      alive = false;
    };
  };
  useEffect(load, [zone, category, path, query, sortIx, d.version]);
  useMainButton(
    { text: path ? 'Загрузить в эту папку' : 'Загрузить сюда', onClick: () => go({ name: 'uploads', dest: { scope: zone, category, path } }) },
    [zone, category, path],
  );
  currentItems = items || [];

  const zoneOv = ov && ov.zones.find((z) => z.scope === zone);
  const chips = zoneOv ? zoneOv.categories.filter((c) => c.files + c.folders > 0 || c.category === category) : [];
  const segments = path ? path.split('/') : [];
  const title = segments.length ? segments[segments.length - 1] : ZONE_TITLE[zone];
  const sub = category ? catLabel(category) : 'все категории';

  async function newFolder() {
    if (!category) return toast('Сначала выберите категорию', 'error');
    const name = await ask({ title: 'Новая папка', placeholder: 'Название папки', ok: 'Создать' });
    if (!name) return;
    try {
      await api('/folders', { method: 'POST', json: { scope: zone, category, path, name: name.trim() } });
      tg.haptic.ok();
      toast(`Создана папка «${name.trim()}»`);
      changed();
    } catch (e) {
      showError(e);
    }
  }

  const switchZone = (z) => {
    tg.haptic.select();
    replaceTop({ zone: z, category: null, path: '' });
  };
  const pickCategory = (c) => {
    tg.haptic.select();
    replaceTop({ category: c, path: '' });
  };
  const nextSort = () => {
    const i = (sortIx + 1) % SORTS.length;
    setSortIx(i);
    writePref('sort', String(i));
    tg.haptic.select();
  };
  const toggleView = (v) => {
    setView(v);
    writePref('view', v);
  };

  let body;
  if (error) body = html`<${ErrorBox} error=${error} onRetry=${load} />`;
  else if (!items) body = html`<${Skeleton} />`;
  else if (items.length === 0) {
    body = html`<div class="group"><div class="empty"><${Icon} name=${query ? 'search' : 'folder'} />
      ${query ? `Ничего не найдено по «${query}».` : 'Здесь пока пусто. Загрузите файлы или создайте папку.'}</div></div>`;
  } else if (view === 'grid') {
    body = html`<div class="grid">${items.map((item) => html`<button class=${`cell tap ${item.type === 'folder' ? 'folder' : ''}`} type="button" key=${item.id}
      onClick=${() => openItem(item)} aria-label=${item.name} style=${`--k:${item.type === 'folder' ? catMeta(item.category).k : KIND_COLOR[item.kind]}`}>
      ${item.type === 'folder' ? html`<${Icon} name="folder" />` : html`<${Thumb} item=${item} cls="ftile" />`}
      ${item.kind !== 'image' && html`<span class="cname">${item.name}</span>`}</button>`)}</div>`;
  } else {
    const folders = items.filter((i) => i.type === 'folder');
    const files = items.filter((i) => i.type === 'file');
    const showWhere = Boolean(query) || !category;
    body = html`
      ${folders.length > 0 && html`<p class="group-title"><span>Папки</span></p><div class="group">${folders.map((i) => html`<${ItemRow} item=${i} key=${i.id} showWhere=${showWhere} />`)}</div>`}
      ${files.length > 0 && html`<p class="group-title"><span>Файлы · ${files.length}</span></p><div class="group">${files.map((i) => html`<${ItemRow} item=${i} key=${i.id} showWhere=${showWhere} />`)}</div>`}`;
  }

  return html`
    <div class="title"><div><h1>${title}</h1><small>${segments.length ? `${ZONE_TITLE[zone]} · ${sub}` : sub}</small></div></div>
    ${!path && html`<div class="seg" role="group" aria-label="Зона">
      <button type="button" aria-pressed=${String(zone === 'my')} onClick=${() => switchZone('my')}>Мои</button>
      <button type="button" aria-pressed=${String(zone === 'shared')} onClick=${() => switchZone('shared')}>Общие</button></div>`}
    <div class="search"><${Icon} name="search" /><input ref=${searchRef} type="search" value=${q} placeholder=${path ? 'Поиск в этой папке' : 'Поиск по имени'}
      aria-label="Поиск по имени файла или папки" onInput=${(e) => setQ(e.currentTarget.value)} /></div>
    ${path
      ? html`<nav class="crumbs" aria-label="Путь"><button type="button" onClick=${() => replaceTop({ path: '' })}>${catLabel(category)}</button>
          ${segments.map((s, i) => (i < segments.length - 1
            ? html`<span>›</span><button type="button" onClick=${() => replaceTop({ path: segments.slice(0, i + 1).join('/') })}>${s}</button>`
            : html`<span>›</span><b>${s}</b>`))}</nav>`
      : html`<div class="chips">
          <button class="chip tap" type="button" aria-pressed=${String(!category)} onClick=${() => pickCategory(null)}>Все<small>${zoneOv ? zoneOv.files : ''}</small></button>
          ${chips.map((c) => html`<button class="chip tap" type="button" key=${c.category} aria-pressed=${String(category === c.category)}
            onClick=${() => pickCategory(c.category)}><i style=${`--k:${catMeta(c.category).k}`}></i>${catLabel(c.category)}<small>${c.files}</small></button>`)}
        </div>`}
    <div class="toolbar">
      <div class="tools">
        <button class="tbtn" type="button" onClick=${nextSort}><${Icon} name="sort" />${SORTS[sortIx].label}</button>
        <button class="tbtn" type="button" onClick=${newFolder} disabled=${!category}><${Icon} name="plus" />Папка</button>
      </div>
      <div class="tools">
        <button class="ib" type="button" aria-pressed=${String(view === 'list')} aria-label="Списком" onClick=${() => toggleView('list')}><${Icon} name="list" /></button>
        <button class="ib" type="button" aria-pressed=${String(view === 'grid')} aria-label="Сеткой" onClick=${() => toggleView('grid')}><${Icon} name="grid" /></button>
      </div>
    </div>
    ${body}`;
}

function Uploads({ screen }) {
  const dest = screen.dest;
  const queue = useQueue();
  const filesRef = useRef(null);
  const [over, setOver] = useState(false);
  const pick = () => filesRef.current && filesRef.current.click();
  useMainButton({ text: 'Выбрать файлы', onClick: pick }, []);

  const add = (list) => {
    const files = Array.from(list || []);
    if (!files.length) return;
    up.addFiles(files, dest);
    tg.haptic.ok();
  };
  const admin = (session().user || {}).is_admin;
  const where = dest.scope === 'exchange'
    ? `Обмен › ${admin ? `отправить ${dest.with}` : 'отправить администратору'}`
    : `${ZONE_TITLE[dest.scope]} › ${dest.category ? catLabel(dest.category) : 'категория по типу файла'}${dest.path ? ` › ${dest.path}` : ''}`;
  const hasFinished = queue.some((i) => i.state === 'ok');

  return html`
    <div class="title"><div><h1>Загрузки</h1><small>${where}</small></div></div>
    <div class=${`drop ${over ? 'over' : ''}`}
      onDragOver=${(e) => { e.preventDefault(); setOver(true); }}
      onDragLeave=${() => setOver(false)}
      onDrop=${(e) => { e.preventDefault(); setOver(false); add(e.dataTransfer.files); }}>
      <${Icon} name="upload" />
      <b>Выберите файлы или перетащите их сюда</b>
      <small>Большие файлы загружаются частями: обрыв связи не начнёт загрузку заново</small>
      <div class="picks">
        <input id="pick-files" class="file-input" type="file" multiple ref=${filesRef} onChange=${(e) => { add(e.currentTarget.files); e.currentTarget.value = ''; }} />
        <label class="btn" for="pick-files"><${Icon} name="file" />Файлы</label>
        <input id="pick-camera" class="file-input" type="file" accept="image/*,video/*" capture="environment" onChange=${(e) => { add(e.currentTarget.files); e.currentTarget.value = ''; }} />
        <label class="btn secondary" for="pick-camera"><${Icon} name="camera" />Камера</label>
      </div>
    </div>
    ${queue.length > 0 && html`
      <p class="group-title"><span>Очередь</span>${hasFinished && html`<button type="button" onClick=${() => up.clearFinished()}>Убрать готовые</button>`}</p>
      <div class="group">${queue.map((item) => html`<${QueueRow} item=${item} key=${item.key} />`)}</div>`}
    <p class="note">Можно ненадолго свернуть Telegram. Если загрузка прервётся, откройте этот экран и выберите тот же файл — она продолжится с места остановки.</p>`;
}

function QueueRow({ item }) {
  const pct = item.size ? Math.min(100, Math.round(((item.sent + item.inFlight) / item.size) * 100)) : 100;
  let state;
  let bar = null;
  if (item.state === 'up') {
    state = `${pct} %${item.speed ? ` · ${fmtBytes(item.speed)}/с` : ''}`;
    bar = html`<span class="bar"><i style=${`width:${pct}%`}></i></span>`;
  } else if (item.state === 'wait') state = 'ожидает';
  else if (item.state === 'ok') {
    state = item.result.direction
      ? `отправлено${item.result.name !== item.name ? ` как ${item.result.name}` : ''}`
      : `готово → ${catLabel(item.result.category)}${item.result.path ? ` › ${item.result.path}` : ''}`;
  }
  else if (item.state === 'err') {
    state = item.error;
    bar = html`<span class="bar err"><i style=${`width:${pct}%`}></i></span>`;
  } else state = 'отменено';
  return html`<div class="row wide">
    <${Thumb} item=${{ type: 'file', id: `queue-${item.key}`, name: item.name, kind: item.result ? item.result.kind : guessKind(item.name), local: true }} />
    <span class="rt"><b>${item.name}</b><small class=${item.state === 'err' ? 'danger-text' : ''}>${fmtBytes(item.size)} · ${state}${item.note ? ` · ${item.note}` : ''}</small>${bar}</span>
    ${item.state === 'ok' && html`<span class="qstate ok"><${Icon} name="check" /></span>`}
    ${item.state === 'err' && html`<button class="tbtn" type="button" onClick=${() => up.retry(item)}><${Icon} name="refresh" />Повторить</button>`}
    ${(item.state === 'up' || item.state === 'wait') && html`<button class="ib" type="button" aria-label=${`Отменить загрузку ${item.name}`} onClick=${() => up.cancel(item)}><${Icon} name="x" /></button>`}
  </div>`;
}

function Exchange({ screen }) {
  const d = data.use();
  const user = session().user || {};
  const owner = screen.owner || '';
  const [res, setRes] = useState(null);
  const [error, setError] = useState(null);
  const [tab, setTab] = useState('incoming');
  const load = () => {
    let alive = true;
    setRes(null);
    setError(null);
    api(owner ? `/exchange/${encodeURIComponent(owner)}` : '/exchange')
      .then((r) => {
        if (!alive) return;
        setRes(r);
        if (!owner) data.set({ exchange: r });
      })
      .catch((e) => alive && setError(e));
    return () => {
      alive = false;
    };
  };
  useEffect(load, [owner, d.version]);
  const partnerList = res && res.role === 'admin';
  const target = res && !partnerList ? res.owner : null;
  useMainButton(
    target ? { text: user.is_admin ? `Отправить файлы ${target}` : 'Отправить администратору', onClick: () => go({ name: 'uploads', dest: { scope: 'exchange', with: target } }) } : null,
    [target],
  );

  if (error) return html`<div class="title"><h1>Обмен</h1></div><${ErrorBox} error=${error} onRetry=${load} />`;
  if (!res) return html`<div class="title"><h1>Обмен</h1></div><${Skeleton} />`;

  if (partnerList) {
    return html`
      <div class="title"><div><h1>Обмен</h1><small>С каждым участником отдельно</small></div></div>
      ${res.partners.length === 0
        ? html`<div class="group"><div class="empty"><${Icon} name="users" />Участников пока нет. Пригласите человека командой /files_invite в чате с ботом.</div></div>`
        : html`<div class="group">${res.partners.map((p) => html`<button class="row wide tap" type="button" key=${p.username} onClick=${() => go({ name: 'exchange', owner: p.username })}>
            <span class="tile" style="--k:var(--c-prog)"><${Icon} name="users" /></span>
            <span class="rt"><b>${p.username}</b><small>пришло ${p.incoming} · отправлено ${p.outgoing}${p.last_mtime ? ` · ${fmtDate(p.last_mtime)}` : ''}</small></span>
            <span class="rv">${p.incoming > 0 && html`<span class="badge" aria-label=${`Пришло файлов: ${p.incoming}`}>${p.incoming}</span>`}<${Icon} name="right" cls="icon chev" /></span></button>`)}</div>`}
      <p class="note" style="margin-top:0">Отправленное участнику видит только он. Присланное можно сохранить копией в «Мои файлы».</p>`;
  }

  const list = (tab === 'incoming' ? res.incoming : res.outgoing).map(asFile);
  currentItems = list;
  const empty = tab === 'incoming'
    ? 'Пока ничего не пришло.'
    : `Вы ещё ничего не отправляли. Нажмите «${user.is_admin ? 'Отправить файлы' : 'Отправить администратору'}» внизу.`;
  return html`
    <div class="title"><div><h1>${user.is_admin ? owner : 'Обмен'}</h1><small>${user.is_admin ? 'Обмен · видите только вы и этот участник' : 'Видите только вы и администратор'}</small></div></div>
    <div class="seg" role="group" aria-label="Направление">
      <button type="button" aria-pressed=${String(tab === 'incoming')} onClick=${() => { setTab('incoming'); tg.haptic.select(); }}>Входящие <small>${res.incoming.length}</small></button>
      <button type="button" aria-pressed=${String(tab === 'outgoing')} onClick=${() => { setTab('outgoing'); tg.haptic.select(); }}>Отправленные <small>${res.outgoing.length}</small></button>
    </div>
    ${list.length === 0
      ? html`<div class="group"><div class="empty"><${Icon} name=${tab === 'incoming' ? 'inbox' : 'send'} />${empty}</div></div>`
      : html`<div class="group">${list.map((i) => html`<${ItemRow} item=${i} key=${i.id} />`)}</div>`}
    <p class="note" style="margin-top:0">«Сохранить к себе» кладёт копию в «Мои файлы» — без лишнего места на диске. Удалённое из обмена исчезает у обоих.</p>`;
}

function ExchangeSheet({ item }) {
  const close = () => setUi({ sheet: null });
  const user = session().user || {};
  const incoming = item.direction === 'incoming';
  const peer = user.is_admin ? item.owner : 'администратор';
  const from = incoming ? `От: ${user.is_admin ? item.owner : 'администратор'}` : `Кому: ${peer}`;

  async function save() {
    try {
      const r = await api(`/exchange/items/${item.id}/save`, { method: 'POST' });
      tg.haptic.ok();
      toast(`${r.message}: ${r.name}`);
      changed();
    } catch (e) {
      showError(e);
    }
  }
  async function remove() {
    const ok = await tg.confirm({
      title: 'Удалить из обмена?',
      message: `«${item.name}» исчезнет из обмена у обоих. Копии, сохранённые в «Мои файлы», останутся.`,
      ok: 'Удалить',
      destructive: true,
    });
    if (!ok) return;
    try {
      await api(`/exchange/items/${item.id}`, { method: 'DELETE' });
      close();
      tg.haptic.warn();
      toast('Удалено из обмена');
      changed();
    } catch (e) {
      showError(e);
    }
  }

  return html`<div class="layer" onClick=${(e) => e.target === e.currentTarget && close()}>
    <div class="sheet" role="dialog" aria-modal="true" aria-label=${item.name}>
      <div class="grab"></div>
      <div class="fhead"><${Thumb} item=${item} /><div><b>${item.name}</b><small>${fmtBytes(item.size)} · ${fmtDate(item.mtime)}</small></div></div>
      <div class="actions">
        <button class="act tap" type="button" onClick=${() => viewFile(item)}><${Icon} name="eye" />Открыть</button>
        <button class="act tap" type="button" onClick=${() => downloadFile(item)}><${Icon} name="download" />Скачать</button>
        ${incoming && html`<button class="act tap" type="button" onClick=${save}><${Icon} name="copy" />К себе</button>`}
        <button class="act tap danger" type="button" onClick=${remove}><${Icon} name="trash" />Удалить</button>
      </div>
      <div class="group"><dl class="kv">
        <dt>Обмен</dt><dd>${from}</dd>
        <dt>Размер</dt><dd>${fmtBytes(item.size)}</dd>
        <dt>${incoming ? 'Получен' : 'Отправлен'}</dt><dd>${fmtDate(item.mtime)}</dd>
      </dl></div>
    </div></div>`;
}

function Settings({ onRelink }) {
  const s = session();
  const tgu = tg.tgUser();
  useMainButton(null, []);

  async function revokeAll() {
    const ok = await tg.confirm({
      title: 'Завершить все сеансы?',
      message: 'Мини-приложение на других устройствах попросит открыть его заново. Здесь вы останетесь в аккаунте.',
      ok: 'Завершить',
    });
    if (!ok) return;
    try {
      await api('/sessions/revoke-all', { method: 'POST' });
      forgetLinks();
      await openSession();
      toast('Остальные сеансы завершены');
    } catch (e) {
      showError(e);
    }
  }

  async function unlink() {
    const ok = await tg.confirm({
      title: 'Отвязать Telegram?',
      message: `Вход в «Файлы» из этого Telegram перестанет работать до новой привязки. Файлы аккаунта ${s.user.username} останутся на месте.`,
      ok: 'Отвязать',
      destructive: true,
    });
    if (!ok) return;
    try {
      await api('/tg/unlink', { method: 'POST' });
      tg.haptic.warn();
      onRelink();
    } catch (e) {
      showError(e);
    }
  }

  return html`
    <div class="title"><div><h1>Настройки</h1></div></div>
    <p class="group-title"><span>Аккаунт</span></p>
    <div class="group"><dl class="kv">
      <dt>Пользователь</dt><dd>${s.user.username}${s.user.is_admin ? ' (администратор)' : ''}</dd>
      <dt>Telegram</dt><dd>${tgu && tgu.username ? `@${tgu.username}` : s.user.telegram_id}</dd>
      <dt>Лимит файла</dt><dd>${fmtBytes(s.limits.max_file_size)}</dd>
    </dl></div>
    <div class="group">
      <button class="row tap" type="button" onClick=${revokeAll}><span class="tile" style="--k:var(--c-other)"><${Icon} name="lock" /></span><span class="rt"><b>Завершить другие сеансы</b><small>Если открывали «Файлы» на чужом устройстве</small></span></button>
      <button class="row tap" type="button" onClick=${unlink}><span class="tile" style="--k:var(--danger)"><${Icon} name="x" /></span><span class="rt"><b class="danger-text">Отвязать Telegram</b></span></button>
    </div>
    <p class="note" style="margin-top:0">rust-file-manager ${s.version}. Вход по подписи Telegram; пароль сервер не видит.</p>`;
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

function ItemSheet({ item }) {
  const close = () => setUi({ sheet: null });
  const where = `${ZONE_TITLE[item.scope]} › ${catLabel(item.category)}${item.path ? ` › ${item.path}` : ''}`;
  const isFolder = item.type === 'folder';

  const openFile = () => viewFile(item);
  const download = () => downloadFile(item);
  async function rename() {
    const value = await ask({ title: isFolder ? 'Переименовать папку' : 'Переименовать файл', value: item.name, ok: 'Сохранить' });
    if (!value || value.trim() === item.name) return;
    try {
      await api(`/files/${item.id}/rename`, { method: 'POST', json: { new_name: value.trim() } });
      close();
      tg.haptic.ok();
      toast('Переименовано');
      changed();
    } catch (e) {
      showError(e);
    }
  }
  async function remove() {
    const ok = await tg.confirm({
      title: isFolder ? 'Удалить папку?' : 'Удалить файл?',
      message: isFolder
        ? `Папка «${item.name}» будет удалена. Удалить можно только пустую папку.`
        : `«${item.name}» будет удалён без возможности восстановления.`,
      ok: 'Удалить',
      destructive: true,
    });
    if (!ok) return;
    try {
      await api(`/files/${item.id}`, { method: 'DELETE' });
      close();
      tg.haptic.warn();
      toast(isFolder ? 'Папка удалена' : 'Файл удалён');
      changed();
    } catch (e) {
      showError(e);
    }
  }

  return html`<div class="layer" onClick=${(e) => e.target === e.currentTarget && close()}>
    <div class="sheet" role="dialog" aria-modal="true" aria-label=${item.name}>
      <div class="grab"></div>
      <div class="fhead"><${Thumb} item=${item} /><div><b>${item.name}</b>
        <small>${isFolder ? `${item.files} ${plural(item.files, 'файл', 'файла', 'файлов')} · ${fmtBytes(item.bytes)}` : `${fmtBytes(item.size)} · ${fmtDate(item.mtime)}`}</small></div></div>
      <div class="actions">
        ${isFolder
          ? html`<button class="act tap" type="button" onClick=${() => { close(); openItem(item); }}><${Icon} name="folder" />Открыть</button>`
          : html`<button class="act tap" type="button" onClick=${openFile}><${Icon} name="eye" />Открыть</button>
                 <button class="act tap" type="button" onClick=${download}><${Icon} name="download" />Скачать</button>`}
        <button class="act tap" type="button" onClick=${rename}><${Icon} name="edit" />Переимен.</button>
        <button class="act tap danger" type="button" onClick=${remove}><${Icon} name="trash" />Удалить</button>
      </div>
      <div class="group"><dl class="kv">
        <dt>Где</dt><dd>${where}</dd>
        ${isFolder
          ? html`<dt>Внутри</dt><dd>${item.files} ${plural(item.files, 'файл', 'файла', 'файлов')}, ${item.folders} ${plural(item.folders, 'папка', 'папки', 'папок')}</dd>`
          : html`<dt>Размер</dt><dd>${fmtBytes(item.size)}</dd><dt>Изменён</dt><dd>${fmtDate(item.mtime)}</dd>`}
      </dl></div>
    </div></div>`;
}

function Viewer({ items, index }) {
  const [i, setI] = useState(index);
  const [src, setSrc] = useState(null);
  const touch = useRef(null);
  const item = items[i];
  useEffect(() => {
    let alive = true;
    setSrc(null);
    linkFor(item, 'inline').then((url) => alive && setSrc(url)).catch(showError);
    return () => {
      alive = false;
    };
  }, [item.id]);
  const step = (d) => {
    const n = i + d;
    if (n >= 0 && n < items.length) {
      setI(n);
      tg.haptic.select();
    }
  };
  const download = async () => {
    try {
      tg.download(await linkFor(item, 'download'), item.name);
    } catch (e) {
      showError(e);
    }
  };
  return html`<div class="viewer" role="dialog" aria-modal="true" aria-label=${item.name}
    onTouchStart=${(e) => { touch.current = e.touches[0].clientX; }}
    onTouchEnd=${(e) => { const dx = e.changedTouches[0].clientX - (touch.current || 0); if (Math.abs(dx) > 50) step(dx < 0 ? 1 : -1); }}>
    <div class="vt">
      <button class="vbtn" type="button" aria-label="Закрыть" onClick=${() => setUi({ viewer: null })}><${Icon} name="x" /></button>
      <div>${item.name}<small>${i + 1} из ${items.length} · ${fmtBytes(item.size)}</small></div>
      <button class="vbtn" type="button" aria-label="Скачать" onClick=${download}><${Icon} name="download" /></button>
    </div>
    <div class="stage">${src ? html`<img src=${src} alt=${item.name} />` : html`<span class="spin"></span>`}</div>
    <div class="vb">
      <button class="vbtn" type="button" disabled=${i === 0} onClick=${() => step(-1)}><${Icon} name="left" />Назад</button>
      <button class="vbtn" type="button" disabled=${i === items.length - 1} onClick=${() => step(1)}>Далее<${Icon} name="right" /></button>
    </div>
  </div>`;
}

function Dialog({ dialog }) {
  const [value, setValue] = useState(dialog.value);
  const ref = useRef(null);
  useEffect(() => {
    if (!ref.current) return;
    ref.current.focus();
    // Select the name without the extension, as file managers do.
    const dot = dialog.value.lastIndexOf('.');
    ref.current.setSelectionRange(0, dot > 0 ? dot : dialog.value.length);
  }, []);
  const finish = (result) => {
    dialog.resolve(result);
    setUi({ dialog: null });
  };
  return html`<div class="dialog-wrap"><div class="dialog" role="dialog" aria-modal="true" aria-label=${dialog.title}>
    <form onSubmit=${(e) => { e.preventDefault(); if (value.trim()) finish(value); }}>
      <div class="db"><b>${dialog.title}</b>${dialog.message && html`<p>${dialog.message}</p>`}
        <input ref=${ref} class="field" value=${value} placeholder=${dialog.placeholder} aria-label=${dialog.title}
          onInput=${(e) => setValue(e.currentTarget.value)} /></div>
      <div class="dbtns"><button type="button" onClick=${() => finish(null)}>Отмена</button><button type="submit" disabled=${!value.trim()}>${dialog.ok}</button></div>
    </form></div></div>`;
}

function Toast({ toast: t }) {
  return html`<div class=${`toast ${t.kind}`} role="status"><${Icon} name=${t.kind === 'error' ? 'alert' : 'check-circle'} /><span>${t.text}</span></div>`;
}

function FallbackButtons() {
  const [, force] = useState(0);
  useEffect(() => tg.onFallbackChange(() => force((x) => x + 1)), []);
  const { main } = tg.fallbackButtons();
  if (!main) return null;
  return html`<div class="fallback-main"><button class="btn" type="button" disabled=${main.disabled} onClick=${main.onClick}>${main.text}</button></div>`;
}

// ---------------------------------------------------------------------------
// Linking / registration (first launch)
// ---------------------------------------------------------------------------

function inviteToken(text) {
  const raw = (text || '').trim();
  const m = raw.match(/startapp=inv_([A-Za-z0-9]+)/) || raw.match(/[?&]token=([A-Za-z0-9]+)/) || raw.match(/^(?:inv_)?([A-Za-z0-9]{20,})$/);
  return m ? m[1] : '';
}

function Link({ info, onDone }) {
  const tgu = tg.tgUser() || {};
  const [mode, setMode] = useState(info.invite_valid ? 'invite' : 'account');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [newName, setNewName] = useState(info.suggested_username || '');
  const [invite, setInvite] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit() {
    setError('');
    setBusy(true);
    try {
      if (mode === 'account') await linkAccount(username.trim(), password);
      else await register(newName.trim(), info.invite_valid ? '' : inviteToken(invite));
      tg.haptic.ok();
      onDone();
    } catch (e) {
      tg.haptic.error();
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }

  const ready = mode === 'account' ? username.trim() && password : newName.trim() && (info.invite_valid || inviteToken(invite));
  useMainButton(
    { text: mode === 'account' ? 'Привязать аккаунт' : 'Создать аккаунт', onClick: submit, disabled: !ready || busy, progress: busy },
    [mode, username, password, newName, invite, busy],
  );

  return html`
    <div class="hero">
      <span class="avatar" aria-hidden="true">${((tgu.first_name || '?')[0] || '?').toUpperCase()}</span>
      <b>${tgu.first_name || 'Добро пожаловать'}</b>
      ${tgu.username && html`<p>@${tgu.username} в Telegram</p>`}
      <p>Свяжите Telegram с аккаунтом файлового менеджера. Дальше вход будет без пароля.</p>
    </div>
    <div class="seg" role="group" aria-label="Способ входа">
      <button type="button" aria-pressed=${String(mode === 'account')} onClick=${() => { setMode('account'); setError(''); }}>У меня есть аккаунт</button>
      <button type="button" aria-pressed=${String(mode === 'invite')} onClick=${() => { setMode('invite'); setError(''); }}>Приглашение</button>
    </div>
    <form onSubmit=${(e) => { e.preventDefault(); if (ready && !busy) submit(); }}>
      ${mode === 'account'
        ? html`<div class="group form">
            <input autocomplete="username" autocapitalize="none" placeholder="Логин" aria-label="Логин" value=${username} onInput=${(e) => setUsername(e.currentTarget.value)} />
            <input type="password" autocomplete="current-password" placeholder="Пароль" aria-label="Пароль" value=${password} onInput=${(e) => setPassword(e.currentTarget.value)} />
          </div>
          ${error && html`<p class="form-error" role="alert">${error}</p>`}
          <p class="note">Пароль нужен один раз. Он остаётся для входа из браузера.</p>`
        : html`<div class="group form">
            ${info.invite_valid
              ? html`<div class="row"><span class="tile" style="--k:var(--c-prog)"><${Icon} name="check" /></span><span class="rt"><b>Приглашение найдено</b><small>Оно сработает один раз</small></span></div>`
              : html`<input autocapitalize="none" placeholder="Ссылка-приглашение" aria-label="Ссылка-приглашение" value=${invite} onInput=${(e) => setInvite(e.currentTarget.value)} />`}
            <input autocapitalize="none" placeholder="Имя пользователя" aria-label="Имя пользователя" value=${newName} onInput=${(e) => setNewName(e.currentTarget.value)} />
          </div>
          ${error && html`<p class="form-error" role="alert">${error}</p>`}
          <p class="note">Латинские буквы, цифры, «_» и «-», от 3 до 32 символов. Пароль не нужен: вход по Telegram.</p>`}
      <button type="submit" class="sr-only" tabindex="-1">Отправить</button>
    </form>`;
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/** Where a deep link asks to start: `?open=exchange[:<user>]` (bot notifications)
 * or `startapp=exchange`. Used once, on the first successful start. */
let deepLink = (() => {
  try {
    return new URLSearchParams(location.search).get('open') || tg.startParam() || '';
  } catch {
    return '';
  }
})();
function startStack(user) {
  const stack = [{ name: 'home' }];
  const link = deepLink;
  deepLink = '';
  if (link === 'exchange' || link.startsWith('exchange:')) {
    stack.push({ name: 'exchange' });
    const owner = link.slice('exchange:'.length);
    if (owner && user && user.is_admin) stack.push({ name: 'exchange', owner });
  }
  return stack;
}

function App() {
  const [phase, setPhase] = useState({ name: 'boot' });
  const n = nav.use();
  const overlays = ui.use();

  async function start() {
    setPhase({ name: 'boot' });
    if (!tg.inTelegram) return setPhase({ name: 'outside' });
    try {
      const s = await openSession();
      if (s.ok) {
        nav.set({ stack: startStack(session().user) });
        data.set({ version: data.state.version + 1, overview: null });
        setPhase({ name: 'ready' });
      } else setPhase({ name: 'link', info: s.notLinked });
    } catch (e) {
      setPhase({ name: 'error', error: e });
    }
  }
  useEffect(() => {
    tg.init();
    start();
  }, []);

  // BackButton: visible while there is something to go back from.
  const canGoBack = phase.name === 'ready' && (n.stack.length > 1 || overlayOpen());
  useEffect(() => {
    if (canGoBack) tg.backButton.show(back);
    else tg.backButton.hide();
  }, [canGoBack]);
  useEffect(() => {
    if (phase.name === 'ready') tg.settingsButton.show(() => go({ name: 'settings' }));
    else tg.settingsButton.hide();
  }, [phase.name]);
  useEffect(() => {
    if (phase.name === 'outside' || phase.name === 'error') {
      mainCfg = null;
      applyMain();
    }
  }, [phase.name]);

  if (phase.name === 'boot') return html`<div class="boot"><span class="spin"></span>Открываем файлы…</div>`;
  if (phase.name === 'outside') {
    return html`<div class="center"><${Icon} name="folder" /><b>Откройте через Telegram</b>
      <p>Это мини-приложение файлового менеджера. Откройте его кнопкой «Файлы» в чате с ботом.</p></div>`;
  }
  if (phase.name === 'error') {
    return html`<div class="center"><${Icon} name="alert" /><b>Не удалось открыть файлы</b><p>${phase.error.message}</p>
      <button class="btn" type="button" onClick=${start}>Повторить</button></div>`;
  }
  if (phase.name === 'link') return html`<${Link} info=${phase.info} onDone=${start} /><${FallbackButtons} />`;

  const screen = top();
  let content;
  if (screen.name === 'browse') content = html`<${Browse} screen=${screen} key=${`${screen.zone}|${screen.category}|${screen.path}|${n.stack.length}`} />`;
  else if (screen.name === 'uploads') content = html`<${Uploads} screen=${screen} />`;
  else if (screen.name === 'exchange') content = html`<${Exchange} screen=${screen} key=${screen.owner || ''} />`;
  else if (screen.name === 'settings') content = html`<${Settings} onRelink=${start} />`;
  else content = html`<${Home} />`;

  return html`${content}
    ${overlays.sheet && (overlays.sheet.item.direction
      ? html`<${ExchangeSheet} item=${overlays.sheet.item} key=${overlays.sheet.item.id} />`
      : html`<${ItemSheet} item=${overlays.sheet.item} key=${overlays.sheet.item.id} />`)}
    ${overlays.viewer && html`<${Viewer} ...${overlays.viewer} />`}
    ${overlays.dialog && html`<${Dialog} dialog=${overlays.dialog} />`}
    ${overlays.toast && html`<${Toast} toast=${overlays.toast} />`}
    <${FallbackButtons} />`;
}

window.addEventListener('unhandledrejection', (e) => {
  if (e.reason instanceof ApiError) showError(e.reason);
});

render(html`<${App} />`, document.getElementById('app'));
