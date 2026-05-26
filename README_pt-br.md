<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alto desempenho que reduz o consumo de tokens de LLM em 60–90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Website</a> &bull;
  <a href="#instalacao">Instalar</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Solução de problemas</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Arquitetura</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Français</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Español</a> &bull;
    <a href="README_pt-br.md">Português Brasileiro</a>
</p>

---

rtk filtra e comprime as saídas de comandos antes que cheguem ao contexto do seu LLM. Binário único em Rust, sem dependências, com menos de 10ms de overhead.

## Economia de tokens (sessão de 30 min no Claude Code)

| Operação | Frequência | Padrão | rtk | Economia |
|-----------|------------|----------|-----|--------|
| `ls` / `tree` | 10x | 2.000 | 400 | -80% |
| `cat` / `read` | 20x | 40.000 | 12.000 | -70% |
| `grep` / `rg` | 8x | 16.000 | 3.200 | -80% |
| `git status` | 10x | 3.000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25.000 | 2.500 | -90% |
| **Total** | | **~118.000** | **~23.900** | **-80%** |

## Instalação

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
rtk --version   # Deve mostrar "rtk 0.27.x"
rtk gain        # Deve mostrar estatísticas de economia
```

## Início rapido

```bash
# 1. Instalar hook para Claude Code (recomendado)
rtk init --global

# 2. Reiniciar o Claude Code e testar
git status  # Automaticamente reescrito para rtk git status
```

## Como funciona

```
  Sem rtk:                                         Com rtk:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens (crudo)      |             |   ~200 tokens        | filtro   |
    +-----------------------------------+             +------- (filtrado) ---+----------+
```

Quatro estratégias:

1. **Filtragem inteligente** - Remove ruído (comentários, espaços, boilerplate)
2. **Agrupamento** - Agrega itens similares (arquivos por diretório, erros por tipo)
3. **Truncamento** - Mantém contexto relevante e remove redundância
4. **Desduplicação** - Colapsa linhas de log repetidas com contadores

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

### Testes
```bash
rtk jest                        # Jest compacto
rtk vitest                      # Vitest compacto
rtk pytest                      # Testes Python (-90%)
rtk go test                     # Testes Go (-90%)
rtk cargo test                  # Testes Rust (-90%)
rtk test <cmd>                  # Apenas falhas (-90%)
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

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — guia completo do usuário (instalação, agentes suportados, o que é otimizado, análises, configuração, solução de problemas)
- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Resolver problemas comuns
- **[INSTALL.md](INSTALL.md)** - Guia detalhado de instalação
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Arquitetura técnica

## Contribuindo

Contribuições são bem-vindas. Abra uma issue ou PR no [GitHub](https://github.com/rtk-ai/rtk).

Participe da comunidade no [Discord](https://discord.gg/RySmvNF5kF).

## Licença

Licença MIT - ver [LICENSE](LICENSE) para detalhes.

## Aviso legal

Ver [DISCLAIMER.md](DISCLAIMER.md).
