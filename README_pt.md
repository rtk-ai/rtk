<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Proxy CLI de alta performance que reduz o consumo de tokens de LLM em 60-90%</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Website</a> &bull;
  <a href="#instalacao">Instalação</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">Solução de problemas</a> &bull;
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
  <a href="README_pt.md">Português (BR)</a>
</p>

---

rtk filtra e comprime saídas de comandos antes que cheguem ao contexto do seu LLM. Binário único em Rust, 100+ comandos suportados, overhead <10ms.

## Economia de Tokens (Sessão de 30 min no Claude Code)

| Operação | Frequência | Padrão | rtk | Economia |
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
| **Total** | | **~118,000** | **~23,900** | **-80%** |

> Estimativas baseadas em projetos TypeScript/Rust de tamanho médio. A economia real varia conforme o tamanho do projeto.

## Instalacao

### Homebrew (recomendado)

```bash
brew install rtk
```

### Instalacao Rapida (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> Instala em `~/.local/bin`. Adicione ao PATH se necessário:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # ou ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### Binarios Pre-compilados

Baixe em [releases](https://github.com/rtk-ai/rtk/releases):
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

### Verificar Instalacao

```bash
rtk --version   # Deve mostrar "rtk 0.28.2"
rtk gain        # Deve mostrar estatísticas de economia de tokens
```

> **Aviso de conflito de nome**: Outro projeto chamado "rtk" (Rust Type Kit) existe no crates.io. Se `rtk gain` falhar, você instalou o pacote errado. Use `cargo install --git` acima.

## Inicio Rapido

```bash
# 1. Instale para sua ferramenta de IA
rtk init -g                     # Claude Code / Copilot (padrão)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init --agent windsurf       # Windsurf
rtk init --agent cline          # Cline / Roo Code

# 2. Reinicie sua ferramenta de IA e teste
git status  # Reescrito automaticamente para rtk git status
```

O hook reescreve comandos Bash de forma transparente (ex.: `git status` -> `rtk git status`) antes da execução. O Claude não vê a reescrita, ele apenas recebe a saída comprimida.

**Importante:** o hook só roda em chamadas de ferramenta Bash. Ferramentas nativas do Claude Code como `Read`, `Grep` e `Glob` não passam pelo hook de Bash, então não são reescritas automaticamente. Para obter a saída compacta do RTK nesses fluxos, use comandos shell (`cat`/`head`/`tail`, `rg`/`grep`, `find`) ou chame `rtk read`, `rtk grep` ou `rtk find` diretamente.

## Como Funciona

```
  Sem rtk:                                      Com rtk:

  Claude  --git status-->  shell  -->  git       Claude  --git status-->  RTK  -->  git
    ^                                   |          ^                      |          |
    |        ~2,000 tokens (bruto)      |          |   ~200 tokens        | filtro   |
    +-----------------------------------+          +------- (filtrado) ---+----------+
```

Quatro estratégias aplicadas por tipo de comando:

1. **Filtragem Inteligente** - Remove ruído (comentários, espaços em branco, boilerplate)
2. **Agrupamento** - Agrega itens semelhantes (arquivos por diretório, erros por tipo)
3. **Truncamento** - Mantém contexto relevante e corta redundância
4. **Deduplicação** - Colapsa linhas de log repetidas com contagens

## Comandos

### Arquivos
```bash
rtk ls .                        # Árvore de diretório otimizada para tokens
rtk read file.rs                # Leitura inteligente de arquivo
rtk read file.rs -l aggressive  # Apenas assinaturas (remove corpos)
rtk smart file.rs               # Resumo heurístico de código em 2 linhas
rtk find "*.rs" .               # Resultado compacto de find
rtk grep "pattern" .            # Resultado de busca agrupado
rtk diff file1 file2            # Diff condensado
```

### Git
```bash
rtk git status                  # Status compacto
rtk git log -n 10               # Commits em uma linha
rtk git diff                    # Diff condensado
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # Lista de PRs compacta
rtk gh pr view 42               # Detalhes de PR + checks
rtk gh issue list               # Lista de issues compacta
rtk gh run list                 # Status de runs de workflow
```

### Runners de Teste
```bash
rtk test cargo test             # Mostra só falhas (-90%)
rtk err npm run build           # Só erros/avisos
rtk vitest run                  # Vitest compacto (somente falhas)
rtk playwright test             # Resultados E2E (somente falhas)
rtk pytest                      # Testes Python (-90%)
rtk go test                     # Testes Go (NDJSON, -90%)
rtk cargo test                  # Testes Cargo (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # Testes RSpec (JSON, -60%+)
```

### Build e Lint
```bash
rtk lint                        # ESLint agrupado por regra/arquivo
rtk lint biome                  # Suporte para outros linters
rtk tsc                         # Erros TypeScript agrupados por arquivo
rtk next build                  # Build Next.js compacto
rtk prettier --check .          # Arquivos com necessidade de formatação
rtk cargo build                 # Build Cargo (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Lint Python (JSON, -80%)
rtk golangci-lint run           # Lint Go (JSON, -85%)
rtk rubocop                     # Lint Ruby (JSON, -60%+)
```

### Gerenciadores de Pacotes
```bash
rtk pnpm list                   # Árvore de dependências compacta
rtk pip list                    # Pacotes Python (auto-detecta uv)
rtk pip outdated                # Pacotes desatualizados
rtk bundle install              # Gems Ruby (remove linhas Using)
rtk prisma generate             # Geração de schema (sem ASCII art)
```

### AWS
```bash
rtk aws sts get-caller-identity # Identidade em uma linha
rtk aws ec2 describe-instances  # Lista de instâncias compacta
rtk aws lambda list-functions   # Nome/runtime/memória (remove segredos)
rtk aws logs get-log-events     # Apenas mensagens com timestamp
rtk aws cloudformation describe-stack-events  # Falhas primeiro
rtk aws dynamodb scan           # Remove annotations de tipo
rtk aws iam list-roles          # Remove documentos de policy
rtk aws s3 ls                   # Truncado com recuperação via tee
```

### Containers
```bash
rtk docker ps                   # Lista compacta de containers
rtk docker images               # Lista compacta de imagens
rtk docker logs <container>     # Logs deduplicados
rtk docker compose ps           # Serviços do compose
rtk kubectl pods                # Lista compacta de pods
rtk kubectl logs <pod>          # Logs deduplicados
rtk kubectl services            # Lista compacta de serviços
```

### Dados e Analytics
```bash
rtk json config.json            # Estrutura sem valores
rtk deps                        # Resumo de dependências
rtk env -f AWS                  # Variáveis de ambiente filtradas
rtk log app.log                 # Logs deduplicados
rtk curl <url>                  # Auto-detecta JSON + schema
rtk wget <url>                  # Download, remove barras de progresso
rtk summary <long command>      # Resumo heurístico
rtk proxy <command>             # Pass-through bruto + tracking
```

### Analytics de Economia de Tokens
```bash
rtk gain                        # Estatísticas resumidas
rtk gain --graph                # Gráfico ASCII (últimos 30 dias)
rtk gain --history              # Histórico recente de comandos
rtk gain --daily                # Quebra diária
rtk gain --all --format json    # Exportação JSON para dashboards

rtk discover                    # Descobrir oportunidades de economia perdidas
rtk discover --all --since 7    # Todos os projetos, últimos 7 dias

rtk session                     # Mostra adoção do RTK em sessões recentes
```

## Flags Globais

```bash
-u, --ultra-compact    # Ícones ASCII, formato inline (economia extra)
-v, --verbose          # Aumenta verbosidade (-v, -vv, -vvv)
```

## Exemplos

**Listagem de diretório:**
```
# ls -la (45 linhas, ~800 tokens)        # rtk ls (12 linhas, ~150 tokens)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Operações Git:**
```
# git push (15 linhas, ~200 tokens)      # rtk git push (1 linha, ~10 tokens)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**Saída de teste:**
```
# cargo test (200+ linhas em falha)      # rtk test cargo test (~20 linhas)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## Hook de Reescrita Automatica

A forma mais efetiva de usar rtk. O hook intercepta comandos Bash de forma transparente e os reescreve para equivalentes `rtk` antes da execução.

**Resultado**: 100% de adoção de rtk em todas as conversas e subagentes, sem overhead de token.

**Nota de escopo:** isso se aplica apenas a chamadas de ferramenta Bash. Ferramentas nativas do Claude Code como `Read`, `Grep` e `Glob` ignoram o hook, então use comandos shell ou comandos `rtk` explícitos quando quiser filtragem RTK nesses casos.

### Setup

```bash
rtk init -g                 # Instala hook + RTK.md (recomendado)
rtk init -g --opencode      # Plugin OpenCode (em vez de Claude Code)
rtk init -g --auto-patch    # Não interativo (CI/CD)
rtk init -g --hook-only     # Só hook, sem RTK.md
rtk init --show             # Verificar instalação
```

Após instalar, **reinicie o Claude Code**.

## Ferramentas de IA Suportadas

RTK suporta 10 ferramentas de código com IA. Cada integração reescreve comandos shell para equivalentes `rtk` de forma transparente, com economia de 60-90% de tokens.

| Ferramenta | Instalação | Método |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | Hook PreToolUse (bash) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | Hook PreToolUse (`rtk hook copilot`) — reescrita transparente |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion (limitação da CLI) |
| **Cursor** | `rtk init -g --agent cursor` | hook preToolUse (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | Hook BeforeTool (`rtk hook gemini`) |
| **Codex** | `rtk init -g --codex` | Instruções em AGENTS.md + RTK.md |
| **Windsurf** | `rtk init --agent windsurf` | .windsurfrules (escopo de projeto) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (escopo de projeto) |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS (tool.execute.before) |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS (before_tool_call) |
| **Mistral Vibe** | Planejado (#800) | Bloqueado por suporte upstream de BeforeToolCallback |

### Claude Code (padrao)

```bash
rtk init -g                 # Instala hook + RTK.md
rtk init -g --auto-patch    # Não interativo (CI/CD)
rtk init --show             # Verificar instalação
rtk init -g --uninstall     # Remover
```

### GitHub Copilot (VS Code + CLI)

```bash
rtk init -g --copilot         # Instala hook + instruções
```

Cria `.github/hooks/rtk-rewrite.json` (hook PreToolUse) e `.github/copilot-instructions.md` (consciência em nível de prompt).

O hook (`rtk hook copilot`) detecta automaticamente o formato:
- **VS Code Copilot Chat**: reescrita transparente via `updatedInput` (igual ao Claude Code)
- **Copilot CLI**: deny-with-suggestion (a CLI ainda não suporta `updatedInput` — veja [copilot-cli#2013](https://github.com/github/copilot-cli/issues/2013))

### Cursor

```bash
rtk init -g --agent cursor
```

Cria `~/.cursor/hooks/rtk-rewrite.sh` + faz patch em `~/.cursor/hooks.json` com matcher preToolUse. Funciona no editor Cursor e no CLI `cursor-agent`.

### Gemini CLI

```bash
rtk init -g --gemini
rtk init -g --gemini --uninstall
```

Cria `~/.gemini/hooks/rtk-hook-gemini.sh` + patch em `~/.gemini/settings.json` com hook BeforeTool.

### Codex (OpenAI)

```bash
rtk init -g --codex
```

Cria `~/.codex/RTK.md` + `~/.codex/AGENTS.md` com referência `@RTK.md`. Codex lê esses arquivos como instruções globais.

### Windsurf

```bash
rtk init --agent windsurf
```

Cria `.windsurfrules` no projeto atual. O Cascade lê as regras e prefixa comandos com `rtk`.

### Cline / Roo Code

```bash
rtk init --agent cline
```

Cria `.clinerules` no projeto atual. Cline lê as regras e prefixa comandos com `rtk`.

### OpenCode

```bash
rtk init -g --opencode
```

Cria `~/.config/opencode/plugins/rtk.ts`. Usa hook `tool.execute.before`.

### OpenClaw

```bash
openclaw plugins install ./openclaw
```

Plugin no diretório `openclaw/`. Usa hook `before_tool_call`, delegando para `rtk rewrite`.

### Mistral Vibe (planejado)

Bloqueado por suporte upstream para BeforeToolCallback ([mistral-vibe#531](https://github.com/mistralai/mistral-vibe/issues/531), [PR #533](https://github.com/mistralai/mistral-vibe/pull/533)). Acompanhado em [#800](https://github.com/rtk-ai/rtk/issues/800).

### Comandos Reescritos

| Comando bruto | Reescrito para |
|-------------|-------------|
| `git status/diff/log/add/commit/push/pull` | `rtk git ...` |
| `gh pr/issue/run` | `rtk gh ...` |
| `cargo test/build/clippy` | `rtk cargo ...` |
| `cat/head/tail <file>` | `rtk read <file>` |
| `rg/grep <pattern>` | `rtk grep <pattern>` |
| `ls` | `rtk ls` |
| `vitest/jest` | `rtk vitest run` |
| `tsc` | `rtk tsc` |
| `eslint/biome` | `rtk lint` |
| `prettier` | `rtk prettier` |
| `playwright` | `rtk playwright` |
| `prisma` | `rtk prisma` |
| `ruff check/format` | `rtk ruff ...` |
| `pytest` | `rtk pytest` |
| `pip list/install` | `rtk pip ...` |
| `go test/build/vet` | `rtk go ...` |
| `golangci-lint` | `rtk golangci-lint` |
| `rake test` / `rails test` | `rtk rake test` |
| `rspec` / `bundle exec rspec` | `rtk rspec` |
| `rubocop` / `bundle exec rubocop` | `rtk rubocop` |
| `bundle install/update` | `rtk bundle ...` |
| `aws sts/ec2/lambda/...` | `rtk aws ...` |
| `docker ps/images/logs` | `rtk docker ...` |
| `kubectl get/logs` | `rtk kubectl ...` |
| `curl` | `rtk curl` |
| `pnpm list/outdated` | `rtk pnpm ...` |

Comandos já usando `rtk`, heredocs (`<<`) e comandos não reconhecidos passam sem alteração.

## Configuracao

### Arquivo de Configuracao

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[tracking]
database_path = "/path/to/custom.db"  # padrão: ~/.local/share/rtk/history.db

[hooks]
exclude_commands = ["curl", "playwright"]  # pula reescrita nesses comandos

[tee]
enabled = true          # salva saída bruta em falhas (padrão: true)
mode = "failures"       # "failures", "always", ou "never"
max_files = 20          # limite de rotação
```

### Tee: Recuperacao da Saida Completa

Quando um comando falha, RTK salva a saída completa sem filtro para o LLM ler sem reexecutar:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

### Desinstalar

```bash
rtk init -g --uninstall     # Remove hook, RTK.md e entrada no settings.json
cargo uninstall rtk          # Remove binário
brew uninstall rtk           # Se instalado via Homebrew
```

## Documentacao

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - Corrigir problemas comuns
- **[INSTALL.md](INSTALL.md)** - Guia detalhado de instalação
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - Arquitetura técnica
- **[SECURITY.md](SECURITY.md)** - Política de segurança e processo de revisão de PR
- **[AUDIT_GUIDE.md](docs/AUDIT_GUIDE.md)** - Guia de analytics de economia de tokens

## Privacidade e Telemetria

RTK coleta **métricas de uso anônimas e agregadas** uma vez por dia, **ativado por padrão**. Isso ajuda a priorizar o desenvolvimento. Veja abaixo opções de opt-out.

**O que é coletado:**
- Hash do dispositivo (SHA-256 com salt aleatório por usuário armazenado localmente, não reversível)
- Versão do RTK, SO, arquitetura
- Contagem de comandos (últimas 24h) e nomes de comandos mais usados (ex.: "git", "cargo" — sem argumentos, sem caminhos)
- Percentual de economia de tokens

**O que NÃO é coletado:** código-fonte, caminhos de arquivos, argumentos de comando, segredos, variáveis de ambiente ou qualquer informação pessoalmente identificável.

**Opt-out** (qualquer uma destas opções):
```bash
# Variável de ambiente
export RTK_TELEMETRY_DISABLED=1

# Ou no arquivo de configuração (~/.config/rtk/config.toml)
[telemetry]
enabled = false
```

## Contribuindo

Contribuições são bem-vindas! Abra uma issue ou PR no [GitHub](https://github.com/rtk-ai/rtk).

Entre na comunidade no [Discord](https://discord.gg/RySmvNF5kF).

## Licenca

Licença MIT - veja [LICENSE](LICENSE) para detalhes.

## Aviso Legal

Veja [DISCLAIMER.md](DISCLAIMER.md).
