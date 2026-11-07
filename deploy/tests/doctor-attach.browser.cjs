// Opt-in browser check against doctor_tests::browser_harness + synthetic bot worker.
// Requires Playwright in the test runtime, not as an application dependency.
const { chromium } = require('playwright');
const fs = require('node:fs');
const assert = require('node:assert/strict');
(async () => {
  const runtime = JSON.parse(fs.readFileSync(process.env.DOCTOR_UI_RUNTIME));
  const browser = await chromium.launch({ headless: true, ...(process.env.DOCTOR_CHROME ? { executablePath: process.env.DOCTOR_CHROME } : {}) });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  const errors = [], writes = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('request', (r) => { if (r.url().endsWith('/doctor/jobs') && r.method() === 'POST') {
    const data = r.postDataJSON(); if (['create', 'review', 'review_confirm'].includes(data.operation)) writes.push(data);
  } });
  await page.route('https://telegram.org/**', (route) => route.fulfill({ body: '' }));
  await page.addInitScript(({ data }) => { window.Telegram = { WebApp: {
    initData: data, initDataUnsafe: { user: { first_name: 'Врач' } }, themeParams: {},
    ready() {}, expand() {}, onEvent() {}, isVersionAtLeast() { return false; },
  } }; }, { data: runtime.init_data });
  async function pickServerFile() {
    await page.getByRole('button', { name: 'Остальные файлы', exact: true }).click();
    await page.getByRole('button', { name: 'Мои', exact: true }).click();
    await page.getByRole('button').filter({ hasText: 'жалоба.docx' }).first().click();
    await page.getByRole('dialog').getByRole('button', { name: 'Помощь врача', exact: true }).click();
    await page.getByRole('heading', { name: 'Добавить документ', exact: true }).waitFor();
  }
  try {
    await page.goto(`http://127.0.0.1:${runtime.port}/tg/`);
    await page.getByRole('heading', { name: 'Помощь врача', exact: true }).waitFor();
    await pickServerFile();
    await page.getByRole('button', { name: 'Начать разбор этой жалобы', exact: true }).waitFor();
    assert.equal(writes.length, 0, 'Selecting a file must not create a case');
    await page.getByText('Проверяем существующие обращения…', { exact: true }).waitFor({ state: 'hidden' });
    for (const width of [320, 390, 768]) {
      await page.setViewportSize({ width, height: 844 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.locator('.workspace-tabs').count(), 1, 'Workspace navigation must not be duplicated');
    await page.waitForTimeout(200);
    if (process.env.DOCTOR_SCREENSHOT) await page.screenshot({ path: process.env.DOCTOR_SCREENSHOT, fullPage: true });
    await page.getByRole('button', { name: 'Начать разбор этой жалобы', exact: true }).click();
    await page.getByRole('button', { name: 'Всё обезличено — сохранить' }).waitFor();
    assert.equal(writes.filter((r) => r.operation === 'create').length, 1);
    assert.equal(writes.filter((r) => r.operation === 'review').length, 1);
    await page.getByRole('button', { name: 'Всё обезличено — сохранить' }).click();
    await page.getByRole('button', { name: 'Добавить: пояснения врача' }).waitFor();
    await pickServerFile();
    await page.getByRole('combobox', { name: 'Что это за документ?' }).selectOption('explanation');
    const add = page.getByRole('button', { name: 'Добавить документ и проверить', exact: true });
    assert.equal(await add.isDisabled(), true, 'Must select an existing complaint before attaching explanations');
    const before = writes.length;
    await page.locator('label.dr-choice').filter({ hasText: 'Жалоба по срокам приёма' }).click();
    assert.equal(writes.length, before, 'Selecting a case must not upload the file');
    await add.click();
    await page.getByRole('button', { name: 'Всё обезличено — сохранить' }).waitFor();
    assert.equal(writes.filter((r) => r.operation === 'create').length, 1, 'Existing-case attachment must not create another case');
    assert.equal(writes.at(-1).payload.role, 'explanation');
    assert.deepEqual(errors, []);
    console.log('New complaint: one click to review. Existing case: explicit selection + attach. No duplicate case, no JS errors, no overflow at 320/390/768px.');
  } finally { await browser.close(); }
})().catch((e) => { console.error(e); process.exit(1); });
