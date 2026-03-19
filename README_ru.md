<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Высокопроизводительный CLI-прокси, сокращающий потребление токенов LLM на 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/pvHdzAec"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Сайт</a> &bull;
  <a href="#установка">Установка</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Устранение неполадок</a> &bull;
  <a href="ARCHITECTURE.md">Архитектура</a> &bull;
  <a href="https://discord.gg/pvHdzAec">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README.ru.md">Русский</a>
</p>

---

rtk фильтрует и сжимает вывод команд перед тем, как он попадёт в контекст вашего LLM. Единственный бинарный файл на Rust, без зависимостей, накладные расходы менее 10 мс.

## Экономия токенов (30-минутная сессия Claude Code)

| Операция | Частота | Стандартно | rtk | Экономия |
|----------|---------|------------|-----|----------|
| `ls` / `tree` | 10x | 2 000 | 400 | -80% |
| `cat` / `read` | 20x | 40 000 | 12 000 | -70% |
| `grep` / `rg` | 8x | 16 000 | 3 200 | -80% |
| `git status` | 10x | 3 000 | 600 | -80% |
| `git diff` | 5x | 10 000 | 2 500 | -75% |
| `git log` | 5x | 2 500 | 500 | -80% |
| `git add/commit/push` | 8x | 1 600 | 120 | -92% |
| `cargo test` / `npm test` | 5x | 25 000 | 2 500 | -90% |
| `ruff check` | 3x | 3 000 | 600 | -80% |
| `pytest` | 4x | 8 000 | 800 | -90% |
| `go test` | 3x | 6 000 | 600 | -90% |
| `docker ps` | 3x | 900 | 180 | -80% |
| **Итого** | | **~118 000** | **~23 900** | **-80%** |

> Оценки основаны на проектах среднего размера на TypeScript/Rust. Реальная экономия зависит от размера проекта.

## Установка

### Homebrew (рекомендуется)

```bash
brew install rtk
```

### Быстрая установка (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> Устанавливается в `~/.local/bin`. При необходимости добавьте в PATH:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # или ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Готовые бинарные файлы

Скачайте из [releases](https://github.com/rtk-ai/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

### Проверка установки

```bash
rtk --version   # Должно показать "rtk 0.28.2"
rtk gain        # Должно показать статистику экономии токенов
```

> **Предупреждение о конфликте имён**: На crates.io существует другой проект с именем "rtk" (Rust Type Kit). Если `rtk gain` не работает, у вас установлен не тот пакет. Используйте `cargo install --git` из инструкции выше.

## Быстрый старт

```bash
# 1. Установите хук для Claude Code (рекомендуется)
rtk init --global
# Следуйте инструкциям для регистрации в ~/.claude/settings.json
# По умолчанию только для Claude Code (используйте --opencode для OpenCode)

# 2. Перезапустите Claude Code и проверьте
git status  # Автоматически перезаписывается в rtk git status
```

Хук прозрачно перезаписывает команды Bash (например, `git status` -> `rtk git status`) перед выполнением. Claude никогда не видит перезапись — он просто получает сжатый вывод.

**Важно:** хук работает только для вызовов инструмента Bash. Встроенные инструменты Claude Code — `Read`, `Grep` и `Glob` — не проходят через хук Bash и поэтому не перезаписываются автоматически. Чтобы получить компактный вывод RTK для этих сценариев, используйте команды оболочки (`cat`/`head`/`tail`, `rg`/`grep`, `find`) или вызывайте `rtk read`, `rtk grep` или `rtk find` напрямую.

## Как это работает

```
  Без rtk:                                    С rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2 000 токенов (сырой)     |            |   ~200 токенов       | фильтр   |
    +-----------------------------------+            +------- (отфильтр.) ---+----------+
```

Для каждого типа команд применяются четыре стратегии:

1. **Умная фильтрация** — удаляет шум (комментарии, пробелы, шаблонный код)
2. **Группировка** — агрегирует похожие элементы (файлы по директориям, ошибки по типу)
3. **Усечение** — сохраняет релевантный контекст, убирает избыточность
4. **Дедупликация** — сворачивает повторяющиеся строки лога с подсчётом

## Команды

### Файлы
```bash
rtk ls .                        # Оптимизированное дерево каталогов
rtk read file.rs                # Умное чтение файла
rtk read file.rs -l aggressive  # Только сигнатуры (без тел функций)
rtk smart file.rs               # Эвристическое резюме кода в 2 строки
rtk find "*.rs" .               # Компактные результаты поиска
rtk grep "pattern" .            # Сгруппированные результаты поиска
rtk diff file1 file2            # Сжатый diff
```

### Git
```bash
rtk git status                  # Компактный статус
rtk git log -n 10               # Коммиты в одну строку
rtk git diff                    # Сжатый diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # Компактный список PR
rtk gh pr view 42               # Детали PR + проверки
rtk gh issue list               # Компактный список задач
rtk gh run list                 # Статус запусков workflow
```

### Тест-раннеры
```bash
rtk test cargo test             # Только сбои (-90%)
rtk err npm run build           # Только ошибки/предупреждения
rtk vitest run                  # Vitest компактно (только сбои)
rtk playwright test             # Результаты E2E (только сбои)
rtk pytest                      # Тесты Python (-90%)
rtk go test                     # Тесты Go (NDJSON, -90%)
rtk cargo test                  # Тесты Cargo (-90%)
```

### Сборка и линтинг
```bash
rtk lint                        # ESLint сгруппированный по правилам/файлам
rtk lint biome                  # Поддерживает другие линтеры
rtk tsc                         # Ошибки TypeScript сгруппированные по файлу
rtk next build                  # Компактная сборка Next.js
rtk prettier --check .          # Файлы, требующие форматирования
rtk cargo build                 # Сборка Cargo (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Линтинг Python (JSON, -80%)
rtk golangci-lint run           # Линтинг Go (JSON, -85%)
```

### Менеджеры пакетов
```bash
rtk pnpm list                   # Компактное дерево зависимостей
rtk pip list                    # Пакеты Python (авто-определение uv)
rtk pip outdated                # Устаревшие пакеты
rtk prisma generate             # Генерация схемы (без ASCII-арта)
```

### Контейнеры
```bash
rtk docker ps                   # Компактный список контейнеров
rtk docker images               # Компактный список образов
rtk docker logs <container>     # Дедуплицированные логи
rtk docker compose ps           # Сервисы Compose
rtk kubectl pods                # Компактный список подов
rtk kubectl logs <pod>          # Дедуплицированные логи
rtk kubectl services            # Компактный список сервисов
```

### Данные и аналитика
```bash
rtk json config.json            # Структура без значений
rtk deps                        # Сводка зависимостей
rtk env -f AWS                  # Отфильтрованные переменные окружения
rtk log app.log                 # Дедуплицированные логи
rtk curl <url>                  # Авто-определение JSON + схема
rtk wget <url>                  # Скачивание, без прогресс-баров
rtk summary <long command>      # Эвристическое резюме
rtk proxy <command>             # Сырой проброс + трекинг
```

### Аналитика экономии токенов
```bash
rtk gain                        # Сводная статистика
rtk gain --graph                # ASCII-график (последние 30 дней)
rtk gain --history              # История недавних команд
rtk gain --daily                # Разбивка по дням
rtk gain --all --format json    # JSON-экспорт для дашбордов

rtk discover                    # Найти упущенные возможности экономии
rtk discover --all --since 7    # Все проекты за последние 7 дней

rtk session                     # Показать использование RTK в последних сессиях
```

## Глобальные флаги

```bash
-u, --ultra-compact    # ASCII-иконки, инлайн-формат (дополнительная экономия токенов)
-v, --verbose          # Увеличить детализацию (-v, -vv, -vvv)
```

## Примеры

**Список файлов:**
```
# ls -la (45 строк, ~800 токенов)          # rtk ls (12 строк, ~150 токенов)
drwxr-xr-x  15 user staff 480 ...          my-project/
-rw-r--r--   1 user staff 1234 ...         +-- src/ (8 files)
...                                          |   +-- main.rs
                                             +-- Cargo.toml
```

**Git-операции:**
```
# git push (15 строк, ~200 токенов)        # rtk git push (1 строка, ~10 токенов)
Enumerating objects: 5, done.               ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**Вывод тестов:**
```
# cargo test (200+ строк при сбое)         # rtk test cargo test (~20 строк)
running 15 tests                             FAILED: 2/15 tests
test utils::test_parse ... ok                  test_edge_case: assertion failed
test utils::test_format ... ok                 test_overflow: panic at utils.rs:18
...
```

## Хук автоматической перезаписи

Наиболее эффективный способ использования rtk. Хук прозрачно перехватывает команды Bash и перезаписывает их в эквиваленты rtk перед выполнением.

**Результат**: 100% охват rtk во всех разговорах и подагентах, нулевые накладные расходы по токенам.

**Примечание об области действия:** это применяется только к вызовам инструмента Bash. Встроенные инструменты Claude Code — `Read`, `Grep` и `Glob` — обходят хук, поэтому используйте команды оболочки или явные команды `rtk`, если нужна фильтрация RTK.

### Настройка

```bash
rtk init -g                 # Установить хук + RTK.md (рекомендуется)
rtk init -g --opencode      # Плагин OpenCode (вместо Claude Code)
rtk init -g --auto-patch    # Неинтерактивный режим (CI/CD)
rtk init -g --hook-only     # Только хук, без RTK.md
rtk init --show             # Проверить установку
```

После установки **перезапустите Claude Code**.

## Плагин OpenCode (глобальный)

OpenCode поддерживает плагины, которые могут перехватывать выполнение инструментов. RTK предоставляет глобальный плагин, который повторяет поведение автоперезаписи Claude Code, переписывая команды инструмента Bash в `rtk ...` перед их выполнением. Этот плагин **не** устанавливается по умолчанию.

> **Примечание**: Этот плагин использует хук `tool.execute.before` OpenCode. Известное ограничение: хуки плагинов не перехватывают вызовы инструментов подагентов ([upstream issue](https://github.com/sst/opencode/issues/5894)). Подробнее см. в [документации плагинов OpenCode](https://open-code.ai/en/docs/plugins).

**Установка плагина OpenCode:**
```bash
rtk init -g --opencode
```

**Что создаётся:**
- `~/.config/opencode/plugins/rtk.ts`

**Требуется перезапуск**: Перезапустите OpenCode, затем проверьте командой `git status` в сессии.

**Ручная установка (резервный вариант):**
```bash
mkdir -p ~/.config/opencode/plugins
cp hooks/opencode-rtk.ts ~/.config/opencode/plugins/rtk.ts
```

### Перезаписываемые команды

| Исходная команда | Перезаписывается в |
|------------------|-------------------|
| `git status/diff/log/add/commit/push/pull` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `cat/head/tail <file>` | `rtk read <file>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `vitest/jest` | `rtk vitest run` |
| `tsc` | `rtk tsc` |
| `eslint/biome` | `rtk lint` |
| `prettier` | `rtk prettier` |
| `playwright` | `rtk playwright` |
| `prisma` | `rtk prisma` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `pip list/install` | `rtk pip ...` |
| `go test/build/vet` | `rtk go ...` |
| `golangci-lint` | `rtk golangci-lint` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |
| `pnpm list/outdated` | `rtk pnpm ...` |

Команды, уже использующие `rtk`, heredoc-ы (`<<`) и нераспознанные команды передаются без изменений.

## Конфигурация

### Файл конфигурации

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[tracking]
database_path = "/path/to/custom.db"  # по умолчанию: ~/.local/share/rtk/history.db

[hooks]
exclude_commands = ["curl", "playwright"]  # пропустить перезапись для этих команд

[tee]
enabled = true          # сохранять сырой вывод при сбое (по умолчанию: true)
mode = "failures"       # "failures", "always" или "never"
max_files = 20          # лимит ротации
```

### Tee: восстановление полного вывода

При сбое команды RTK сохраняет полный нефильтрованный вывод, чтобы LLM мог прочитать его без повторного запуска:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

### Удаление

```bash
rtk init -g --uninstall     # Удалить хук, RTK.md, запись settings.json
cargo uninstall rtk          # Удалить бинарный файл
brew uninstall rtk           # Если установлен через Homebrew
```

## Документация

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** — Решение типичных проблем
- **[INSTALL.md](INSTALL.md)** — Подробное руководство по установке
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — Техническая архитектура
- **[SECURITY.md](SECURITY.md)** — Политика безопасности и процесс ревью PR
- **[AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md)** — Руководство по аналитике экономии токенов

## Участие в разработке

Мы приветствуем любой вклад! Пожалуйста, откройте issue или PR на [GitHub](https://github.com/rtk-ai/rtk).

Присоединяйтесь к сообществу в [Discord](https://discord.gg/pvHdzAec).

## Лицензия

Лицензия MIT — подробности в файле [LICENSE](LICENSE).
