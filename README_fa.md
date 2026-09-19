<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>یک پراکسی سریع برای خط فرمان که تا ۹۰٪ از خروجی bashِ خوانده‌شده توسط دستیار هوش مصنوعی‌تان را کم می‌کند</strong>
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

RTK خروجی دستورها را قبل از اینکه به کانتکستِ مدل هوش مصنوعی برسد، فیلتر و فشرده می‌کند. یک فایل اجرایی Rust، بیش از ۱۰۰ دستور پشتیبانی‌شده، و کمتر از ۱۰ میلی‌ثانیه سربار.

## RTK چه کار می‌کند؟

RTK دستورهای شل را وسط راه می‌گیرد و خروجی‌شان را فشرده می‌کند تا حجم کمتری به دستیار هوش مصنوعی‌تان برسد.

| دستور | کاری که RTK با خروجی می‌کند |
|--------|-----------------------------|
| `ls` / `tree` | نمایش درختی همراه با تعداد فایل‌ها، به‌جای یک خط برای هر مورد |
| `cat` / `read` | خواندن هوشمند فایل: نمایش امضاها و ساختار به‌جای کل بدنه |
| `grep` / `rg` | کوتاه کردن خط‌های بلند و گروه‌بندی نتایج بر اساس فایل |
| `git status` | نمایش فشرده و دسته‌بندی‌شده بر اساس وضعیت |
| `git diff` | کانتکست کمتر و حذف سرآیندها |
| `git log` | فقط هش، نویسنده و عنوان کامیت |
| `git add/commit/push` | فقط یک خط تأیید، به‌جای کل خروجی پیشرفت |
| `cargo test` / `npm test` | فقط تست‌های ناموفق؛ تست‌های موفق در یک عدد جمع می‌شوند |
| `ruff check` | گروه‌بندی بر اساس قاعده و فایل |
| `pytest` | فقط خطاها، با traceback کوتاه‌شده |
| `go test` | خواندن NDJSON، فقط خطاها |
| `docker ps` | فقط فیلدهای مهم |

## صرفه‌جویی چطور اتفاق می‌افتد؟

RTK تا ۹۰٪ از **خروجی bash** را که دستیار شما می‌خواند کم می‌کند. این همان چیزی است که RTK اندازه می‌گیرد — و با «۹۰٪ کم شدن صورت‌حساب» فرق دارد.

خروجی bash فقط **یکی از چیزهایی است که توکن ورودی را زیاد می‌کند**؛ در کنار آن، پرامپت خودتان، پرامپت سیستمی و تاریخچه‌ی گفت‌وگو هم هستند. توکن ورودی هم **فقط بخشی از صورت‌حساب** است و توکن خروجی هم حساب می‌شود. برای همین این کاهش در هر مرحله کم‌رنگ‌تر می‌شود.

عددهای توکنی که RTK نشان می‌دهد با فرمول ساده‌ی `bytes / 4` تخمین زده می‌شوند؛ RTK توکنایزر واقعی ندارد، پس **درصدها قابل‌اعتمادند ولی عددهای دقیق توکن تقریبی‌اند**.

> توضیح کامل: [صرفه‌جویی در RTK چطور کار می‌کند](docs/guide/resources/savings-explained.md)

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

### بررسی نصب

```bash
rtk --version   # باید چیزی مثل "rtk 0.49.x" نشان بدهد
rtk gain        # باید آمار صرفه‌جویی را نشان بدهد
```

## شروع سریع

```bash
# ۱. نصب برای ابزار هوش مصنوعی‌تان (پیش‌فرض: Claude Code / Copilot)
rtk init -g

# ۲. ابزار را یک بار ببندید و باز کنید، بعد امتحان کنید
git status  # خودکار به rtk git status تبدیل می‌شود
```

هوک فقط روی دستورهای ابزار Bash کار می‌کند. ابزارهای داخلی Claude Code مثل `Read`، `Grep` و `Glob` از هوک رد نمی‌شوند؛ برای اینکه خروجی آن‌ها هم فشرده شود، از دستورهای شل یا مستقیماً از `rtk read`، `rtk grep` و `rtk find` استفاده کنید.

## طرز کار

```
  بدون rtk:                                        با rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        خروجی خام و کامل            |             |   خروجی فشرده         | فیلتر    |
    +-----------------------------------+             +------- (فیلترشده) ----+----------+
```

چهار کاری که RTK بسته به نوع دستور انجام می‌دهد:

1. **فیلتر هوشمند** — حذف چیزهای اضافه (توضیحات، فاصله‌های خالی، متن تکراری قالبی)
2. **گروه‌بندی** — کنار هم گذاشتن موارد مشابه (فایل‌ها بر اساس پوشه، خطاها بر اساس نوع)
3. **کوتاه‌سازی** — نگه داشتن بخش‌های مهم و حذف حرف‌های تکراری
4. **حذف تکرار** — جمع کردن خط‌های تکراری لاگ در یک خط همراه با تعداد

> **آیا RTK کش پرامپت Claude را خراب می‌کند؟** نه. RTK خروجی هر دستور را فقط یک بار فیلتر می‌کند؛ نتیجه در تاریخچه ذخیره می‌شود و دفعه‌های بعد مثل قبل کش می‌شود.

## دستورها

> درصدهای زیر یعنی **چقدر از خروجی bash کم می‌شود**، نه چقدر از صورت‌حساب شما. رجوع کنید به [صرفه‌جویی چطور اتفاق می‌افتد](#صرفهجویی-چطور-اتفاق-میافتد).

### فایل‌ها
```bash
rtk ls .                        # نمایش درختی فشرده‌ی پوشه
rtk read file.rs                # خواندن هوشمند فایل
rtk read file.rs -l aggressive  # فقط امضاها (بدون بدنه)
rtk find "*.rs" .               # نتیجه‌ی فشرده‌ی find
rtk grep "pattern" .            # نتیجه‌ی جست‌وجو، گروه‌بندی‌شده
```

### Git
```bash
rtk git status                  # وضعیت فشرده
rtk git log -n 10               # کامیت‌ها، هرکدام در یک خط
rtk git diff                    # diff فشرده
rtk git push                    # ‏-> "ok main"
```

### تست‌ها
```bash
rtk pytest                      # تست‌های پایتون (−۹۰٪)
rtk go test                     # تست‌های Go (−۹۰٪)
rtk cargo test                  # تست‌های Rust (−۹۰٪)
rtk test <cmd>                  # فقط تست‌های ناموفق (−۹۰٪)
```

### ساخت و لینت
```bash
rtk lint                        # ESLint، گروه‌بندی‌شده بر اساس قاعده
rtk tsc                         # خطاهای TypeScript، گروه‌بندی‌شده
rtk cargo build                 # ساخت با Cargo (−۸۰٪)
rtk ruff check                  # لینت پایتون (−۸۰٪)
```

### آمار صرفه‌جویی
```bash
rtk gain                        # آمار صرفه‌جویی
rtk gain --graph                # نمودار متنی (۳۰ روز اخیر)
rtk discover                    # پیدا کردن صرفه‌جویی‌هایی که از دست رفته
```

## ابزارهای هوش مصنوعی پشتیبانی‌شده

RTK از ۱۷ ابزار کدنویسی هوش مصنوعی پشتیبانی می‌کند — از جمله Claude Code، GitHub Copilot، Cursor، Gemini CLI، Codex، Windsurf، Cline/Roo Code و بقیه. هر کدام دستورهای شل را — هرجا که آن ابزار اجازه بدهد — به معادل `rtk` تبدیل می‌کنند. برای جزئیات هر ابزار، [راهنمای ابزارهای پشتیبانی‌شده](https://www.rtk-ai.app/guide/getting-started/supported-agents) را ببینید.

## تنظیمات

`~/.config/rtk/config.toml` (در macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # این دستورها تبدیل نشوند

[retriever]
mode = "sqlite"         # sqlite (پیش‌فرض) | tee (فایل‌های قدیمی) | disabled
```

وقتی دستوری با خطا شکست می‌خورد، RTK کل خروجی فیلترنشده را نگه می‌دارد تا مدل بتواند بدون اجرای دوباره آن را ببیند (`rtk recall <id>`).

## حریم خصوصی و تله‌متری

تله‌متری **به‌طور پیش‌فرض خاموش** است و فقط با اجازه‌ی صریح خودتان (opt-in) روشن می‌شود. هیچ‌وقت کد، مسیر فایل، آرگومان دستورها، رمزها یا محتوای مخزن جمع نمی‌شود — فقط شمارش‌های کلی و بدون نام. برای مدیریتش از `rtk telemetry status | enable | disable | forget` استفاده کنید، و متغیر محیطی `RTK_TELEMETRY_DISABLED=1` هم آن را در هر حالت خاموش می‌کند.

## مستندات

- **[INSTALL.md](INSTALL.md)** — راهنمای کامل نصب
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — معماری فنی
- **[رفع اشکال](https://www.rtk-ai.app/guide/troubleshooting)** — حل مشکلات رایج

## مشارکت

از مشارکت شما استقبال می‌کنیم. کافی است یک issue یا PR در [GitHub](https://github.com/rtk-ai/rtk) باز کنید.

به جمع کاربران در [Discord](https://discord.gg/RySmvNF5kF) بپیوندید.

## مجوز

مجوز Apache 2.0 — برای جزئیات [LICENSE](LICENSE) را ببینید.

## سلب مسئولیت

[DISCLAIMER.md](DISCLAIMER.md) را ببینید.
