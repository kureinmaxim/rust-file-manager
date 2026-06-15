# Локальный запуск

Инструкция для запуска **rust-file-manager** на локальной машине под Windows или macOS.

---

## Предварительные требования

| | Windows | macOS |
|---|---|---|
| Rust | [rustup.rs](https://rustup.rs/) → установщик `.exe` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Git | [git-scm.com](https://git-scm.com/) | встроен или через Xcode CLI: `xcode-select --install` |

Проверка после установки:

```sh
rustc --version
cargo --version
```

---

## Windows

> Все команды выполняются в **PowerShell** (не CMD).

### 1. Сборка

```powershell
cargo build --release
```

Бинарник появится в `target\release\rust-file-manager.exe`.

### 2. Создать `.env`

```powershell
copy .env.example .env
```

### 3. Сгенерировать хэш пароля

```powershell
echo "ВАШ-ПАРОЛЬ" | .\target\release\rust-file-manager.exe hash-password
```

Программа выведет строку вида `$2b$12$...` и готовую строку для `.env`. Скопируй её.

### 4. Сгенерировать ключ сессий

```powershell
[System.Convert]::ToBase64String(
  [System.Security.Cryptography.RandomNumberGenerator]::GetBytes(64)
)
```

Скопируй выведенную строку.

### 5. Заполнить `.env`

Открой `.env` в любом редакторе и заполни:

```dotenv
BIND_ADDR=127.0.0.1:8080
UPLOAD_DIR=uploads
USERS_FILE=users.json
MAX_FILE_SIZE_MB=200
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH='$2b$12$сюда-результат-шага-3'
SESSION_SECRET=сюда-результат-шага-4
COOKIE_SECURE=false
RUST_LOG=info
```

> ⚠️ Хэш пароля — строго в **одинарных кавычках**. Без них символы `$` внутри хэша
> раскрываются как переменные, хэш портится и вход перестаёт работать.

### 6. Запуск

```powershell
.\target\release\rust-file-manager.exe
```

Открой в браузере: **http://127.0.0.1:8080**

---

## macOS

### 1. Сборка

```sh
cargo build --release
```

Бинарник появится в `target/release/rust-file-manager`.

### 2. Создать `.env`

```sh
cp .env.example .env
```

### 3. Сгенерировать хэш пароля

```sh
echo 'ВАШ-ПАРОЛЬ' | ./target/release/rust-file-manager hash-password
```

Программа выведет строку вида `$2b$12$...` и готовую строку для `.env`. Скопируй её.

### 4. Сгенерировать ключ сессий

```sh
openssl rand -base64 64 | tr -d '\n'
```

Если `openssl` не установлен, поставь через Homebrew: `brew install openssl`.

### 5. Заполнить `.env`

```dotenv
BIND_ADDR=127.0.0.1:8080
UPLOAD_DIR=uploads
USERS_FILE=users.json
MAX_FILE_SIZE_MB=200
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH='$2b$12$сюда-результат-шага-3'
SESSION_SECRET=сюда-результат-шага-4
COOKIE_SECURE=false
RUST_LOG=info
```

> ⚠️ Хэш пароля — строго в **одинарных кавычках** (см. выше).

### 6. Запуск

```sh
./target/release/rust-file-manager
```

Открой в браузере: **http://127.0.0.1:8080**

---

## Что делать дальше

- Войди с логином `admin` (или тем, что задал в `ADMIN_USERNAME`) и паролем из шага 3.
- Для деплоя на VPS — смотри [`DEPLOY.md`](DEPLOY.md).
- Для настройки Cloudflare Tunnel — [`GUIDE_cloudflare.md`](GUIDE_cloudflare.md).
