<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alto rendimiento que reduce el consumo de tokens LLM en un 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Sitio web</a> &bull;
  <a href="#instalacion">Instalar</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Solucion de problemas</a> &bull;
  <a href="ARCHITECTURE.md">Arquitectura</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a>
</p>

---

rtk filtra y comprime las salidas de comandos antes de que lleguen al contexto de tu LLM. Binario Rust unico, cero dependencias, <10ms de overhead.

## Ahorro de tokens (sesion de 30 min en Claude Code)

| Operacion | Frecuencia | Estandar | rtk | Ahorro |
|-----------|------------|----------|-----|--------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **Total** | | **~118,000** | **~23,900** | **-80%** |

## Instalacion

### Homebrew (recomendado)

```bash
brew install rtk
```

### Instalacion rapida (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Verificacion

```bash
rtk --version   # Debe mostrar "rtk 0.27.x"
rtk gain        # Debe mostrar estadisticas de ahorro
```

## Inicio rapido

```bash
# 1. Instalar hook para Claude Code (recomendado)
rtk init --global

# 2. Reiniciar Claude Code, luego probar
git status  # Automaticamente reescrito a rtk git status
```

## Como funciona

```
  Sin rtk:                                         Con rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens (crudo)      |             |   ~200 tokens        | filtro   |
    +-----------------------------------+             +------- (filtrado) ---+----------+
```

Cuatro estrategias:

1. **Filtrado inteligente** - Elimina ruido (comentarios, espacios, boilerplate)
2. **Agrupacion** - Agrega elementos similares (archivos por directorio, errores por tipo)
3. **Truncamiento** - Mantiene contexto relevante, elimina redundancia
4. **Deduplicacion** - Colapsa lineas de log repetidas con contadores

## Comandos

### Archivos
```bash
rtk ls .                        # Arbol de directorios optimizado
rtk read file.rs                # Lectura inteligente
rtk find "*.rs" .               # Resultados compactos
rtk grep "pattern" .            # Busqueda agrupada por archivo
```

### Git
```bash
rtk git status                  # Estado compacto
rtk git log -n 10               # Commits en una linea
rtk git diff                    # Diff condensado
rtk git push                    # -> "ok main"
```

### Tests
```bash
rtk test cargo test             # Solo fallos (-90%)
rtk vitest run                  # Vitest compacto
rtk pytest                      # Tests Python (-90%)
rtk go test                     # Tests Go (-90%)
```

### Build & Lint
```bash
rtk lint                        # ESLint agrupado por regla
rtk tsc                         # Errores TypeScript agrupados
rtk cargo build                 # Build Cargo (-80%)
rtk ruff check                  # Lint Python (-80%)
```

### Analiticas
```bash
rtk gain                        # Estadisticas de ahorro
rtk gain --graph                # Grafico ASCII (30 dias)
rtk discover                    # Descubrir ahorros perdidos
```

## Documentacion

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Resolver problemas comunes
- **[INSTALL.md](INSTALL.md)** - Guia de instalacion detallada
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Arquitectura tecnica

## Contribuir

Las contribuciones son bienvenidas. Abre un issue o PR en [GitHub](https://github.com/rtk-ai/rtk).

Unete a la comunidad en [Discord](https://discord.gg/RySmvNF5kF).

## Licencia

Licencia MIT - ver [LICENSE](LICENSE) para detalles.
