// One explicit next action; generating and archived cases never offer mutations accidentally.
export function caseGuide(item) {
  const docs = item.documents || {};
  if (item.kind === 'template') return { step: null, title: 'Шаблон ответа', hint: 'Откройте текст или сохраните документ.', action: 'template' };
  if (item.status === 'archived') return { step: null, title: 'Обращение в архиве', hint: 'Документы доступны для просмотра и скачивания.', action: item.read_only ? null : 'restore' };
  if (item.status === 'generating') return { step: 2, title: 'Готовим ответ', hint: 'Можно открыть другие файлы. Черновик сохранится в этом обращении.', action: 'waiting' };
  if (item.status === 'saved' || item.read_only) return { step: null, title: 'Ответ готов', hint: 'Случай сохранён в базе. Ответ можно скачать или отправить себе в чат.', action: 'deliver' };
  if (!docs.complaint) return { step: 0, title: 'Добавьте жалобу', hint: 'Выберите файл или вставьте текст обращения пациента.', action: 'complaint' };
  if (!docs.explanation) return { step: 1, title: 'Что было на самом деле?', hint: 'Напишите диагноз, почему выбрано лечение и что объясняли пациенту.', action: 'explanation' };
  if (docs.final) return { step: 2, title: 'Ответ проверен', hint: 'Осталось сохранить случай в базу — он поможет в будущих обращениях.', action: 'save' };
  if (docs.draft) return { step: 2, title: 'Черновик готов', hint: 'Проверьте формулировки и медицинские факты, при необходимости исправьте текст.', action: 'review' };
  return { step: 2, title: 'Всё готово для ответа', hint: 'Жалоба и пояснения добавлены. Подготовим черновик на их основе.', action: 'generate' };
}
