<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>LLM token tüketimini %60-90 oranında azaltan yüksek performanslı CLI vekili (proxy)</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Web Sitesi</a> &bull;
  <a href="#kurulum">Kurulum</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">Sorun Giderme</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Mimari</a> &bull;
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
  <a href="README_tr.md">Türkçe</a>
</p>

---

rtk, komut çıktılarını LLM bağlamınıza (context) ulaşmadan önce filtreler ve sıkıştırır. Tek bir Rust ikilisi (binary), 100'den fazla desteklenen komut, <10ms ek yük (overhead).

## Token Tasarrufu (30 Dakikalık Claude Code Oturumu)

| İşlem | Sıklık | Standart | rtk | Tasarruf |
|-----------|-----------|----------|-----|---------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `git diff` | 5x | 10,000 | 2,500 | -75% |
| `git log` | 5x | 2,500 | 500 | -80% |
| `git add/commit/push` | 8x | 1,600 | 120 | -92% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| `ruff check` | 3x | 3,000 | 600 | -80% |
| `pytest` | 4x | 8,000 | 800 | -90% |
| `go test` | 3x | 6,000 | 600 | -90% |
| `docker ps` | 3x | 900 | 180 | -80% |
| **Toplam** | | **~118,000** | **~23,900** | **-80%** |

> Tahminler orta ölçekli TypeScript/Rust projelerine dayanmaktadır. Gerçek tasarruf proje boyutuna göre değişiklik gösterebilir.

## Kurulum

### Homebrew (önerilen)

```bash
brew install rtk
```

### Hızlı Kurulum (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> `~/.local/bin` dizinine kurulur. Gerekirse PATH'e ekleyin:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # veya ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Hazır İkililer (Pre-built Binaries)

[Sürümlerden (releases)](https://github.com/rtk-ai/rtk/releases) indirin:
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows kullanıcıları**: Zip dosyasını çıkartın ve `rtk.exe` dosyasını PATH'inizde bulunan bir yere yerleştirin (örneğin `C:\Users\<kullanıcı>\.local\bin`). RTK'yı **Komut İstemi**, **PowerShell** veya **Windows Terminal** üzerinden çalıştırın — `.exe` dosyasına çift tıklamayın (hızla açılıp kapanacaktır). En iyi deneyim için, tam kanca (hook) sisteminin yerel olarak çalıştığı [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) kullanın. Detaylar için aşağıdaki [Windows kurulumu](#windows) bölümüne bakın.

### Kurulumu Doğrulama

```bash
rtk --version   # "rtk 0.28.2" göstermelidir
rtk gain        # Token tasarruf istatistiklerini göstermelidir
```

> **İsim çakışması uyarısı**: crates.io üzerinde "rtk" (Rust Type Kit) adında başka bir proje bulunmaktadır. Eğer `rtk gain` başarısız olursa, yanlış paketi yüklemiş olabilirsiniz. Bunun yerine yukarıdaki `cargo install --git` komutunu kullanın.

## Hızlı Başlangıç

```bash
# 1. Yapay zeka aracınız için kurun
rtk init -g                     # Claude Code / Copilot (varsayılan)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init --agent windsurf       # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init --agent hermes         # Hermes

# 2. Yapay zeka aracınızı yeniden başlatın, ardından test edin
git status  # Otomatik olarak 'rtk git status' şeklinde yeniden yazılır
```

Kanca (hook) tabanlı aracılar, Bash komutlarını (örneğin, `git status` -> `rtk git status`) yürütmeden önce yeniden yazar. Hermes dahil eklenti (plugin) tabanlı aracılar, komutları yürütmeden önce yeniden yazmak için kendi eklenti API'lerini kullanır. Aracı, `rtk`yı açıkça çağırmaya gerek kalmadan sıkıştırılmış çıktı alır.

**Önemli:** Kanca sadece Bash araç çağrılarında çalışır. Claude Code'un `Read`, `Grep` ve `Glob` gibi yerleşik araçları Bash kancasından geçmez, bu nedenle otomatik olarak yeniden yazılmazlar. Bu iş akışları için RTK'nın kompakt çıktısını almak istiyorsanız, kabuk komutlarını (`cat`/`head`/`tail`, `rg`/`grep`, `find`) kullanın veya doğrudan `rtk read`, `rtk grep` veya `rtk find` komutlarını çağırın.

## Nasıl Çalışır

```
  rtk olmadan:                                    rtk ile:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 token (ham)         |            |   ~200 token         | filtre   |
    +-----------------------------------+            +------- (filtrelenmiş) ---+----------+
```

Komut türü başına uygulanan dört strateji:

1. **Akıllı Filtreleme** - Gürültüyü (yorumlar, boşluklar, kalıplaşmış kodlar) kaldırır
2. **Gruplama** - Benzer öğeleri bir araya getirir (dizine göre dosyalar, türe göre hatalar)
3. **Kırpma** - İlgili bağlamı korur, fazlalıkları keser
4. **Tekilleştirme** - Tekrarlanan günlük satırlarını sayılarla birleştirir

## Komutlar

### Dosyalar
```bash
rtk ls .                        # Token-optimize edilmiş dizin ağacı
rtk read file.rs                # Akıllı dosya okuma
rtk read file.rs -l aggressive  # Sadece imzalar (gövdeleri çıkarır)
rtk smart file.rs               # 2 satırlık sezgisel kod özeti
rtk find "*.rs" .               # Kompakt bulma sonuçları
rtk grep "pattern" .            # Gruplanmış arama sonuçları
rtk diff file1 file2            # Yoğunlaştırılmış diff
```

### Git
```bash
rtk git status                  # Kompakt durum
rtk git log -n 10               # Tek satırlık commit'ler
rtk git diff                    # Yoğunlaştırılmış diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # Kompakt PR listeleme
rtk gh pr view 42               # PR detayları + kontroller
rtk gh issue list               # Kompakt sorun (issue) listeleme
rtk gh run list                 # İş akışı (workflow) çalışma durumu
```

### Test Çalıştırıcılar
```bash
rtk jest                        # Jest kompakt (sadece hatalar)
rtk vitest                      # Vitest kompakt (sadece hatalar)
rtk playwright test             # E2E sonuçları (sadece hatalar)
rtk pytest                      # Python testleri (-90%)
rtk go test                     # Go testleri (NDJSON, -90%)
rtk cargo test                  # Cargo testleri (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # RSpec testleri (JSON, -60%+)
rtk err <cmd>                   # Herhangi bir komuttan sadece hataları filtrele
rtk test <cmd>                  # Genel test sarmalayıcı - sadece hatalar (-90%)
```

### İnşa & Lint (Build & Lint)
```bash
rtk lint                        # Kural/dosyaya göre gruplanmış ESLint
rtk lint biome                  # Diğer linter'ları destekler
rtk tsc                         # Dosyaya göre gruplanmış TypeScript hataları
rtk next build                  # Next.js inşa kompakt
rtk prettier --check .          # Formatlama gerektiren dosyalar
rtk cargo build                 # Cargo inşa (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Python linting (JSON, -80%)
rtk golangci-lint run           # Go linting (JSON, -85%)
rtk rubocop                     # Ruby linting (JSON, -60%+)
```

### Paket Yöneticileri
```bash
rtk pnpm list                   # Kompakt bağımlılık ağacı
rtk pip list                    # Python paketleri (uv otomatik algılanır)
rtk pip outdated                # Güncel olmayan paketler
rtk bundle install              # Ruby gem'leri ('Using' satırlarını temizler)
rtk prisma generate             # Şema oluşturma (ASCII sanatı olmadan)
```

### AWS
```bash
rtk aws sts get-caller-identity # Tek satırlık kimlik
rtk aws ec2 describe-instances  # Kompakt örnek (instance) listesi
rtk aws lambda list-functions   # İsim/çalışma zamanı/bellek (sırları temizler)
rtk aws logs get-log-events     # Sadece zaman damgalı mesajlar
rtk aws cloudformation describe-stack-events  # Önce hatalar
rtk aws dynamodb scan           # Tür ek açıklamalarını açar
rtk aws iam list-roles          # Politika belgelerini temizler
rtk aws s3 ls                   # Tee kurtarma ile kırpılmış
```

### Konteynırlar
```bash
rtk docker ps                   # Kompakt konteynır listesi
rtk docker images               # Kompakt imaj listesi
rtk docker logs <container>     # Tekilleştirilmiş günlükler
rtk docker compose ps           # Compose servisleri
rtk kubectl pods                # Kompakt pod listesi
rtk kubectl logs <pod>          # Tekilleştirilmiş günlükler
rtk kubectl services            # Kompakt servis listesi
```

### Veri & Analiz
```bash
rtk json config.json            # Değerler olmadan yapı
rtk deps                        # Bağımlılıklar özeti
rtk env -f AWS                  # Filtrelenmiş ortam değişkenleri
rtk log app.log                 # Tekilleştirilmiş günlükler
rtk curl <url>                  # Kırp + tam çıktıyı kaydet
rtk wget <url>                  # İndir, ilerleme çubuklarını temizle
rtk summary <long command>      # Sezgisel özet
rtk proxy <command>             # Ham geçiş + izleme
```

### Token Tasarrufu Analitiği
```bash
rtk gain                        # Özet istatistikler
rtk gain --graph                # ASCII grafiği (son 30 gün)
rtk gain --history              # Son komut geçmişi
rtk gain --daily                # Günlük döküm
rtk gain --all --format json    # Panolar için JSON dışa aktarma

rtk discover                    # Kaçırılan tasarruf fırsatlarını bul
rtk discover --all --since 7    # Tüm projeler, son 7 gün

rtk session                     # Son oturumlardaki RTK kullanımını göster
```

## Genel Bayraklar

```bash
-u, --ultra-compact    # ASCII simgeleri, satır içi format (ekstra token tasarrufu)
-v, --verbose          # Ayrıntı düzeyini artır (-v, -vv, -vvv)
```

## Örnekler

**Dizin listeleme:**
```
# ls -la (45 satır, ~800 token)        # rtk ls (12 satır, ~150 token)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 dosya)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git işlemleri:**
```
# git push (15 satır, ~200 token)       # rtk git push (1 satır, ~10 token)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**Test çıktısı:**
```
# cargo test (Hata durumunda 200+ satır) # rtk test cargo test (~20 satır)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## Otomatik Yeniden Yazma Kancası (Auto-Rewrite Hook)

rtk'yı kullanmanın en etkili yolu. Kanca, Bash komutlarını şeffaf bir şekilde yakalar ve yürütmeden önce rtk eşdeğerlerine yeniden yazar.

**Sonuç**: Tüm konuşmalarda ve alt aracılarda %100 rtk benimsenmesi, sıfır token ek yükü.

**Kapsam notu:** Bu sadece Bash araç çağrıları için geçerlidir. Claude Code'un `Read`, `Grep` ve `Glob` gibi yerleşik araçları kancayı devre dışı bırakır, bu nedenle buralarda RTK filtrelemesi istiyorsanız kabuk komutlarını veya açık `rtk` komutlarını kullanın.

### Kurulum

```bash
rtk init -g                 # Kanca + RTK.md kurulumu (önerilen)
rtk init -g --opencode      # OpenCode eklentisi (Claude Code yerine)
rtk init -g --auto-patch    # Etkileşimsiz (CI/CD)
rtk init -g --hook-only     # Sadece kanca, RTK.md yok
rtk init --show             # Kurulumu doğrula
```

Kurulumdan sonra, **Claude Code'u yeniden başlatın**.

## Windows

RTK, Windows üzerinde bazı sınırlamalarla çalışır. Otomatik yeniden yazma kancası (`rtk-rewrite.sh`) bir Unix kabuğu gerektirir, bu nedenle yerel Windows'ta RTK, **CLAUDE.md enjeksiyon moduna** geri döner — yapay zeka yardımcınız RTK talimatlarını alır ancak komutlar otomatik olarak yeniden yazılmaz.

### Önerilen: WSL (tam destek)

En iyi deneyim için, [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) (Linux için Windows Alt Sistemi) kullanın. WSL içinde RTK, tıpkı Linux gibi çalışır — tam kanca desteği, otomatik yeniden yazma, her şey:

```bash
# WSL içinde
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

### Yerel Windows (sınırlı destek)

Yerel Windows'ta (cmd.exe / PowerShell), RTK filtreleri çalışır ancak kanca komutları otomatik olarak yeniden yazmaz:

```powershell
# 1. Sürümlerden rtk-x86_64-pc-windows-msvc.zip dosyasını indirin ve çıkartın
# 2. rtk.exe dosyasını PATH'inize ekleyin
# 3. Başlatın (CLAUDE.md enjeksiyonuna geri döner)
rtk init -g
# 4. rtk'yı açıkça kullanın
rtk cargo test
rtk git status
```

**Önemli**: `rtk.exe` dosyasına çift tıklamayın — bu, kullanım bilgilerini yazdıran ve hemen çıkan bir CLI aracıdır. Her zaman bir terminalden (Komut İstemi, PowerShell veya Windows Terminal) çalıştırın.

| Özellik | WSL | Yerel Windows |
|---------|-----|----------------|
| Filtreler (cargo, git, vb.) | Tam | Tam |
| Otomatik yeniden yazma kancası | Evet | Hayır (CLAUDE.md geri dönüşü) |
| `rtk init -g` | Kanca modu | CLAUDE.md modu |
| `rtk gain` / analitik | Tam | Tam |

## Desteklenen Yapay Zeka Araçları

RTK, 13 yapay zeka kodlama aracını destekler. Her entegrasyon, aracının komut yakalamayı desteklediği durumlarda %60-90 token tasarrufu için kabuk komutlarını `rtk` eşdeğerlerine yeniden yazar.

| Araç | Kurulum | Yöntem |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse kancası (bash) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse kancası — şeffaf yeniden yazma |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse öneriyle reddetme (CLI sınırlaması) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse kancası (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool kancası |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md talimatları |
| **Windsurf** | `rtk init --agent windsurf` | .windsurfrules (proje kapsamlı) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (proje kapsamlı) |
| **OpenCode** | `rtk init -g --opencode` | Eklenti TS (tool.execute.before) |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Eklenti TS (before_tool_call) |
| **Hermes** | `rtk init --agent hermes` | Python eklenti adaptörü (`rtk rewrite` aracılığıyla terminal komut mutasyonu) |
| **Mistral Vibe** | Planlanıyor ([#800](https://github.com/rtk-ai/rtk/issues/800)) | Üst akışta engellendi |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md (proje kapsamlı) |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md (proje kapsamlı) |

Aracı başına kurulum detayları, geçersiz kılma kontrolleri ve zarif bozulma (graceful degradation) için [Desteklenen Aracılar kılavuzuna](https://www.rtk-ai.app/guide/getting-started/supported-agents) bakın. Hermes eklentisi kaynak kodu ve testleri `hooks/hermes/` altındadır; kurulu Hermes çalışma zamanı dosyaları hala `~/.hermes/plugins/rtk-rewrite/` altında bulunur.

## Yapılandırma

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # bunlar için yeniden yazmayı atla

[tee]
enabled = true          # hata durumunda ham çıktıyı kaydet (varsayılan: true)
mode = "failures"       # "failures", "always" veya "never"
```

Bir komut başarısız olduğunda, RTK tam filtrelenmemiş çıktıyı kaydeder, böylece LLM bunu yeniden yürütmeden okuyabilir:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

Tam yapılandırma referansı (tüm bölümler, ortam değişkenleri, proje başına filtreler) için [Yapılandırma kılavuzuna](https://www.rtk-ai.app/guide/getting-started/configuration) bakın.

### Kaldırma (Uninstall)

```bash
rtk init -g --uninstall     # Kancayı, RTK.md'yi ve settings.json girişini kaldır
cargo uninstall rtk          # İkiliyi kaldır
brew uninstall rtk           # Homebrew ile kurulduysa
```

## Dokümantasyon

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — tam kullanıcı kılavuzu (kurulum, desteklenen aracılar, nelerin optimize edildiği, analitik, yapılandırma, sorun giderme)
- **[INSTALL.md](INSTALL.md)** — detaylı kurulum referansı
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — sistem tasarımı ve teknik kararlar
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — katkıda bulunma kılavuzu
- **[SECURITY.md](SECURITY.md)** — güvenlik politikası

## Gizlilik ve Telemetri

RTK, günde bir kez **anonim, toplu kullanım metrikleri** toplayabilir. Telemetri **varsayılan olarak devre dışıdır** ve `rtk init` sırasında veya `rtk telemetry enable` aracılığıyla **açık onay (opt-in)** (GDPR Madde 6, 7) gerektirir. Bu veriler daha iyi bir ürün oluşturmamıza yardımcı olur: hangi komutların filtrelere ihtiyacı olduğunu, hangi filtrelerin iyileştirilmesi gerektiğini ve RTK'nın ne kadar değer sunduğunu belirlememizi sağlar. Alanların tam listesi, veri işleme ve katkıda bulunanlar için kılavuzlar için **[docs/TELEMETRY.md](docs/TELEMETRY.md)** dosyasına bakın.

**Neler toplanır ve neden:**

| Kategori | Veri | Neden |
|----------|------|-----|
| Kimlik | Tuzlanmış (salted) cihaz karması (SHA-256, geri döndürülemez) | Bireyleri takip etmeden benzersiz kurulumları saymak |
| Ortam | RTK sürümü, İşletim Sistemi, mimari, kurulum yöntemi | Hangi platformların destekleneceğini ve test edileceğini bilmek |
| Kullanım hacmi | Komut sayısı (24s), toplam komutlar, kaydedilen token'lar (24s/30g/toplam) | Benimsenmeyi ve sunulan değeri ölçmek |
| Kalite | En çok kullanılan 5 geçiş komutu (%0 tasarruf), ayrıştırma hatası sayısı, <%30 tasarruflu komutlar | Eksik ve geliştirilmesi gereken zayıf filtreleri belirlemek |
| Ekosistem | Komut kategorisi dağılımı (örneğin git %45, cargo %20, js %15) | Popüler ekosistemler için filtre geliştirmeye öncelik vermek |
| Elde tutma | İlk kullanımdan bu yana geçen gün, son 30 gündeki aktif gün sayısı | Etkileşimi anlamak ve müşteri kaybını (churn) tespit etmek |
| Benimseme | Yapay zeka aracı kanca türü (claude/gemini/codex), özel TOML filtre sayısı | Entegrasyon kapsamını ve DSL benimsenmesini izlemek |
| Yapılandırma | config.toml dosyasının varlığı, hariç tutulan komut sayısı, proje sayısı | Kullanıcı olgunluğunu ve özelleştirme kalıplarını anlamak |
| Özellikler | Meta komutlar için kullanım sayıları (gain, discover, proxy, verify) | Hangi RTK özelliklerinin değerli olduğunu, hangilerinin kullanılmadığını bilmek |
| Ekonomi | Tahmini USD tasarrufu (API token fiyatlandırmasına göre) | RTK'nın kullanıcılara sağladığı değeri nicelleştirmek |

Tüm veriler **toplam sayılar veya anonimleştirilmiş komut adlarıdır** (ilk 3 kelime, argüman yok). En popüler komutlar sadece araç adlarını bildirir (örneğin "git", "cargo"), asla tam komut satırlarını bildirmez.

**Neler toplanmaz:** kaynak kodu, dosya yolları, komut argümanları, sırlar, ortam değişkenleri, kişisel veriler veya depo içerikleri.

**Telemetriyi yönetin:**
```bash
rtk telemetry status     # Mevcut onay durumunu kontrol et
rtk telemetry enable     # Onay ver (etkileşimli istem)
rtk telemetry disable    # Onayı geri çek — tüm veri toplamayı hemen durdurur
rtk telemetry forget     # Onayı geri çek + tüm yerel verileri sil + sunucu tarafında silme talep et
```

**Ortam değişkeni ile geçersiz kılma:**
```bash
export RTK_TELEMETRY_DISABLED=1   # Onaydan bağımsız olarak telemetriyi engeller
```

## Yıldız Geçmişi (Star History)

<a href="https://www.star-history.com/?repos=rtk-ai%2Frtk&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
 </picture>
</a>

## StarMapper

<a href="https://starmapper.bruniaux.com/rtk-ai/rtk">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=light" />
    <img alt="StarMapper" src="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk" />
  </picture>
</a>

## Çekirdek Ekip

- **Patrick Szymkowiak** — Kurucu
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — Ana Katılımcı
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — Ana Katılımcı
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)

## Katkıda Bulunma

Katkılarınızı bekliyoruz! Lütfen [GitHub](https://github.com/rtk-ai/rtk) üzerinden bir konu (issue) veya PR açın.

[Discord](https://discord.gg/RySmvNF5kF) üzerindeki topluluğumuza katılın.

## Lisans

Apache Lisansı 2.0 - detaylar için [LICENSE](LICENSE) dosyasına bakın.

## Sorumluluk Reddi (Disclaimer)

Bakınız [DISCLAIMER.md](DISCLAIMER.md).
