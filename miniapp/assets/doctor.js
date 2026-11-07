// Mobile doctor workspace: existing Preact/htm, no new framework or build.
import { html, useEffect, useRef, useState } from './vendor/preact-htm.module.js';
import { Icon } from './widgets.js';
import { api, qs } from './api.js';
import { doctorRequest, readDoctorFile } from './doctor-api.js';
import * as tg from './tg.js';

const ROLES = { complaint: 'Жалоба', explanation: 'Пояснения врача', draft: 'Черновик ИИ', final: 'Окончательный ответ', template: 'Шаблон ответа' };
const STATUS = { needs_complaint: 'Нужна жалоба', needs_explanation: 'Нужны пояснения', ready: 'Можно составить ответ', generating: 'Ответ готовится', draft_ready: 'Проверьте черновик', final_ready: 'Ответ проверен', saved: 'В базе случаев', archived: 'В архиве' };
const DATE = new Intl.DateTimeFormat('ru', { day: 'numeric', month: 'short' });
const day = (s) => { const d = new Date(s); return Number.isNaN(d.getTime()) ? '' : DATE.format(d); };
const isWork = (item) => item.kind !== 'template' && !['saved', 'archived'].includes(item.status);
const args = (item) => ({ case_id: item.id, revision: item.revision });

function Notice({ text, error = false }) {
  return text && html`<div class=${`dr-notice ${error ? 'dr-error' : ''}`} role=${error ? 'alert' : 'status'}>${text}</div>`;
}

function ServerPicker({ onSelect, busy }) {
  const [zone, setZone] = useState('my');
  const [path, setPath] = useState('');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState(null);
  const [error, setError] = useState('');
  const [revision, reload] = useState(0);
  useEffect(() => {
    let alive = true;
    setItems(null);
    const timer = setTimeout(() => {
      api(`/files?${qs({ scope: zone, category: 'Документы', path, q: query, sort: 'mtime', order: 'desc' })}`)
        .then((r) => { if (alive) { setItems(r.items.filter((i) => i.type === 'folder' || i.name.toLowerCase().endsWith('.docx'))); setError(''); } })
        .catch((e) => alive && setError(e.message));
    }, 200);
    return () => { alive = false; clearTimeout(timer); };
  }, [zone, path, query, revision]);
  return html`<section aria-label="Выбор документа с сервера">
    <div class="dr-tabs">${['my', 'shared'].map((z) => html`<button type="button" aria-pressed=${zone === z} onClick=${() => { setZone(z); setPath(''); }}>${z === 'my' ? 'Мои документы' : 'Общие'}</button>`)}</div>
    <label class="dr-field">Поиск DOCX<input type="search" value=${query} placeholder="Название документа" onInput=${(e) => setQuery(e.target.value)} /></label>
    ${path && html`<button type="button" class="dr-back" onClick=${() => { setPath(path.split('/').slice(0, -1).join('/')); setQuery(''); }}>← Назад · ${path}</button>`}
    <${Notice} text=${error} error />${error && html`<button class="btn secondary" onClick=${() => reload(revision + 1)}>Повторить</button>`}
    ${!items && !error && html`<p class="dr-muted" role="status">Загружаю список…</p>`}
    ${items && !items.length && html`<p class="dr-muted">Документы .docx не найдены. Выберите другую папку или загрузите с устройства.</p>`}
    <div class="dr-list">${(items || []).map((item) => html`<button type="button" class="dr-row" key=${item.id} disabled=${busy} onClick=${() => item.type === 'folder' ? (setPath([item.path, item.name].filter(Boolean).join('/')), setQuery('')) : onSelect(item)}>
      <${Icon} name=${item.type === 'folder' ? 'folder' : 'file-text'} /><span><b>${item.name}</b><small>${item.type === 'folder' ? 'Открыть папку' : `${Math.ceil(item.size / 1024)} КБ · ${[item.category, item.path].filter(Boolean).join(' / ')}`}</small></span><${Icon} name="right" />
    </button>`)}</div>
  </section>`;
}

function Source({ item, role, onReview, onBack, busy, initialText = '' }) {
  const [source, setSource] = useState(initialText ? 'text' : '');
  const [file, setFile] = useState(null);
  const [text, setText] = useState(initialText);
  const fileRef = useRef(null);
  return html`<section>
    <button class="dr-back" type="button" disabled=${busy} onClick=${onBack}>← К обращению</button>
    <h2>${ROLES[role]}</h2>
    <p class="dr-muted">${role === 'template' ? 'Шаблон для будущих ответов' : item?.title}</p>
    ${!source && html`<h3>Откуда взять документ?</h3><div class="dr-list">
      <button class="dr-row" type="button" onClick=${() => setSource('server')}><${Icon} name="folder" /><span><b>С сервера</b><small>Уже загруженные документы</small></span><${Icon} name="right" /></button>
      <button class="dr-row" type="button" onClick=${() => { setSource('device'); fileRef.current?.click(); }}><${Icon} name="upload" /><span><b>С iPhone / iCloud Drive</b><small>Выбрать через приложение «Файлы»</small></span><${Icon} name="right" /></button>
      <button class="dr-row" type="button" onClick=${() => setSource('text')}><${Icon} name="edit" /><span><b>Написать или вставить текст</b><small>${role === 'explanation' ? 'Диагноз, лечение и объяснения пациенту' : 'Без загрузки файла'}</small></span><${Icon} name="right" /></button>
    </div>`}
    <input class="file-input" type="file" accept=".docx,application/vnd.openxmlformats-officedocument.wordprocessingml.document" ref=${fileRef} onChange=${(e) => { setFile(e.target.files[0] || null); setSource('device'); }} />
    ${source && html`<button type="button" class="dr-back" disabled=${busy} onClick=${() => setSource('')}>Выбрать другой источник</button>`}
    ${source === 'server' && html`<${ServerPicker} busy=${busy} onSelect=${(f) => onReview({ source_id: f.id })} />`}
    ${source === 'device' && html`<div class="dr-panel"><p>В окне выбора откройте «Обзор» → «На моём iPhone» или «iCloud Drive».</p>
      <p class="dr-muted">DOCX с текстом, до 5 МБ. На компьютере можно выбрать локальный файл.</p>
      <button type="button" class="btn secondary" disabled=${busy} onClick=${() => fileRef.current?.click()}>${file ? 'Выбрать другой файл' : 'Выбрать документ'}</button>
      ${file && html`<p class="dr-filename">${file.name}</p><button type="button" class="btn dr-full" disabled=${busy} onClick=${() => onReview({ file })}>Проверить обезличивание</button>`}
    </div>`}
    ${source === 'text' && html`<label class="dr-field">${ROLES[role]}<textarea rows="10" maxLength="100000" value=${text} placeholder=${role === 'explanation' ? 'Как было на самом деле: диагноз, почему выбрано лечение, что объясняли пациенту…' : 'Введите текст документа…'} onInput=${(e) => setText(e.target.value)} /></label>
      <button type="button" class="btn dr-full" disabled=${busy || !text.trim()} onClick=${() => onReview({ text })}>Проверить обезличивание</button>`}
    <p class="dr-muted">Сначала покажем текст для проверки. В обращение сохранится только подтверждённая версия.</p>
  </section>`;
}

function Review({ value, onChange, onConfirm, onCancel, busy }) {
  const [words, setWords] = useState('');
  const [numbers, setNumbers] = useState('');
  return html`<section><p class="dr-eyebrow">Проверка перед сохранением</p><h2>${ROLES[value.role]}</h2>
    <p>Убедитесь, что имена, контакты и другие личные сведения скрыты.</p>
    <div class="dr-document" tabindex="0">${value.text}</div>
    <details class="dr-panel"><summary>Замены (${value.replacements.length})</summary><ul>${value.replacements.map((line) => html`<li>${line}</li>`)}</ul></details>
    <details class="dr-panel"><summary>Скрыть ещё / вернуть замену</summary>
      <label class="dr-field">Что ещё скрыть<input value=${words} placeholder="Слова или фразы через запятую" onInput=${(e) => setWords(e.target.value)} /></label>
      <button class="btn secondary dr-full" type="button" disabled=${busy || !words.trim()} onClick=${() => onChange('review_hide', { phrases: words })}>Скрыть указанные фразы</button>
      <label class="dr-field">Номера замен, которые нужно вернуть<input value=${numbers} inputmode="numeric" placeholder="Например: 2, 5" onInput=${(e) => setNumbers(e.target.value)} /></label>
      <button class="btn secondary dr-full" type="button" disabled=${busy || !numbers.trim()} onClick=${() => onChange('review_restore', { numbers })}>Вернуть выбранные замены</button>
    </details>
    <div class="dr-sticky"><button class="btn dr-full" type="button" disabled=${busy} onClick=${onConfirm}>Всё обезличено — сохранить</button>
      <button class="btn secondary dr-full" type="button" disabled=${busy} onClick=${onCancel}>Отменить добавление</button></div>
  </section>`;
}

export function DoctorApp({ capability, initialFile, onFiles, useMainButton }) {
  const [view, setView] = useState(initialFile ? 'attach' : 'list');
  const [rows, setRows] = useState(null);
  const [item, setItem] = useState(null);
  const [role, setRole] = useState('complaint');
  const [review, setReview] = useState(null);
  const [tab, setTab] = useState('work');
  const [query, setQuery] = useState('');
  const [from, setFrom] = useState('');
  const [roleFilter, setRoleFilter] = useState('');
  const [stateFilter, setStateFilter] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState('');
  const [text, setText] = useState('');
  const lock = useRef(false);
  const refreshToken = useRef(0);
  useMainButton(null, []);

  async function perform(action) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(''); setNotice(''); setProgress('Обрабатываю…');
    try { await action(); tg.haptic.ok(); } catch (e) { setError(e.message); tg.haptic.error(); }
    finally { lock.current = false; setBusy(false); setProgress(''); }
  }
  async function load() {
    const n = ++refreshToken.current;
    try { const r = await doctorRequest('list', { q: query }); if (n === refreshToken.current) { setRows(r.items); setError(''); } }
    catch (e) { if (n === refreshToken.current) setError(e.message); }
  }
  useEffect(() => { const t = setTimeout(load, 200); return () => { clearTimeout(t); refreshToken.current += 1; }; }, [query]);
  const showItem = (value) => { setItem(value); setView('item'); setError(''); setText(''); };
  const open = (id) => perform(async () => showItem(await doctorRequest('get', { case_id: id })));
  const list = () => { setView('list'); setReview(null); setError(''); load(); };
  const add = (r, initial = '') => { setRole(r); setText(initial); setError(''); setView('source'); };
  const create = () => perform(async () => { const value = await doctorRequest('create'); setItem(value); setRole('complaint'); setText(''); setView(initialFile ? 'attach' : 'source'); load(); });
  async function startReview(payload, selected = item, selectedRole = role) {
    await perform(async () => {
      if (payload.file) payload = { content: await readDoctorFile(payload.file) };
      const value = await doctorRequest('review', { ...(selected ? args(selected) : {}), role: selectedRole, ...payload }, setProgress);
      setReview(value); setView('review'); setText('');
    });
  }
  const mutate = (op) => perform(async () => { showItem(await doctorRequest(op, args(item), setProgress)); load(); });
  const exportDoc = (r) => perform(async () => {
    const value = await doctorRequest('export', { case_id: item.id, role: r });
    const bytes = Uint8Array.from(atob(value.content), (c) => c.charCodeAt(0));
    const file = new File([bytes], value.name, { type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' });
    const url = URL.createObjectURL(file);
    const a = document.createElement('a'); a.href = url; a.download = value.name; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
    setNotice('Документ подготовлен. Если iPhone не сохранил файл, нажмите «В чат».');
  });
  const sendDoc = (r) => perform(async () => { const value = await doctorRequest('send_chat', { case_id: item.id, role: r }); setNotice(value.message); });

  let content;
  if (view === 'source') content = html`<${Source} key=${`${item?.id}:${role}`} item=${item} role=${role} busy=${busy} initialText=${text} onReview=${startReview} onBack=${() => item ? setView('item') : list()} />`;
  else if (view === 'review') content = html`<${Review} value=${review} busy=${busy}
    onChange=${(op, body) => perform(async () => setReview(await doctorRequest(op, { review_token: review.review_token, ...body })))}
    onConfirm=${() => perform(async () => { showItem(await doctorRequest('review_confirm', { review_token: review.review_token })); setReview(null); load(); })}
    onCancel=${() => perform(async () => { await doctorRequest('review_cancel', { review_token: review.review_token }); setReview(null); item ? setView('item') : list(); })} />`;
  else if (view === 'item' && item) {
    const editable = !item.read_only && isWork(item) && item.status !== 'generating';
    const roles = item.kind === 'template' ? ['template'] : ['complaint', 'explanation', 'draft', 'final'];
    content = html`<button type="button" class="dr-back" disabled=${busy} onClick=${list}>← Все обращения</button>
      <div class="dr-heading"><span class="dr-eyebrow">${item.kind === 'template' ? 'Шаблон' : `Обращение ${item.id.slice(1).toUpperCase()}`}</span><span class="dr-status">${STATUS[item.status]}</span></div>
      <h2>${item.title}</h2><p class="dr-muted">Обновлено ${day(item.updated)} · ${item.roles.length} документа</p>
      ${item.status === 'generating' && html`<${Notice} text="Ответ ещё готовится. Обновите карточку через некоторое время." />`}
      <div class="dr-documents">${roles.map((r, index) => html`<section class="dr-panel" key=${r}><div class="dr-slot"><span class=${`dr-step ${item.documents[r] ? 'dr-done' : ''}`}>${item.documents[r] ? '✓' : index + 1}</span><div><h3>${ROLES[r]}</h3><small class="dr-muted">${item.documents[r] ? (r === 'draft' ? 'Проверьте перед использованием' : 'Проверено и сохранено') : 'Пока не добавлен'}</small></div></div>
        ${item.documents[r] && html`<details><summary>Посмотреть текст</summary><div class="dr-document">${item.documents[r]}</div></details><div class="dr-actions"><button type="button" disabled=${busy} onClick=${() => exportDoc(r)}><${Icon} name="download" />Скачать</button><button type="button" disabled=${busy} onClick=${() => sendDoc(r)}><${Icon} name="send" />В чат</button></div>`}
        ${editable && r !== 'draft' && html`<button type="button" class="btn secondary dr-full" disabled=${busy} onClick=${() => add(r)}>${item.documents[r] ? 'Заменить документ' : `Добавить: ${ROLES[r].toLowerCase()}`}</button>`}
        ${editable && r === 'draft' && item.documents.draft && html`<button type="button" class="btn dr-full" disabled=${busy} onClick=${() => add('final', item.documents.draft)}>Проверить и оформить ответ</button>`}
      </section>`)}</div>
      ${item.notes.length > 0 && html`<details class="dr-panel"><summary>Что проверить в ответе (${item.notes.length})</summary><ul>${item.notes.map((n) => html`<li>${n}</li>`)}</ul></details>`}
      ${editable && item.documents.complaint && item.documents.explanation && !item.documents.final && html`<button type="button" class="btn dr-full" disabled=${busy} onClick=${() => mutate('generate')}>${item.documents.draft ? 'Составить новый черновик' : 'Составить ответ с ИИ'}</button>`}
      ${editable && item.documents.complaint && item.documents.final && html`<button type="button" class="btn dr-full" disabled=${busy} onClick=${() => mutate('save')}>Сохранить случай в базу примеров</button><p class="dr-muted">После сохранения этот случай поможет составлять будущие ответы.</p>`}
      <div class="dr-actions"><button type="button" disabled=${busy} onClick=${() => open(item.id)}>Обновить</button>${!item.read_only && html`<button type="button" disabled=${busy} onClick=${() => mutate('archive')}>${item.status === 'archived' ? 'Вернуть из архива' : 'В архив'}</button>`}</div>`;
  } else if (view === 'attach' && initialFile) {
    const options = (rows || []).filter(isWork);
    content = html`<button type="button" class="dr-back" onClick=${onFiles}>← К файлам</button><h2>Добавить в помощь врача</h2><p class="dr-filename">${initialFile.name}</p>
      <label class="dr-field">Назначение документа<select value=${role} onChange=${(e) => setRole(e.target.value)}>${['complaint', 'explanation', 'final', 'template'].map((r) => html`<option value=${r}>${ROLES[r]}</option>`)}</select></label>
      ${role === 'template' ? html`<button class="btn dr-full" disabled=${busy} onClick=${() => startReview({ source_id: initialFile.id }, null, role)}>Проверить и добавить шаблон</button>` : html`
      <h3>К какому обращению?</h3>${item && !options.some((v) => v.id === item.id) && html`<button class="dr-row" disabled=${busy} onClick=${() => startReview({ source_id: initialFile.id }, item, role)}>${item.title}</button>`}
      <div class="dr-list">${options.map((r) => html`<button class="dr-row" disabled=${busy} onClick=${() => perform(async () => { const full = await doctorRequest('get', { case_id: r.id }); setItem(full); const value = await doctorRequest('review', { ...args(full), role, source_id: initialFile.id }); setReview(value); setView('review'); })}><span><b>${r.title}</b><small>${day(r.updated)} · ${STATUS[r.status]}</small></span><${Icon} name="right" /></button>`)}</div>
      <button class="btn secondary dr-full" disabled=${busy} onClick=${create}>Создать новое обращение</button>`}`;
  } else {
    const filtered = (rows || []).filter((r) => (tab === 'work' ? isWork(r) : tab === 'base' ? r.kind === 'case' && r.status === 'saved' : tab === 'templates' ? r.kind === 'template' : r.status === 'archived')
      && (!from || r.updated.slice(0, 10) >= from) && (!roleFilter || r.roles.includes(roleFilter)) && (!stateFilter || r.status === stateFilter));
    content = html`<div class="dr-heading"><div><p class="dr-eyebrow">Рабочее пространство</p><h1>Помощь врача</h1></div><button type="button" class="dr-refresh" disabled=${busy} aria-label="Обновить обращения" onClick=${load}><${Icon} name="refresh" /></button></div>
      <button type="button" class="btn dr-full" disabled=${busy} onClick=${tab === 'templates' ? () => { setItem(null); add('template'); } : create}>${tab === 'templates' ? '+ Добавить шаблон' : '+ Новое обращение'}</button>
      <div class="dr-tabs dr-sections" aria-label="Разделы помощи врача">${[['work', 'В работе'], ['base', 'База случаев'], ['templates', 'Шаблоны'], ['archive', 'Архив']].map(([value, label]) => html`<button type="button" aria-pressed=${tab === value} onClick=${() => { setTab(value); setStateFilter(''); }}>${label}</button>`)}</div>
      <label class="dr-field dr-search"><span class="sr-only">Поиск обращений</span><input type="search" placeholder="Найти по названию или тексту" value=${query} onInput=${(e) => setQuery(e.target.value)} /></label>
      <details class="dr-filters"><summary>Фильтры${from || roleFilter || stateFilter ? ' · включены' : ''}</summary><div>
        <label class="dr-field">Обновлено начиная с<input type="date" value=${from} onChange=${(e) => setFrom(e.target.value)} /></label>
        <label class="dr-field">Есть документ<select value=${roleFilter} onChange=${(e) => setRoleFilter(e.target.value)}><option value="">Любой</option>${Object.entries(ROLES).map(([r, label]) => html`<option value=${r}>${label}</option>`)}</select></label>
        <label class="dr-field">Статус<select value=${stateFilter} onChange=${(e) => setStateFilter(e.target.value)}><option value="">Все статусы</option>${Object.entries(STATUS).map(([s, label]) => html`<option value=${s}>${label}</option>`)}</select></label>
        <button class="dr-back" type="button" onClick=${() => { setFrom(''); setRoleFilter(''); setStateFilter(''); }}>Сбросить фильтры</button>
      </div></details>
      <p class="dr-count">${rows ? `${filtered.length} записей · новые сверху` : 'Загружаю обращения…'}</p>
      <div class="dr-cases">${filtered.map((r) => html`<button type="button" class="dr-case" disabled=${busy} key=${r.id} onClick=${() => open(r.id)}><div class="dr-heading"><small>${day(r.updated)} · ${r.id.slice(1).toUpperCase()}</small><span class="dr-status">${r.kind === 'template' ? 'Шаблон' : STATUS[r.status]}</span></div><b>${r.title}</b><div class="dr-chips">${r.roles.map((role) => html`<span>${ROLES[role]}</span>`)}</div><span class="dr-open">Открыть <${Icon} name="right" /></span></button>`)}</div>
      ${rows && !filtered.length && html`<div class="dr-empty"><${Icon} name="file-text" /><h3>${query || from || roleFilter || stateFilter ? 'Ничего не найдено' : tab === 'work' ? 'Начните с жалобы' : 'Здесь пока нет документов'}</h3><p>${tab === 'work' ? 'Добавьте жалобу и пояснения — подготовим черновик ответа. Все документы будут в одной карточке.' : tab === 'base' ? 'Сохранённые в чате и приложении случаи появятся здесь.' : 'Добавленные документы появятся в этом разделе.'}</p></div>`}`;
  }
  return html`<main class="doctor">${capability?.banner && html`<${Notice} text=${capability.banner} />`}
    <${Notice} text=${error} error /><${Notice} text=${notice} />${busy && html`<${Notice} text=${progress} />`}${content}
  </main>`;
}
