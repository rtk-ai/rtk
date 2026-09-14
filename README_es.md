<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alto rendimiento que elimina hasta el 90% de la salida bash que lee tu agente</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Sitio web</a> &bull;
  <a href="#instalacion">Instalar</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Solucion de problemas</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Arquitectura</a> &bull;
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

## Que hace RTK

RTK intercepta comandos de shell y comprime su salida antes de que tu agente la lea.

| Operacion | Que hace RTK con la salida |
|-----------|----------------------------|
| `ls` / `tree` | Formato de arbol con conteo de archivos en lugar de una linea por entrada |
| `cat` / `read` | Lectura inteligente: firmas y estructura en vez de cuerpos completos |
| `grep` / `rg` | Trunca lineas largas, agrupa coincidencias por archivo |
| `git status` | Formato stat compacto, agrupado por estado |
| `git diff` | Contexto reducido, cabeceras eliminadas |
| `git log` | Solo hash, autor y asunto |
| `git add/commit/push` | Linea de confirmacion en lugar de la salida de progreso completa |
| `cargo test` / `npm test` | Solo fallos, los tests que pasan se reducen a un contador |
| `ruff check` | Agrupado por regla y archivo |
| `pytest` | Solo fallos, traceback recortado |
| `go test` | NDJSON parseado, solo fallos |
| `docker ps` | Solo campos esenciales |

## Como funciona el ahorro

RTK elimina **hasta el 90% de la salida bash** que lee tu agente. Eso es lo que RTK mide, y no es lo mismo que reducir tu factura en un 90%.

La salida bash es **uno de los factores que alimentan los tokens de entrada**, junto con tu prompt, el prompt del sistema y el historial de conversacion. Los tokens de entrada son a su vez **solo una parte de la factura**, que tambien cuenta los tokens de salida. La reduccion se diluye en cada paso.

Los recuentos de tokens que reporta RTK se estiman como `bytes / 4`: RTK no incluye ningun tokenizador, por lo que los **porcentajes son fiables pero las cifras absolutas de tokens son aproximadas**.

> Explicacion completa: [Como funciona el ahorro en RTK](docs/guide/resources/savings-explained.md)

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

> Los porcentajes de abajo son **reducciones de bytes de salida bash**, medidas con el estimador `bytes / 4` de RTK. Ver [Como funciona el ahorro](#como-funciona-el-ahorro).

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
rtk jest                        # Jest compacto
rtk vitest                      # Vitest compacto
rtk pytest                      # Tests Python (-90%)
rtk go test                     # Tests Go (-90%)
rtk cargo test                  # Tests Rust (-90%)
rtk test <cmd>                  # Solo fallos (-90%)
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
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Arquitectura tecnica

## Contribuir

Las contribuciones son bienvenidas. Abre un issue o PR en [GitHub](https://github.com/rtk-ai/rtk).

Unete a la comunidad en [Discord](https://discord.gg/RySmvNF5kF).

## Licencia

Licencia Apache 2.0 - ver [LICENSE](LICENSE) para detalles.

## Descargo de responsabilidad

Ver [DISCLAIMER.md](DISCLAIMER.md).
