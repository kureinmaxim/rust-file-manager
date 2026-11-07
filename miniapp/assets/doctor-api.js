// All medical content stays in memory. Never store it in localStorage or URLs.
import { api, ApiError } from './api.js';

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export async function doctorRequest(operation, payload = {}, onProgress = () => {}) {
  const random = new Uint8Array(16);
  crypto.getRandomValues(random);
  const request_id = Array.from(random, (b) => b.toString(16).padStart(2, '0')).join('');
  const command = { request_id, operation, payload };
  let submitted;
  // A lost POST response is retried with the SAME id: never generate twice.
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      submitted = await api('/doctor/jobs', { method: 'POST', json: command });
      break;
    } catch (e) {
      if (e.status || attempt) throw e;
      await pause(600);
    }
  }
  const until = Date.now() + 240_000;
  while (Date.now() < until) {
    let result;
    try {
      result = await api(`/doctor/jobs/${encodeURIComponent(submitted.job)}`);
    } catch (e) {
      if (e.status) throw e;
      onProgress('Связь прервалась. Проверяем результат…');
      await pause(1500);
      continue;
    }
    if (result.state === 'done') {
      if (!result.result || !result.result.ok) {
        throw new ApiError(result.result?.message || 'Действие не выполнено. Повторите.');
      }
      return result.result.data;
    }
    onProgress(operation === 'generate' ? 'Составляю ответ. Можно открыть другие файлы — работа продолжится.' : 'Обрабатываю…');
    await pause(500);
  }
  throw new ApiError('Запрос ещё не завершён. Откройте обращение заново и проверьте результат перед повтором.');
}

export function readDoctorFile(file) {
  if (!file || !file.name.toLowerCase().endsWith('.docx')) throw new Error('Выберите документ .docx.');
  if (file.size > 5 * 1024 * 1024) throw new Error('Документ больше 5 МБ.');
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error('Не удалось прочитать файл. Выберите его ещё раз.'));
    reader.onload = () => resolve(String(reader.result).split(',', 2)[1]);
    reader.readAsDataURL(file);
  });
}
