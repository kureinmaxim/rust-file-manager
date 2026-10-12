// Formatting shared by the Mini App (app.js) and the computer's page (pc.js).

export function fmtBytes(n) {
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

export const plural = (n, one, few, many) => {
  const a = n % 10;
  const b = n % 100;
  return a === 1 && b !== 11 ? one : a >= 2 && a <= 4 && (b < 12 || b > 14) ? few : many;
};

/** «меньше минуты», «12 мин», «1 ч 25 мин». */
export function fmtDuration(sec) {
  if (sec < 60) return 'меньше минуты';
  const min = Math.round(sec / 60);
  if (min < 60) return `${min} мин`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  return m ? `${h} ч ${m} мин` : `${h} ч`;
}

/** An upload counts as long from an hour on: worth a warning. */
export const LONG_UPLOAD_SECS = 60 * 60;

export const extOf = (name) => {
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
export const guessKind = (name) => {
  const ext = (name.split('.').pop() || '').toLowerCase();
  return Object.keys(EXT_KIND).find((k) => EXT_KIND[k].includes(ext)) || 'other';
};

export const KIND_ICON = { image: 'image', video: 'film', audio: 'music', pdf: 'file-text', text: 'file-text', archive: 'archive', other: 'file' };
export const KIND_COLOR = { image: 'var(--c-photo)', video: 'var(--c-video)', audio: 'var(--c-video)', pdf: 'var(--c-doc)', text: 'var(--c-doc)', archive: 'var(--c-backup)', other: 'var(--c-other)' };

/** The code of a computer («Загрузить с ПК») from a scanned link
 * (`…startapp=pc_ABCD2345`) or from typing: same rules as the server. */
export function computerCode(text) {
  let raw = String(text || '');
  const at = raw.indexOf('pc_');
  if (at >= 0) raw = raw.slice(at + 3);
  raw = raw.split(/[&#/]/)[0];
  const code = raw
    .replace(/[^0-9a-z]/gi, '')
    .toUpperCase()
    .replace(/O/g, '0')
    .replace(/[IL]/g, '1');
  return /^[0-9A-HJKMNP-TV-Z]{8}$/.test(code) ? code : '';
}
