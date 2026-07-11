<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alta performance que reduz o consumo de tokens LLM em 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Site</a> &bull;
  <a href="#instalacao">Instalar</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">Solução de problemas</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Arquitetura</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a>
</p>

---

rtk filtra e comprime saídas de comandos antes de chegarem ao contexto do seu LLM. Binário Rust único, zero dependências, overhead inferior a 10ms.

## Economia de tokens (sessão de 30 min no Claude Code)

| Operação | Frequência | Padrão | rtk | Economia |
|-----------|------------|----------|-----|--------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **Total** | | **~118,000** | **~23,900** | **-80%** |

## Instalacao

### Homebrew (recomendado)

```bash
brew install rtk
```

### Instalação rápida (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Verificação

```bash
rtk --version   # Deve exibir "rtk 0.28.2"
rtk gain        # Deve exibir estatísticas de economia
```

## Inicio rapido

```bash
# 1. Instalar hook para Claude Code (recomendado)
rtk init --global

# 2. Reiniciar Claude Code, depois testar
git status  # Reescrito automaticamente para rtk git status
```

## Como funciona

```
  Sem rtk:                                        Com rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |        ~2,000 tokens (bruto)      |            |   ~200 tokens        | filtro   |
    +-----------------------------------+            +------- (filtrado) ---+----------+
```

Quatro estratégias:

1. **Filtragem inteligente** - Elimina ruído (comentários, espaços, boilerplate)
2. **Agrupamento** - Agrega itens similares (arquivos por diretório, erros por tipo)
3. **Truncamento** - Mantém contexto relevante, elimina redundância
4. **Deduplicação** - Colapsa linhas de log repetidas com contadores

## Comandos

### Arquivos
```bash
rtk ls .                        # Árvore de diretórios otimizada
rtk read file.rs                # Leitura inteligente
rtk find "*.rs" .               # Resultados compactos
rtk grep "pattern" .            # Busca agrupada por arquivo
```

### Git
```bash
rtk git status                  # Status compacto
rtk git log -n 10               # Commits em uma linha
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
rtk test <cmd>                  # Só falhas (-90%)
```

### Build & Lint
```bash
rtk lint                        # ESLint agrupado por regra
rtk tsc                         # Erros TypeScript agrupados
rtk cargo build                 # Build Cargo (-80%)
rtk ruff check                  # Lint Python (-80%)
```

### Análises
```bash
rtk gain                        # Estatísticas de economia
rtk gain --graph                # Gráfico ASCII (30 dias)
rtk discover                    # Descobrir economias perdidas
```

## Documentação

- **[INSTALL.md](INSTALL.md)** - Guia de instalação detalhado
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Arquitetura técnica
- **[CONTRIBUTING.md](CONTRIBUTING.md)** - Guia de contribuição

## Contribuir

Contribuições são bem-vindas. Abra uma issue ou PR no [GitHub](https://github.com/rtk-ai/rtk).

Junte-se à comunidade no [Discord](https://discord.gg/RySmvNF5kF).

## Licença

Apache License 2.0 - veja [LICENSE](LICENSE) para detalhes.

## Aviso Legal

Veja [DISCLAIMER.md](DISCLAIMER.md).
