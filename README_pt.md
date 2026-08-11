<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alta performance que corta até 90% da saída bash que seu agente lê</strong>
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

## O que o RTK faz

O RTK intercepta comandos de shell e comprime a saída antes que seu agente a leia.

| Operação | O que o RTK faz com a saída |
|-----------|-----------------------------|
| `ls` / `tree` | Formato de árvore com contagem de arquivos em vez de uma linha por entrada |
| `cat` / `read` | Leitura inteligente: assinaturas e estrutura em vez de corpos completos |
| `grep` / `rg` | Trunca linhas longas, agrupa correspondências por arquivo |
| `git status` | Formato stat compacto, agrupado por estado |
| `git diff` | Contexto reduzido, cabeçalhos removidos |
| `git log` | Apenas hash, autor e assunto |
| `git add/commit/push` | Linha de confirmação em vez da saída de progresso completa |
| `cargo test` / `npm test` | Apenas falhas, testes aprovados reduzidos a um contador |
| `ruff check` | Agrupado por regra e arquivo |
| `pytest` | Apenas falhas, traceback encurtado |
| `go test` | NDJSON parseado, apenas falhas |
| `docker ps` | Apenas campos essenciais |

## Como funciona a economia

O RTK corta **até 90% da saída bash** que seu agente lê. É isso que o RTK mede, e não é a mesma coisa que reduzir sua fatura em 90%.

A saída bash é **um dos contribuintes para os tokens de entrada**, ao lado do seu prompt, do prompt de sistema e do histórico da conversa. Os tokens de entrada são, por sua vez, **apenas parte da fatura**, que também conta os tokens de saída. A redução se dilui a cada etapa.

As contagens de tokens que o RTK reporta são estimadas como `bytes / 4`: o RTK não embarca nenhum tokenizador, portanto os **percentuais são confiáveis, mas os números absolutos de tokens são aproximados**.

> Explicação completa: [Como funciona a economia do RTK](docs/guide/resources/savings-explained.md)

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

> Os percentuais abaixo são **reduções de bytes da saída bash**, medidas com o estimador `bytes / 4` do RTK. Veja [Como funciona a economia](#como-funciona-a-economia).

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
