// «Загрузить с ПК»: the page a computer opens at /pc. It shows a QR code and a
// short code; once the phone approves it in the Mini App, files dropped here
// go into the owner's «Мои файлы» (category by file type) through the same
// chunked upload API. The page gets nothing else: it never sees the files.

import { html, render, useEffect, useRef, useState } from './vendor/preact-htm.module.js';
import { api, ApiError, useComputerSession } from './api.js';
import { fmtBytes, plural } from './format.js';
import * as up from './upload.js';
import { CapacityCard, Icon, QueueRow } from './widgets.js';

const DEST = { scope: 'my', category: null, path: '' };
/** A forgotten tab stops asking for new codes after about half an hour. */
const MAX_AUTO_CODES = 6;

// The stylesheet uses Telegram's theme variables; here the system decides.
const darkQuery = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;
const applyScheme = () => {
  document.documentElement.dataset.scheme = darkQuery && darkQuery.matches ? 'dark' : 'light';
};
applyScheme();
if (darkQuery && darkQuery.addEventListener) darkQuery.addEventListener('change', applyScheme);

/** Random id of this browser: its unfinished uploads resume after reconnecting. */
function deviceId() {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  const fresh = () => Array.from(crypto.getRandomValues(new Uint8Array(32)), (b) => alphabet[b % alphabet.length]).join('');
  try {
    let id = localStorage.getItem('pc_device') || '';
    if (!/^[A-Za-z0-9]{32}$/.test(id)) {
      id = fresh();
      localStorage.setItem('pc_device', id);
    }
    return id;
  } catch {
    return fresh(); // no storage: uploads resume only within this connection
  }
}

async function post(path, json) {
  let res;
  try {
    res = await fetch(`/api/v1${path}`, {
      method: 'POST',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify(json),
    });
  } catch {
    throw new ApiError('Нет связи с сервером. Проверьте интернет и повторите.', 0, 'network');
  }
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(body.message || `Ошибка ${res.status}`, res.status, body.code, body);
  return body;
}

function useQueue() {
  const [, force] = useState(0);
  useEffect(() => up.subscribe(() => force((x) => x + 1)), []);
  return up.queue;
}

const mmss = (sec) => `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`;

function ErrorBox({ error, onRetry }) {
  return html`<div class="error-box" role="alert"><${Icon} name="alert" /><span>${error.message}</span>
    ${onRetry && html`<button type="button" onClick=${onRetry}>Повторить</button>`}</div>`;
}

function Head({ sub, children }) {
  return html`<header class="pc-head">
    <span class="tile" style="--k:var(--c-doc)"><${Icon} name="monitor" /></span>
    <div><h1>Загрузка с компьютера</h1><small>${sub}</small></div>
    ${children}
  </header>`;
}

function Qr({ qr }) {
  if (!qr) return html`<div class="qr-box empty"><span>QR-код недоступен — введите код вручную</span></div>`;
  const side = qr.size + 8; // quiet zone of 4 modules on each side
  // Always dark on white: inverted codes confuse some scanners.
  return html`<div class="qr-box"><svg viewBox=${`-4 -4 ${side} ${side}`} role="img" aria-label="QR-код для подключения телефона" shape-rendering="crispEdges">
    <rect x="-4" y="-4" width=${side} height=${side} fill="#fff" /><path d=${qr.path} fill="#000" /></svg></div>`;
}

function Pairing({ onConnected }) {
  const [pair, setPair] = useState(null);
  const [left, setLeft] = useState(0);
  const [error, setError] = useState(null);
  const [expired, setExpired] = useState(false);
  const codes = useRef(0);
  const device = useRef(deviceId());

  async function fresh() {
    setError(null);
    setExpired(false);
    setPair(null);
    try {
      const p = await post('/pc/pair', { device: device.current });
      codes.current += 1;
      setLeft(p.expires_in);
      setPair(p);
    } catch (e) {
      setError(e);
    }
  }
  useEffect(() => {
    fresh();
  }, []);

  useEffect(() => {
    if (!pair) return undefined;
    const started = Date.now();
    const timer = setInterval(() => setLeft(Math.max(0, pair.expires_in - Math.floor((Date.now() - started) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [pair]);

  // Ask whether the phone has approved; the token comes exactly once.
  useEffect(() => {
    if (!pair) return undefined;
    let alive = true;
    let timer;
    const tick = async () => {
      try {
        const r = await post('/pc/poll', { secret: pair.secret });
        if (!alive) return;
        if (r.status === 'approved') {
          onConnected(r);
          return;
        }
      } catch (e) {
        if (!alive) return;
        if (e.code === 'pair_expired') {
          if (codes.current < MAX_AUTO_CODES && document.visibilityState === 'visible') fresh();
          else setExpired(true);
          return;
        }
        // A network hiccup: keep asking.
      }
      timer = setTimeout(tick, (pair.interval || 2) * 1000);
    };
    timer = setTimeout(tick, (pair.interval || 2) * 1000);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [pair]);

  let qr;
  if (error) qr = html`<div class="qr-box empty"><${ErrorBox} error=${error} onRetry=${fresh} /></div>`;
  else if (expired) {
    qr = html`<div class="qr-box empty"><span>Код устарел</span>
      <button class="btn" type="button" onClick=${() => { codes.current = 0; fresh(); }}><${Icon} name="refresh" />Новый код</button></div>`;
  } else if (!pair) qr = html`<div class="qr-box empty"><span class="spin" aria-label="Готовим код"></span></div>`;
  else qr = html`<${Qr} qr=${pair.qr} />`;

  return html`
    <${Head} sub="Файлы попадут в «Мои файлы» — по типу, как при загрузке с телефона" />
    <section class="group pair">
      ${qr}
      <div class="how">
        <ol class="steps">
          <li><b>Откройте «Файлы» в Telegram на телефоне</b><small>Кнопка «Файлы» в чате с ботом → «Загрузки» → «С компьютера»</small></li>
          <li><b>Нажмите «Сканировать QR-код»</b><small>и наведите камеру на код. Подойдёт и камера телефона — она откроет Telegram.</small></li>
          <li><b>Подтвердите подключение на телефоне</b><small>Эта страница сама перейдёт к загрузке файлов</small></li>
        </ol>
        ${pair && !expired && html`<div class="code">
          <small>Код для ручного ввода</small>
          <b class="mono" aria-label=${`Код ${pair.code.split('').join(' ')}`}>${pair.code}</b>
          <small>${left > 0 ? `действует ещё ${mmss(left)}` : 'обновляем…'}</small>
        </div>`}
      </div>
    </section>
    <p class="note">Компьютер сможет только добавлять файлы — ваши файлы он не увидит. Подключение закроется через 30 минут без загрузок или кнопкой «Отключить» здесь или на телефоне.</p>`;
}

function Connected({ info, onEnd }) {
  const queue = useQueue();
  const [over, setOver] = useState(false);
  const [computer, setComputer] = useState(info.computer);
  const filesRef = useRef(null);
  const busy = queue.some((i) => i.state === 'up' || i.state === 'wait');

  const add = (list) => {
    const files = Array.from(list || []);
    if (files.length) up.addFiles(files, DEST);
  };

  useEffect(() => {
    up.refreshCapacity();
    // The session's end time moves with uploads; a 401 here means the phone
    // disconnected this computer (api.js then calls onEnd).
    const timer = setInterval(() => {
      api('/pc/session')
        .then((r) => setComputer(r.computer))
        .catch(() => {});
    }, 30000);
    return () => clearInterval(timer);
  }, []);

  // Closing the tab stops the uploads: ask first.
  useEffect(() => {
    if (!busy) return undefined;
    const guard = (e) => {
      e.preventDefault();
      e.returnValue = '';
    };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  }, [busy]);

  // Files can be dropped anywhere on the page.
  useEffect(() => {
    const hasFiles = (e) => Array.from((e.dataTransfer && e.dataTransfer.types) || []).includes('Files');
    const enter = (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      setOver(true);
    };
    const leave = (e) => {
      if (!e.relatedTarget) setOver(false);
    };
    const drop = (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      setOver(false);
      add(e.dataTransfer.files);
    };
    window.addEventListener('dragover', enter);
    window.addEventListener('dragleave', leave);
    window.addEventListener('drop', drop);
    return () => {
      window.removeEventListener('dragover', enter);
      window.removeEventListener('dragleave', leave);
      window.removeEventListener('drop', drop);
    };
  }, []);

  async function disconnect() {
    if (busy && !window.confirm('Остановить загрузки и отключить компьютер?')) return;
    await Promise.all(up.active().map((item) => up.cancel(item)));
    try {
      await api('/pc/session', { method: 'DELETE' });
    } catch {
      /* already disconnected */
    }
    onEnd('Компьютер отключён. Чтобы загрузить ещё, подключите его снова.');
  }

  const ends = new Date(computer.ends_at * 1000);
  const endsAt = `${String(ends.getHours()).padStart(2, '0')}:${String(ends.getMinutes()).padStart(2, '0')}`;
  const done = queue.filter((i) => i.state === 'ok');
  const where = (item) => `готово → ${item.result.category}`;

  return html`
    <${Head} sub=${`Аккаунт ${info.user.username} · без загрузок отключится в ${endsAt}`}>
      <button class="btn secondary" type="button" onClick=${disconnect}><${Icon} name="x" />Отключить</button>
    <//>
    <div class=${`drop big ${over ? 'over' : ''}`}>
      <${Icon} name="upload" />
      <b>Перетащите файлы в это окно</b>
      <small>Фото попадут в «Фото», документы — в «Документы» и так далее. Большие файлы идут частями: обрыв связи не начнёт загрузку заново.</small>
      <div class="picks">
        <input id="pick-files" class="file-input" type="file" multiple ref=${filesRef} onChange=${(e) => { add(e.currentTarget.files); e.currentTarget.value = ''; }} />
        <label class="btn" for="pick-files"><${Icon} name="file" />Выбрать файлы</label>
      </div>
    </div>
    <${CapacityCard} longHint="Не закрывайте вкладку и не давайте компьютеру уснуть. Если загрузка прервётся, подключитесь снова и выберите тот же файл — она продолжится с места остановки." />
    ${queue.length > 0 && html`
      <p class="group-title"><span>Очередь</span>${done.length > 0 && html`<button type="button" onClick=${() => up.clearFinished()}>Убрать готовые</button>`}</p>
      <div class="group">${queue.map((item) => html`<${QueueRow} item=${item} where=${where} key=${item.key} />`)}</div>`}
    ${done.length > 0 && html`<p class="note">Загружено: ${done.length} ${plural(done.length, 'файл', 'файла', 'файлов')}, ${fmtBytes(done.reduce((n, i) => n + i.size, 0))} — уже в «Мои файлы» на телефоне.</p>`}`;
}

function Ended({ reason, onAgain }) {
  return html`<div class="center"><${Icon} name="monitor" /><b>Компьютер отключён</b><p>${reason}</p>
    <button class="btn" type="button" onClick=${onAgain}><${Icon} name="refresh" />Подключить снова</button></div>`;
}

function App() {
  const [phase, setPhase] = useState({ name: 'pairing' });
  const end = (reason) => setPhase({ name: 'ended', reason });
  const connected = (body) => {
    useComputerSession(body, () =>
      end('Подключение закрыто: его отключили на телефоне, прошло 30 минут без загрузок или сервер перезапустился. Незавершённые загрузки продолжатся после нового подключения — выберите те же файлы.'),
    );
    setPhase({ name: 'connected', info: body });
  };
  if (phase.name === 'connected') return html`<${Connected} info=${phase.info} onEnd=${end} />`;
  if (phase.name === 'ended') return html`<${Ended} reason=${phase.reason} onAgain=${() => setPhase({ name: 'pairing', key: Date.now() })} />`;
  return html`<${Pairing} onConnected=${connected} key=${phase.key || 0} />`;
}

const root = document.getElementById('app');
root.textContent = ''; // the static «Готовим код…» placeholder
render(html`<${App} />`, root);
