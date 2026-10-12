// UI pieces shared by the Mini App (app.js) and the computer's page (pc.js):
// icons, the upload queue row and the free space / time left card. File names
// are only ever rendered as text nodes.

import { html } from './vendor/preact-htm.module.js';
import { extOf, fmtBytes, fmtDuration, guessKind, KIND_COLOR, KIND_ICON, LONG_UPLOAD_SECS, plural } from './format.js';
import * as up from './upload.js';

export function Icon({ name, cls = 'icon' }) {
  return html`<svg class=${cls} aria-hidden="true"><use href=${`#i-${name}`} /></svg>`;
}

/** File type tile without a preview: local files in the queue. */
export function FileTile({ name, kind }) {
  const ext = extOf(name);
  const color = KIND_COLOR[kind] || 'var(--c-other)';
  return html`<span class=${`ftile ${ext ? 'has-ext' : ''}`} style=${`--k:${color}`}>
    <${Icon} name=${KIND_ICON[kind] || 'file'} />${ext && html`<em>${ext}</em>`}</span>`;
}

/** One file of the upload queue. `where(item)` names the destination of a
 * finished upload (category and folder). */
export function QueueRow({ item, where }) {
  const pct = item.size ? Math.min(100, Math.round(((item.sent + item.inFlight) / item.size) * 100)) : 100;
  let state;
  let bar = null;
  if (item.state === 'up') {
    const rest = Math.max(0, item.size - item.sent - item.inFlight);
    const eta = item.speed ? ` · осталось ${fmtDuration(rest / item.speed)}` : '';
    state = `${pct} %${item.speed ? ` · ${fmtBytes(item.speed)}/с` : ''}${eta}`;
    bar = html`<span class="bar"><i style=${`width:${pct}%`}></i></span>`;
  } else if (item.state === 'wait') state = 'ожидает';
  else if (item.state === 'ok') state = where(item);
  else if (item.state === 'err') {
    state = item.error;
    bar = html`<span class="bar err"><i style=${`width:${pct}%`}></i></span>`;
  } else state = 'отменено';
  const kind = item.result ? item.result.kind : guessKind(item.name);
  return html`<div class="row wide">
    <${FileTile} name=${item.name} kind=${kind} />
    <span class="rt"><b>${item.name}</b><small class=${item.state === 'err' ? 'danger-text' : ''}>${fmtBytes(item.size)} · ${state}${item.note ? ` · ${item.note}` : ''}</small>${bar}</span>
    ${item.state === 'ok' && html`<span class="qstate ok"><${Icon} name="check" /></span>`}
    ${item.state === 'err' && !item.fatal && html`<button class="tbtn" type="button" onClick=${() => up.retry(item)}><${Icon} name="refresh" />Повторить</button>`}
    ${(item.state === 'up' || item.state === 'wait') && html`<button class="ib" type="button" aria-label=${`Отменить загрузку ${item.name}`} onClick=${() => up.cancel(item)}><${Icon} name="x" /></button>`}
  </div>`;
}

/** How much the server takes now and how long the queue will take. `longHint`
 * is shown under the card when the rest needs more than an hour. */
export function CapacityCard({ longHint }) {
  const cap = up.capacity();
  const est = up.estimate();
  if (!cap && !est.count) return null;

  let space = null;
  if (cap) {
    const free = cap.available_bytes;
    const known = typeof free === 'number';
    const perFile = known ? Math.min(free, cap.max_file_size) : cap.max_file_size;
    const low = known && free < cap.max_file_size;
    space = html`<div class="row">
      <span class="tile" style=${`--k:${low ? 'var(--c-warn)' : 'var(--c-prog)'}`}><${Icon} name="disk" /></span>
      <span class="rt"><b>${known ? `Свободно для загрузок: ${fmtBytes(free)}` : `Файл до ${fmtBytes(cap.max_file_size)}`}</b>
        <small>${known
          ? `Один файл — до ${fmtBytes(perFile)}${low ? ' (столько осталось на сервере)' : ''}`
          : 'Свободное место на сервере неизвестно'}</small></span>
    </div>`;
  }

  let time = null;
  const long = est.seconds !== null && est.seconds > LONG_UPLOAD_SECS;
  const tooBig = cap && typeof cap.available_bytes === 'number' && est.remaining > cap.available_bytes;
  if (est.count) {
    const files = `${est.count} ${plural(est.count, 'файл', 'файла', 'файлов')}, ${fmtBytes(est.remaining)}`;
    let title;
    if (est.seconds === null) title = 'Измеряем скорость…';
    else if (!est.measured) title = `Около ${fmtDuration(est.seconds)} при прошлой скорости`;
    else title = `${long ? 'Долго: осталось' : 'Осталось'} ${est.seconds < 60 ? '' : '≈ '}${fmtDuration(est.seconds)}`;
    const color = long || tooBig ? 'var(--c-warn)' : 'var(--accent)';
    time = html`<div class="row">
      <span class="tile" style=${`--k:${color}`}><${Icon} name="clock" /></span>
      <span class="rt"><b>${title}</b><small>${files}${est.speed ? ` · ${fmtBytes(est.speed)}/с` : ''}</small></span>
    </div>`;
  }

  return html`<div class="group" aria-live="polite">${space}${time}</div>
    ${tooBig && html`<p class="note warn-text">В очереди больше, чем свободно на сервере: часть файлов не поместится.</p>`}
    ${long && !tooBig && longHint && html`<p class="note warn-text">${longHint}</p>`}`;
}
