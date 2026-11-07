// Run against browser_harness and a synthetic DoctorMobile worker, as documented in DOCTOR_WORKSPACE.md.
const { chromium } = require('playwright');
const fs = require('node:fs');
const assert = require('node:assert/strict');
(async () => {
  const runtime = JSON.parse(fs.readFileSync(process.env.DOCTOR_UI_RUNTIME));
  const browser = await chromium.launch({ headless: true, ...(process.env.DOCTOR_CHROME ? { executablePath: process.env.DOCTOR_CHROME } : {}) });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, acceptDownloads: true });
  const errors = [], writes = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('request', (r) => { if (r.url().endsWith('/doctor/jobs') && r.method() === 'POST') writes.push(r.postDataJSON().operation); });
  await page.route('https://telegram.org/**', (route) => route.fulfill({ body: '' }));
  await page.addInitScript(({ data }) => { window.Telegram = { WebApp: {
    initData: data, initDataUnsafe: { user: { first_name: 'Врач' } }, themeParams: {},
    ready() {}, expand() {}, onEvent() {}, isVersionAtLeast(version) { return version === '8.0'; },
    downloadFile({ url, file_name }) {
      const link = document.createElement('a');
      link.href = url; link.download = file_name; document.body.append(link); link.click(); link.remove();
    },
  } }; }, { data: runtime.init_data });
  async function enterText(text) {
    await page.getByRole('button', { name: 'Написать или вставить текст', exact: false }).click();
    await page.getByRole('textbox').fill(text);
    await page.getByRole('button', { name: 'Проверить обезличивание', exact: true }).click();
    await page.getByRole('button', { name: 'Всё обезличено — сохранить', exact: true }).click();
  }
  try {
    await page.goto(`http://127.0.0.1:${runtime.port}/tg/`);
    await page.getByRole('button', { name: '+ Новое обращение', exact: true }).click();
    await enterText('Жалоба на сроки приёма. Пациентка Иванова Анна Петровна обратилась 01.10.2025.');
    await page.getByRole('button', { name: 'Добавить пояснения', exact: true }).waitFor();
    assert.equal(await page.locator('.dr-document-toggle').count(), 1);
    assert.equal(await page.locator('.dr-open-document').count(), 0);
    for (const width of [320, 390, 768]) {
      await page.setViewportSize({ width, height: 844 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(200);
    if (process.env.DOCTOR_WORK_SCREENSHOT) await page.screenshot({ path: process.env.DOCTOR_WORK_SCREENSHOT, fullPage: true });
    await page.getByRole('button', { name: 'Добавить пояснения', exact: true }).click();
    await enterText('Иванова осмотрена 06.10.2025, назначено лечение.');
    const prepare = page.getByRole('button', { name: 'Подготовить ответ', exact: true });
    await prepare.waitFor();
    assert.equal(writes.filter((op) => op === 'generate').length, 0);
    await prepare.click();
    await page.getByRole('button', { name: 'Проверить и завершить', exact: true }).waitFor();
    assert.equal(writes.filter((op) => op === 'generate').length, 1);
    assert.equal(await page.locator('.dr-open-document').count(), 0);
    assert.equal(await page.locator('.dr-progress button').last().locator('.dr-progress-done').count(), 0, 'An AI draft is not a final answer');
    await page.getByRole('button', { name: 'Проверить и завершить', exact: true }).click();
    await page.getByRole('textbox').fill('Ваше обращение рассмотрено. По результатам осмотра назначено лечение.');
    await page.getByRole('button', { name: 'Проверить обезличивание', exact: true }).click();
    await page.getByRole('button', { name: 'Всё обезличено — сохранить', exact: true }).click();
    await page.getByRole('button', { name: 'Завершить и сохранить в базу', exact: true }).click();
    await page.getByRole('button', { name: 'Подготовить DOCX', exact: true }).waitFor();
    assert.equal(writes.filter((op) => op === 'save').length, 1);
    assert.equal(await page.locator('.dr-document-toggle').count(), 4);
    assert.equal(await page.locator('.dr-open-document').count(), 0);
    await page.waitForTimeout(200);
    if (process.env.DOCTOR_DONE_SCREENSHOT) await page.screenshot({ path: process.env.DOCTOR_DONE_SCREENSHOT, fullPage: true });
    await page.getByRole('button', { name: 'Подготовить DOCX', exact: true }).click();
    const downloadButton = page.getByRole('button', { name: 'Скачать DOCX', exact: true });
    await downloadButton.waitFor();
    assert.equal(await page.locator('a[href^="blob:"]').count(), 0, 'Telegram gets an HTTPS URL, never a blob URL');
    const downloaded = page.waitForEvent('download');
    await downloadButton.click();
    assert.ok((await downloaded).suggestedFilename().endsWith('.docx'));
    await page.getByRole('button', { name: 'Отправить в чат', exact: true }).click();
    await page.getByText('Документ отправлен вам в чат с ботом.', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Пояснения врача', exact: true }).click();
    await page.getByRole('region', { name: 'Пояснения врача', exact: true }).waitFor();
    assert.equal(await page.locator('.dr-open-document').count(), 1);
    assert.equal(writes.filter((op) => op === 'generate').length, 1);
    await page.getByText('Другие действия', { exact: true }).click();
    await page.getByRole('button', { name: 'В архив', exact: true }).click();
    await page.getByRole('button', { name: 'Вернуть из архива', exact: true }).first().waitFor();
    await page.getByRole('button', { name: 'Вернуть из архива', exact: true }).first().click();
    await page.getByRole('button', { name: 'Подготовить DOCX', exact: true }).waitFor();
    assert.deepEqual(errors, []);
    console.log('Guided case: complaint → explanations → single AI call → review → save → DOCX/download/chat → open one document → archive/restore. No JS errors or mobile overflow.');
  } finally { await browser.close(); }
})().catch((e) => { console.error(e); process.exit(1); });
