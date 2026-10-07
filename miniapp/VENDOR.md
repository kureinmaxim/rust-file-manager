# Вендорные библиотеки мини-приложения

Сборки фронтенда нет: модули отдаются как есть и вшиваются в бинарник
(`build.rs`). Внешний скрипт один — `telegram-web-app.js` с telegram.org
(так требует документация Telegram).

| Файл | Источник | Лицензия | sha256 |
|---|---|---|---|
| `assets/vendor/preact-htm.module.js` | npm `htm@3.1.1`, файл `preact/standalone.module.js` (htm + Preact 10 + hooks одним ES-модулем) | Apache-2.0 (htm), MIT (Preact) — `LICENSE-htm.txt`, `LICENSE-preact.txt` | `72284e8e9079c87817145df1110f74e8a2aa040b2fc384922e18dfcb46fc1fd7` |

Целостность архива проверена по реестру npm:
`sha512-983Vyg8NwUE7JkZ6NmOqpCZ+sh1bKv2iYTlUkzlWmA5JD2acKoxd4KVxbMmxX/85mtfdnDmTFoNKcg5DGAvxNQ==`.

Обновление: скачать новый архив `htm`, сверить `dist.integrity` с реестром,
заменить файл, обновить версию и sha256 в этой таблице.
