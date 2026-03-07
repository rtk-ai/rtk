<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proksi CLI berkinerja tinggi yang mengurangi konsumsi token LLM hingga 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/gFwRPEKq4p"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Situs web</a> &bull;
  <a href="#instalasi">Instalasi</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Pemecahan Masalah</a> &bull;
  <a href="ARCHITECTURE.md">Arsitektur</a> &bull;
  <a href="https://discord.gg/gFwRPEKq4p">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_id.md">Bahasa Indonesia</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk menyaring dan mengompresi keluaran perintah sebelum masuk ke konteks LLM Anda. Biner Rust tunggal, tanpa dependensi, overhead <10ms.

## Penghematan Token (Sesi 30 menit di Claude Code)

| Operasi | Frekuensi | Standar | rtk | Penghematan |
|---------|-----------|---------|-----|-------------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **Total** | | **~118,000** | **~23,900** | **-80%** |

## Instalasi

### Homebrew (direkomendasikan)

```bash
brew install rtk
```

### Instalasi Cepat (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Verifikasi

```bash
rtk --version   # Harus menampilkan "rtk 0.27.x"
rtk gain        # Harus menampilkan statistik penghematan
```

## Mulai Cepat

```bash
# 1. Pasang hook untuk Claude Code (direkomendasikan)
rtk init --global

# 2. Restart Claude Code, lalu uji
git status  # Secara otomatis ditulis ulang menjadi rtk git status
```

## Cara Kerja

```
  Tanpa rtk:                                       Dengan rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 token (mentah)      |            |   ~200 token         | filter   |
    +-----------------------------------+            +------- (terfilter) --+----------+
```

Empat strategi:

1. **Penyaringan cerdas** - Menghilangkan noise (komentar, spasi, boilerplate)
2. **Pengelompokan** - Mengelompokkan item serupa (berkas per direktori, error per jenis)
3. **Pemotongan** - Menjaga konteks relevan, memangkas redundansi
4. **Deduplikasi** - Merangkum baris log berulang dengan penghitung

## Perintah

### Berkas
```bash
rtk ls .                        # Pohon direktori yang dioptimalkan
rtk read file.rs                # Pembacaan berkas cerdas
rtk find "*.rs" .               # Hasil pencarian ringkas
rtk grep "pattern" .            # Pencarian yang dikelompokkan per berkas
```

### Git
```bash
rtk git status                  # Status ringkas
rtk git log -n 10               # Commit satu baris
rtk git diff                    # Diff ringkas
rtk git push                    # -> "ok main"
```

### Pengujian
```bash
rtk test cargo test             # Hanya kegagalan (-90%)
rtk vitest run                  # Vitest ringkas
rtk pytest                      # Pengujian Python (-90%)
rtk go test                     # Pengujian Go (-90%)
```

### Build & Lint
```bash
rtk lint                        # ESLint dikelompokkan per aturan
rtk tsc                         # Error TypeScript dikelompokkan
rtk cargo build                 # Build Cargo (-80%)
rtk ruff check                  # Lint Python (-80%)
```

### Analitik
```bash
rtk gain                        # Statistik penghematan
rtk gain --graph                # Grafik ASCII (30 hari)
rtk discover                    # Menemukan penghematan yang terlewat
```

## Dokumentasi

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Mengatasi masalah umum
- **[INSTALL.md](INSTALL.md)** - Panduan instalasi rinci
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Arsitektur teknis

## Kontribusi

Kontribusi sangat diterima. Buka issue atau PR di [GitHub](https://github.com/rtk-ai/rtk).

Bergabung dengan komunitas di [Discord](https://discord.gg/pvHdzAec).

## Lisensi

Lisensi MIT - lihat [LICENSE](LICENSE) untuk detail.
