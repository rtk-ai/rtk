<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Высокопроизводительный CLI-прокси, сокращающий потребление токенов LLM на 60–90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Сайт</a> &bull;
  <a href="#установка">Установка</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Траблшутинг</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Архитектура</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Дискорд</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_ru.md">Русский</a>
</p>

---

rtk фильтрует и сжимает вывод команд до того, как они попадут в контекст вашей LLM. Один бинарник на Rust, ноль зависимостей, оверхед менее 10 мс.

## Экономия токенов (30-минутная сессия Claude Code)

| Операция | Частота | Стандартно | rtk | Экономия |
|----------|---------|------------|-----|----------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **Итого** | | **~118,000** | **~23,900** | **-80%** |

## Установка

### Homebrew (рекомендуется)

```bash
brew install rtk
```

### Быстрая установка (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Проверка

```bash
rtk --version   # Должно показать "rtk 0.27.x"
rtk gain        # Должно показать статистику экономии
```

## Быстрый старт

```bash
# 1. Установка хука для Claude Code (рекомендуется)
rtk init --global

# 2. Перезапуск Claude Code и проверка
git status  # автоматически переписывается в rtk git status
```

## Как это работает

```
  Без rtk:                                         С rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |       ~2,000 токенов (сырой)      |             |   ~200 токенов       | фильтр   |
    +-----------------------------------+             +-------(фильтр.)------+----------+
```

Четыре стратегии:

1. **Умная фильтрация** — убирает шум (комментарии, пробелы, шаблонный код)
2. **Группировка** — объединяет похожие элементы (файлы по каталогам, ошибки по типу)
3. **Обрезание** — сохраняет важный контекст, удаляет избыточность
4. **Дедупликация** — сворачивает повторяющиеся строки логов со счётчиками

## Команды

### Файлы
```bash
rtk ls .                        # оптимизированное дерево каталогов
rtk read file.rs                # умное чтение файлов
rtk find "*.rs" .               # компактные результаты поиска
rtk grep "pattern" .            # поиск, сгруппированный по файлам
```

### Git
```bash
rtk git status                  # компактный статус
rtk git log -n 10               # коммиты в одну строку
rtk git diff                    # сжатый diff
rtk git push                    # -> "ok main"
```

### Тесты
```bash
rtk jest                        # компактный Jest
rtk vitest                      # компактный Vitest
rtk pytest                      # Python-тесты (-90%)
rtk go test                     # Go-тесты (-90%)
rtk cargo test                  # Rust-тесты (-90%)
rtk test <cmd>                  # только падения (-90%)
```

### Сборка и линт
```bash
rtk lint                        # ESLint, сгруппированный по правилам
rtk tsc                         # ошибки TypeScript по группам
rtk cargo build                 # сборка Cargo (-80%)
rtk ruff check                  # линт Python (-80%)
```

### Аналитика
```bash
rtk gain                        # статистика экономии
rtk gain --graph                # ASCII-график (30 дней)
rtk discover                    # экономия токенов
```

## Документация

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** — типичные проблемы и их решения
- **[INSTALL.md](INSTALL.md)** — подробная инструкция по установке
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — техническая архитектура

## Вклад в проект

Pull request'ы и issues приветствуются. Открывайте их на [GitHub](https://github.com/rtk-ai/rtk).

Присоединяйтесь к сообществу в [Discord](https://discord.gg/RySmvNF5kF).

## Лицензия

MIT — см. [LICENSE](LICENSE).

## Отказ от ответственности

См. [DISCLAIMER.md](DISCLAIMER.md).
