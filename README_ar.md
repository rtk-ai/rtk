<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>وكيل CLI عالي الأداء يقلل استهلاك رموز LLM بنسبة 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/gFwRPEKq4p"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">الموقع الإلكتروني</a> &bull;
  <a href="#installation">التثبيت</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">استكشاف الأخطاء</a> &bull;
  <a href="ARCHITECTURE.md">البنية التقنية</a> &bull;
  <a href="https://discord.gg/gFwRPEKq4p">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Français</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Español</a> &bull;
  <a href="README_ar.md">العربية</a>
</p>

---

يقوم rtk بتصفية وضغط مخرجات الأوامر قبل أن تصل إلى سياق نموذج اللغة الخاص بك. ملف Rust ثنائي واحد، بدون أي تبعيات، مع زمن استجابة أقل من 10 ميلي ثانية.

## توفير الرموز (جلسة Claude Code لمدة 30 دقيقة)

| العملية | التكرار | المعيار | rtk | التوفير |
|---------|---------|---------|-----|---------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `git diff` | 5x | 10,000 | 2,500 | -75% |
| `git log` | 5x | 2,500 | 500 | -80% |
| `git add/commit/push` | 8x | 1,600 | 120 | -92% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **الإجمالي** | | **~118,000** | **~23,900** | **-80%** |

> تقديرات مبنية على مشاريع TypeScript/Rust متوسطة الحجم.

## التثبيت

### Homebrew (موصى به)

```bash
brew install rtk
```

### تثبيت سريع (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### التحقق من التثبيت

```bash
rtk --version   # يجب أن يعرض "rtk 0.27.x"
rtk gain        # يجب أن يعرض إحصائيات التوفير
```

> **تحذير**: يوجد مشروع آخر باسم "rtk" (Rust Type Kit) على crates.io. إذا فشل `rtk gain`، فأنت تستخدم الحزمة الخاطئة.

## البداية السريعة

```bash
# 1. تثبيت الخطاف لـ Claude Code (موصى به)
rtk init --global
# اتبع التعليمات للتسجيل في ~/.claude/settings.json

# 2. أعد تشغيل Claude Code، ثم اختبر
git status  # تُعاد كتابته تلقائياً إلى rtk git status
```

يُعيد الخطاف كتابة الأوامر بشكل شفاف (مثال: `git status` ← `rtk git status`) قبل التنفيذ.

## كيف يعمل

```
  بدون rtk:                                        مع rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |      ~2,000 رمز (خام)             |             |   ~200 رمز           | تصفية   |
    +-----------------------------------+             +------- (تصفية) ------+----------+
```

أربع استراتيجيات مطبقة حسب نوع الأمر:

1. **التصفية الذكية** - إزالة الضوضاء (التعليقات، المسافات، النصوص المكررة)
2. **التجميع** - تجميع العناصر المتشابهة (الملفات حسب المجلد، الأخطاء حسب النوع)
3. **الاقتطاع** - الاحتفاظ بالسياق ذي الصلة وحذف التكرار
4. **إزالة التكرار** - دمج سطور السجل المتكررة مع عدادات

## الأوامر

### الملفات
```bash
rtk ls .                        # شجرة مجلدات محسّنة
rtk read file.rs                # قراءة ذكية
rtk read file.rs -l aggressive  # التوقيعات فقط
rtk find "*.rs" .               # نتائج مضغوطة
rtk grep "pattern" .            # نتائج مجمّعة حسب الملف
rtk diff file1 file2            # فرق مضغوط
```

### Git
```bash
rtk git status                  # حالة مضغوطة
rtk git log -n 10               # commits في سطر واحد
rtk git diff                    # فرق مضغوط
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
```

### الاختبارات
```bash
rtk test cargo test             # الإخفاقات فقط (-90%)
rtk vitest run                  # Vitest مضغوط
rtk pytest                      # اختبارات Python (-90%)
rtk go test                     # اختبارات Go (-90%)
rtk cargo test                  # اختبارات Cargo (-90%)
```

### البناء والفحص
```bash
rtk lint                        # ESLint مجمّع حسب القاعدة
rtk tsc                         # أخطاء TypeScript مجمّعة
rtk cargo build                 # بناء Cargo (-80%)
rtk cargo clippy                # Clippy (-80%)
rtk ruff check                  # فحص Python (-80%)
```

### الحاويات
```bash
rtk docker ps                   # قائمة مضغوطة
rtk docker logs <container>     # سجلات بدون تكرار
rtk kubectl pods                # Pods مضغوطة
```

### التحليلات
```bash
rtk gain                        # إحصائيات التوفير
rtk gain --graph                # رسم بياني ASCII (30 يومًا)
rtk discover                    # اكتشاف فرص التوفير الفائتة
```

## الإعداد

```toml
# ~/.config/rtk/config.toml
[tracking]
database_path = "/مسار/مخصص.db"

[hooks]
exclude_commands = ["curl", "playwright"]

[tee]
enabled = true
mode = "failures"
```

## التوثيق

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - حل المشكلات الشائعة
- **[INSTALL.md](INSTALL.md)** - دليل التثبيت التفصيلي
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - البنية التقنية

## المساهمة

نرحب بالمساهمات! افتح issue أو PR على [GitHub](https://github.com/rtk-ai/rtk).

انضم إلى المجتمع على [Discord](https://discord.gg/pvHdzAec).

## الرخصة

رخصة MIT - راجع [LICENSE](LICENSE) للتفاصيل.