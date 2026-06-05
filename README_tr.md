<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>LLM token tüketimini %60-90 azaltan yüksek performanslı CLI proxy</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Web sitesi</a> &bull;
  <a href="#kurulum">Kurulum</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Sorun giderme</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Mimari</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Français</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Español</a> &bull;
  <a href="README_tr.md">Türkçe</a>
</p>

---

rtk, komut çıktılarını LLM bağlamına ulaşmadan önce filtreleyip sıkıştırır. Tek bir Rust binary'si, sıfır bağımlılık, <10ms ek yük.

## Token tasarrufu (Claude Code'da 30 dakikalık oturum)

| İşlem | Sıklık | Standart | rtk | Tasarruf |
|-------|--------|----------|-----|----------|
| `ls` / `tree` | 10x | 2.000 | 400 | -%80 |
| `cat` / `read` | 20x | 40.000 | 12.000 | -%70 |
| `grep` / `rg` | 8x | 16.000 | 3.200 | -%80 |
| `git status` | 10x | 3.000 | 600 | -%80 |
| `cargo test` / `npm test` | 5x | 25.000 | 2.500 | -%90 |
| **Toplam** | | **~118.000** | **~23.900** | **-%80** |

## Kurulum

### Homebrew (önerilen)

```bash
brew install rtk
```

### Hızlı kurulum (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Doğrulama

```bash
rtk --version   # "rtk 0.27.x" göstermeli
rtk gain        # Tasarruf istatistiklerini göstermeli
```

## Hızlı başlangıç

```bash
# 1. Claude Code için hook'u kur (önerilen)
rtk init --global

# 2. Claude Code'u yeniden başlat ve dene
git status  # Otomatik olarak rtk git status'e yeniden yazılır
```

## Nasıl çalışır

```
  rtk olmadan:                                     rtk ile:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2.000 token (ham)         |             |   ~200 token         | filtre   |
    +-----------------------------------+             +------ (filtrelenmiş) +----------+
```

Dört strateji:

1. **Akıllı filtreleme** - Gürültüyü kaldırır (yorumlar, boşluklar, gereksiz tekrarlar)
2. **Gruplama** - Benzer öğeleri toplar (dizine göre dosyalar, türe göre hatalar)
3. **Kısaltma** - İlgili bağlamı korur, fazlalıkları atar
4. **Tekrar tekilleştirme** - Tekrarlayan log satırlarını sayaçla birleştirir

## Komutlar

### Dosyalar
```bash
rtk ls .                        # Optimize edilmiş dizin ağacı
rtk read file.rs                # Akıllı okuma
rtk find "*.rs" .               # Kompakt sonuçlar
rtk grep "pattern" .            # Dosyaya göre gruplanmış arama
```

### Git
```bash
rtk git status                  # Kompakt durum
rtk git log -n 10               # Tek satırlık commit'ler
rtk git diff                    # Yoğunlaştırılmış diff
rtk git push                    # -> "ok main"
```

### Testler
```bash
rtk jest                        # Kompakt Jest
rtk vitest                      # Kompakt Vitest
rtk pytest                      # Python testleri (-%90)
rtk go test                     # Go testleri (-%90)
rtk cargo test                  # Rust testleri (-%90)
rtk test <cmd>                  # Sadece başarısızlar (-%90)
```

### Build & Lint
```bash
rtk lint                        # Kurala göre gruplanmış ESLint
rtk tsc                         # Gruplanmış TypeScript hataları
rtk cargo build                 # Cargo build (-%80)
rtk ruff check                  # Python lint (-%80)
```

### Analitik
```bash
rtk gain                        # Tasarruf istatistikleri
rtk gain --graph                # ASCII grafik (30 gün)
rtk discover                    # Kaçırılan tasarrufları keşfet
```

## Dokümantasyon

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Yaygın sorunları çözme
- **[INSTALL.md](INSTALL.md)** - Detaylı kurulum kılavuzu
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Teknik mimari

## Katkıda bulunma

Katkılar memnuniyetle karşılanır. [GitHub](https://github.com/rtk-ai/rtk) üzerinden issue veya PR açın.

[Discord](https://discord.gg/RySmvNF5kF) topluluğuna katılın.

## Lisans

MIT Lisansı - detaylar için [LICENSE](LICENSE) dosyasına bakın.

## Sorumluluk reddi

[DISCLAIMER.md](DISCLAIMER.md) dosyasına bakın.
