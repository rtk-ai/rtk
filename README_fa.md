<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>پراکسی خط فرمانِ پرسرعت که تا ۹۰٪ از خروجی bash را که عامل هوش مصنوعی شما می‌خواند حذف می‌کند</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">وب‌سایت</a> &bull;
  <a href="#نصب">نصب</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">رفع اشکال</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">معماری</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a> &bull;
  <a href="README_fa.md">فارسی</a>
</p>

---

RTK خروجی دستورات را **پیش از آنکه به بستر (context) مدل زبانی شما برسد** فیلتر و فشرده می‌کند. یک باینری واحد Rust، بیش از ۱۰۰ دستور پشتیبانی‌شده، و سربار کمتر از ۱۰ میلی‌ثانیه.

## RTK چه کاری انجام می‌دهد

RTK دستورات شل را میان‌گیری (intercept) می‌کند و خروجی آن‌ها را پیش از آنکه عامل هوش مصنوعی شما بخواند، فشرده می‌کند.

| عملیات | کاری که RTK با خروجی می‌کند |
|--------|-----------------------------|
| `ls` / `tree` | قالب درختی با شمارش فایل‌ها به‌جای یک خط برای هر مورد |
| `cat` / `read` | خواندن هوشمند فایل: امضاها و ساختار به‌جای بدنه‌ی کامل |
| `grep` / `rg` | کوتاه‌کردن خطوط بلند، گروه‌بندی تطبیق‌ها بر اساس فایل |
| `git status` | قالب فشرده و گروه‌بندی‌شده بر اساس وضعیت |
| `git diff` | بستر کاهش‌یافته، حذف سرآیندها |
| `git log` | فقط هش، نویسنده و موضوع کامیت |
| `git add/commit/push` | یک خط تأیید به‌جای خروجی کامل پیشرفت |
| `cargo test` / `npm test` | فقط خطاها؛ تست‌های موفق به یک شمارنده جمع می‌شوند |
| `ruff check` | گروه‌بندی بر اساس قاعده و فایل |
| `pytest` | فقط خطاها، traceback کوتاه‌شده |
| `go test` | تجزیه‌ی NDJSON، فقط خطاها |
| `docker ps` | فقط فیلدهای ضروری |

## صرفه‌جویی چگونه کار می‌کند

RTK **تا ۹۰٪ از خروجی bash** را که عامل شما می‌خواند حذف می‌کند. این همان چیزی است که RTK اندازه می‌گیرد — و با «۹۰٪ کاهش صورت‌حساب شما» یکی نیست.

خروجی bash **یکی از عوامل تشکیل‌دهنده‌ی توکن‌های ورودی** است، در کنار پرامپت شما، پرامپت سیستمی و تاریخچه‌ی گفت‌وگو. توکن‌های ورودی هم به‌نوبه‌ی خود **تنها بخشی از صورت‌حساب** هستند که توکن‌های خروجی را نیز در بر می‌گیرد. بنابراین این کاهش در هر مرحله رقیق‌تر می‌شود.

شمارش توکنی که RTK گزارش می‌دهد با فرمول `bytes / 4` تخمین زده می‌شود: RTK هیچ توکِنایزری در خود ندارد، پس **درصدها قابل‌اعتمادند اما اعداد مطلقِ توکن تقریبی‌اند**.

> توضیح کامل: [صرفه‌جویی در RTK چگونه کار می‌کند](docs/guide/resources/savings-explained.md)

## نصب

### Homebrew (پیشنهادی)

```bash
brew install rtk
```

### نصب سریع (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### راستی‌آزمایی نصب

```bash
rtk --version   # باید چیزی مانند "rtk 0.49.x" نشان دهد
rtk gain        # باید آمار صرفه‌جویی را نشان دهد
```

## شروع سریع

```bash
# ۱. نصب هوک برای ابزار هوش مصنوعی شما (پیش‌فرض: Claude Code / Copilot)
rtk init -g

# ۲. ابزار هوش مصنوعی را دوباره اجرا (restart) کنید، سپس تست کنید
git status  # به‌طور خودکار به rtk git status بازنویسی می‌شود
```

هوک فقط روی فراخوانی‌های ابزار Bash اثر می‌گذارد. ابزارهای داخلی Claude Code مانند `Read`، `Grep` و `Glob` از هوک عبور نمی‌کنند؛ برای فیلترشدن آن‌ها از دستورهای شل یا مستقیماً از `rtk read`، `rtk grep` و `rtk find` استفاده کنید.

## طرز کار

```
  بدون rtk:                                        با rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        خروجی خام کامل              |             |   خروجی فشرده         | فیلتر    |
    +-----------------------------------+             +------- (فیلترشده) ----+----------+
```

چهار راهبرد بسته به نوع دستور:

1. **فیلتر هوشمند** — حذف نویز (توضیحات، فاصله‌ها، boilerplate)
2. **گروه‌بندی** — تجمیع موارد مشابه (فایل‌ها بر اساس پوشه، خطاها بر اساس نوع)
3. **کوتاه‌سازی** — نگه‌داشتن بستر مرتبط، حذف افزونگی
4. **حذف تکرار** — جمع‌کردن خطوط تکراری لاگ همراه با شمارنده

> **آیا RTK کش پرامپت Claude را می‌شکند؟** خیر. RTK خروجی را یک‌بار برای هر دستور فیلتر می‌کند؛ نتیجه در تاریخچه ذخیره و در فراخوانی‌های بعدی به‌طور عادی کش می‌شود.

## دستورها

> درصدهای زیر **کاهش در خروجی bash** هستند، نه کاهش در صورت‌حساب شما. به [صرفه‌جویی چگونه کار می‌کند](#صرفهجویی-چگونه-کار-میکند) مراجعه کنید.

### فایل‌ها
```bash
rtk ls .                        # درخت پوشه‌ی فشرده
rtk read file.rs                # خواندن هوشمند فایل
rtk read file.rs -l aggressive  # فقط امضاها (حذف بدنه‌ها)
rtk find "*.rs" .               # نتایج فشرده‌ی find
rtk grep "pattern" .            # نتایج جست‌وجوی گروه‌بندی‌شده
```

### Git
```bash
rtk git status                  # وضعیت فشرده
rtk git log -n 10               # کامیت‌ها در یک خط
rtk git diff                    # diff فشرده
rtk git push                    # ‏-> "ok main"
```

### تست‌ها
```bash
rtk pytest                      # تست‌های پایتون (−۹۰٪)
rtk go test                     # تست‌های Go (−۹۰٪)
rtk cargo test                  # تست‌های Rust (−۹۰٪)
rtk test <cmd>                  # فقط خطاها (−۹۰٪)
```

### ساخت و لینت
```bash
rtk lint                        # ESLint گروه‌بندی‌شده بر اساس قاعده
rtk tsc                         # خطاهای TypeScript گروه‌بندی‌شده
rtk cargo build                 # ساخت Cargo (−۸۰٪)
rtk ruff check                  # لینت پایتون (−۸۰٪)
```

### تحلیل صرفه‌جویی
```bash
rtk gain                        # آمار صرفه‌جویی
rtk gain --graph                # نمودار ASCII (۳۰ روز)
rtk discover                    # کشف صرفه‌جویی‌های ازدست‌رفته
```

## ابزارهای هوش مصنوعی پشتیبانی‌شده

RTK از ۱۷ ابزار کدنویسی هوش مصنوعی پشتیبانی می‌کند — از جمله Claude Code، GitHub Copilot، Cursor، Gemini CLI، Codex، Windsurf، Cline/Roo Code و موارد دیگر. هر یکپارچه‌سازی، دستورهای شل را در جایی که آن ابزار از میان‌گیری دستور پشتیبانی می‌کند به معادل `rtk` بازنویسی می‌کند. برای جزئیات هر ابزار، به [راهنمای ابزارهای پشتیبانی‌شده](https://www.rtk-ai.app/guide/getting-started/supported-agents) مراجعه کنید.

## پیکربندی

`~/.config/rtk/config.toml` (در macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # این دستورها بازنویسی نشوند

[retriever]
mode = "sqlite"         # sqlite (پیش‌فرض) | tee (فایل‌های قدیمی) | disabled
```

وقتی دستوری با خطا مواجه شود، RTK خروجی کاملِ فیلترنشده را ذخیره می‌کند تا مدل زبانی بتواند بدون اجرای دوباره آن را بازیابی کند (`rtk recall <id>`).

## حریم خصوصی و تله‌متری

تله‌متری **به‌طور پیش‌فرض غیرفعال** است و تنها با **رضایت صریح** (opt-in) فعال می‌شود. هرگز کد منبع، مسیر فایل، آرگومان دستورها، اسرار یا محتوای مخزن جمع‌آوری نمی‌شود — تنها شمارش‌های تجمیعی و ناشناس. مدیریت آن با `rtk telemetry status | enable | disable | forget` انجام می‌شود، و متغیر محیطی `RTK_TELEMETRY_DISABLED=1` آن را صرف‌نظر از رضایت مسدود می‌کند.

## مستندات

- **[INSTALL.md](INSTALL.md)** — راهنمای نصب کامل
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — معماری فنی
- **[رفع اشکال](https://www.rtk-ai.app/guide/troubleshooting)** — حل مشکلات رایج

## مشارکت

مشارکت‌ها با آغوش باز پذیرفته می‌شوند. یک issue یا PR در [GitHub](https://github.com/rtk-ai/rtk) باز کنید.

به جامعه‌ی کاربران در [Discord](https://discord.gg/RySmvNF5kF) بپیوندید.

## مجوز

مجوز Apache 2.0 — برای جزئیات به [LICENSE](LICENSE) مراجعه کنید.

## سلب مسئولیت

به [DISCLAIMER.md](DISCLAIMER.md) مراجعه کنید.
