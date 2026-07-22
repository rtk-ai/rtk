<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>LLM 토큰 소비를 60-90% 줄이는 고성능 CLI 프록시</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">웹사이트</a> &bull;
  <a href="#설치">설치</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">문제 해결</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">아키텍처</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">디스코드</a>
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

RTK는 명령어 출력이 LLM 컨텍스트에 전달되기 전에 필터링하고 압축합니다. 단일 Rust 바이너리로 동작하며, 100개 이상의 명령어를 지원하고 10ms 미만의 오버헤드를 제공합니다.

## 토큰 절약 (30분 Claude Code 세션)

| 작업 | 빈도 | 표준 | rtk | 절약 |
|------|------|------|-----|------|
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
| **합계** | | **~118,000** | **~23,900** | **-80%** |

> 중간 규모의 TypeScript/Rust 프로젝트를 기준으로 추정한 수치입니다. 실제 절감 효과는 프로젝트 규모에 따라 달라질 수 있습니다.

## 설치

### Homebrew (권장)

```bash
brew install rtk
```

### 빠른 설치 (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> `~/.local/bin`에 설치됩니다. 필요할 경우 PATH에 추가하세요:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # or ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 사전 빌드된 바이너리

[Releases](https://github.com/rtk-ai/rtk/releases) 페이지에서 다운로드하세요.

- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows 사용자:** ZIP 파일을 압축 해제한 뒤 `rtk.exe`를 PATH에 포함된 위치(예: `C:\Users\<사용자명>\.local\bin`)에 저장하세요.
>
> RTK는 **명령 프롬프트(Command Prompt)**, **PowerShell**, 또는 **Windows Terminal**에서 실행해야 합니다. `.exe` 파일을 더블 클릭하면 잠깐 실행된 후 바로 종료됩니다.
>
> 전체 Hook 시스템은 Windows에서 기본적으로 지원되며, [WSL](https://learn.microsoft.com/en-us/windows/wsl/install)에서도 사용할 수 있습니다. 자세한 내용은 아래의 [Windows 설정](#windows) 섹션을 참고하세요.

### 설치 확인

```bash
rtk --version   # "rtk 0.28.2" 표시되어야 함
rtk gain        # 토큰 절약 통계 표시되어야 함
```
> **주의:** crates.io에는 **"rtk" (Rust Type Kit)** 라는 다른 프로젝트도 존재합니다. `rtk gain` 명령이 실행되지 않는다면 잘못된 패키지를 설치한 것입니다. 위의 `cargo install --git` 명령을 사용하여 설치하세요.

## 빠른 시작

```bash
# 1. AI에 설치하기
rtk init -g                     # Claude Code / Copilot (default)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init --agent kimi           # Kimi AI
rtk init -g --agent pi          # Pi
rtk init --agent hermes         # Hermes
rtk init -g --agent droid       # Factory Droid

# 2. AI 재시작 후 테스트
git status  # 자동으로 rtk git status로 재작성
```

Hook 기반 에이전트는 Bash 명령을 실행하기 전에 자동으로 변환합니다. 예를 들어 `git status`는 `rtk git status`로 변경됩니다. Hermes를 포함한 플러그인 기반 에이전트는 플러그인 API를 통해 명령을 실행 전에 변환합니다. 따라서 에이전트는 `rtk`를 직접 호출하지 않아도 RTK의 간결한 출력 결과를 받을 수 있습니다.

**중요:** Hook은 **Bash 도구 호출**에서만 동작합니다. Claude Code의 기본 도구인 `Read`, `Grep`, `Glob`은 Bash Hook을 거치지 않으므로 자동으로 변환되지 않습니다. 이러한 작업에서도 RTK의 간결한 출력을 사용하려면 셸 명령(`cat`/`head`/`tail`, `rg`/`grep`, `find`)을 사용하거나 `rtk read`, `rtk grep`, `rtk find`를 직접 실행하세요.

## 작동 원리

```
  rtk 없이:                                        rtk 사용:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens (원본)        |             |   ~200 tokens        | 필터     |
    +-----------------------------------+             +------- (필터링) -----+----------+
```

네 가지 전략:

1. **스마트 필터링** - 노이즈 제거 (주석, 공백, 보일러플레이트)
2. **그룹화** - 유사 항목 집계 (디렉토리별 파일, 유형별 에러)
3. **잘라내기** - 관련 컨텍스트 유지, 중복 제거
4. **중복 제거** - 반복 로그 라인을 카운트와 함께 통합

## 명령어

### 파일
```bash
rtk ls .                        # 토큰 최적화된 디렉터리 트리
rtk read file.rs                # 스마트 파일 읽기
rtk read file.rs -l aggressive  # 시그니처만 표시 (본문 제거)
rtk smart file.rs               # 2줄 요약 코드 분석
rtk find "*.rs" .               # 간결한 파일 검색 결과
rtk grep "pattern" .            # 그룹화된 검색 결과
rtk diff file1 file2            # 요약된 diff (파일이 다르면 종료 코드 1)
```

### Git
```bash
rtk git status                  # 간결한 상태 표시
rtk git log -n 10               # 한 줄 커밋 기록
rtk git diff                    # 요약된 diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3개 파일 +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # 간결한 PR 목록
rtk gh pr view 42               # PR 상세 정보 및 체크 상태
rtk gh issue list               # 간결한 이슈 목록
rtk gh run list                 # 워크플로 실행 상태
```

### 테스트 러너

```bash
rtk jest                        # Jest 요약 출력 (실패만 표시)
rtk vitest                      # Vitest 요약 출력 (실패만 표시)
rtk playwright test             # E2E 테스트 결과 (실패만 표시)
rtk pytest                      # Python 테스트 (-90%)
rtk go test                     # Go 테스트 (NDJSON, -90%)
rtk cargo test                  # Cargo 테스트 (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # RSpec 테스트 (JSON, -60%+)
rtk err <cmd>                   # 모든 명령의 오류만 필터링
rtk test <cmd>                  # 범용 테스트 래퍼 (실패만 표시, -90%)
```

### 빌드 & 린트

```bash
rtk lint                        # ESLint 규칙/파일별 그룹화 출력
rtk lint biome                  # 다른 린터 지원
rtk tsc                         # TypeScript 오류를 파일별로 그룹화
rtk next build                  # Next.js 빌드 결과 요약 출력
rtk prettier --check .          # 포맷팅이 필요한 파일 표시
rtk cargo build                 # Cargo 빌드 (-80%)
rtk cargo clippy                # Cargo clippy 검사 (-80%)
rtk ruff check                  # Python 린팅 (JSON, -80%)
rtk golangci-lint run           # Go 린팅 (JSON, -85%)
rtk rubocop                     # Ruby 린팅 (JSON, -60%+)
rtk sbt test                    # ScalaTest 출력 (-90%)
rtk sbt compile                 # 컴파일 오류만 표시 (-75%)
rtk sbt run                     # SBT 시작 메시지 노이즈 제거
```

### 패키지 관리자
```bash
rtk pnpm list                   # 간결한 의존성 트리
rtk uv run pytest               # uv 환경 유지, 오류만 표시
rtk pip list                    # Python 패키지 목록 (uv 자동 감지)
rtk pip outdated                # 오래된 패키지 확인
rtk bundle install              # Ruby gem 설치 (Using 라인 제거)
rtk prisma generate             # 스키마 생성 (ASCII 아트 제거)
```

### AWS

```bash
rtk aws sts get-caller-identity # 한 줄로 표시되는 사용자 인증 정보
rtk aws ec2 describe-instances  # 간결한 인스턴스 목록
rtk aws lambda list-functions   # 이름/런타임/메모리 표시 (비밀 정보 제거)
rtk aws logs get-log-events     # 타임스탬프가 포함된 메시지만 표시
rtk aws cloudformation describe-stack-events  # 실패 항목 우선 표시
rtk aws dynamodb scan           # 타입 어노테이션 제거 및 정리
rtk aws iam list-roles          # 정책 문서 제거
rtk aws s3 ls                   # 잘린 출력 제공 (tee로 복구 가능)
```

### 컨테이너

```bash
rtk docker ps                   # 간결한 컨테이너 목록
rtk docker images               # 간결한 이미지 목록
rtk docker logs <container>     # 중복 제거된 로그
rtk docker compose ps           # Compose 서비스 목록
rtk kubectl pods                # 간결한 Pod 목록
rtk kubectl logs <pod>          # 중복 제거된 로그
rtk kubectl services            # 간결한 서비스 목록
rtk oc get pods                 # OpenShift Pod 요약
rtk oc get services             # OpenShift 서비스 목록
rtk oc logs <pod>               # 중복 제거된 로그
```

### Infrastructure as Code (IaC)

```bash
rtk pulumi preview              # 헤더/URL/소요 시간 등의 불필요한 정보 제거
rtk pulumi up                   # 간결한 적용 결과 출력
rtk pulumi destroy              # 간결한 삭제 결과 출력
rtk pulumi refresh              # 변경 사항(Drift) 요약
rtk pulumi stack                # 스택 메타데이터 표시 (소유자/타임스탬프 제거)
```

### 데이터 및 분석

```bash
rtk json config.json            # 값을 제외한 구조 표시
rtk deps                        # 의존성 요약
rtk env -f AWS                  # 필터링된 환경 변수
rtk log app.log                 # 중복 제거된 로그
rtk curl <url>                  # 출력 축약 + 전체 결과 저장
rtk wget <url>                  # 다운로드 진행 표시줄 제거
rtk summary <long command>      # 휴리스틱 기반 요약
rtk proxy <command>             # 원본 출력 전달 + 추적
```

### 토큰 절감 분석

```bash
rtk gain                        # 요약 통계
rtk gain --graph                # ASCII 그래프 (최근 30일)
rtk gain --history              # 최근 명령 기록
rtk gain --daily                # 일별 분석
rtk gain --all --format json    # 대시보드용 JSON 내보내기

rtk discover                    # 놓친 절감 기회 탐색
rtk discover --all --since 7    # 모든 프로젝트, 최근 7일 기준

rtk session                     # 최근 세션에서 RTK 사용 현황 표시
```

## 글로벌 플래그

```bash
-u, --ultra-compact    # ASCII 아이콘, 인라인 형식 (추가 토큰 절감)
-v, --verbose          # 출력 상세 수준 증가 (-v, -vv, -vvv)
```

## 예시

**디렉토리 목록**
```
# ls -la (45 lines, ~800 tokens)        # rtk ls (12 lines, ~150 tokens)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git작업**
```
# git push (15 lines, ~200 tokens)       # rtk git push (1 line, ~10 tokens)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
```

**테스트 결과:**
```
# cargo test (200+ lines on failure)     # rtk test cargo test (~20 lines)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 자동 변환 Hook

rtk를 사용하는 가장 효과적인 방법입니다. 이 Hook은 Bash 명령을 가로채고, 실행 전에 해당 명령을 rtk 명령으로 자동 변환합니다.

**결과:** 모든 대화와 서브에이전트에서 100% rtk 사용 적용, 토큰 오버헤드 없음.

**범위 참고:** 이 기능은 Bash 도구 호출에만 적용됩니다. Claude Code의 기본 도구인 `Read`, `Grep`, `Glob` 등은 Hook을 우회하므로 자동 변환되지 않습니다. 해당 작업에서도 RTK 필터링을 사용하려면 셸 명령 또는 명시적인 `rtk` 명령을 사용하세요.

### 설정

```bash
rtk init -g                 # Hook + RTK.md 설치 (권장)
rtk init -g --opencode      # OpenCode 플러그인 설치 (Claude Code 대신 사용)
rtk init -g --auto-patch    # 비대화형 설치 (CI/CD용)
rtk init -g --hook-only     # Hook만 설치, RTK.md 제외
rtk init --show             # 설치 상태 확인
```

## Windows

RTK는 윈도우 환경에서도 완벽하게 작동합니다. **v0.37.2**부터 자동 명령어 변환 Hook은 **네이티브 바이너리 명령어**(`rtk hook claude`)로 실행됩니다. 따라서 Unix 셸, bash, 또는 jq가 필요하지 않습니다.

Command Prompt, PowerShell, Windows Terminal에서도 Linux 및 macOS와 동일하게 명령어가 투명하게 자동 변환됩니다.

### Native Windows

```powershell
# 1. Releases에서 rtk-x86_64-pc-windows-msvc.zip 다운로드 및 압축 해제
# 2. rtk.exe를 PATH에 추가 (예: C:\Users\<사용자명>\.local\bin)
# 3. Initialize — 네이티브 바이너리 Hook 설치
rtk init -g
```

**기존 설치 버전에서 업그레이드하는 경우:**  
v0.37.2 이전에 RTK를 설정했다면 Unix 셸이 필요한 기존 `rtk-rewrite.sh` 셸 Hook이 남아 있을 수 있습니다. `rtk init -g`를 다시 실행하여 네이티브 바이너리 Hook으로 마이그레이션하세요.

**필수 조건:** 일부 필터는 [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`)를 사용하여 셸 명령을 실행합니다. `rg`를 설치하고 PATH에 추가해 두세요 (예: `winget install BurntSushi.ripgrep.MSVC`). 이렇게 하면 `Binary 'rg' not found on PATH` 경고를 방지할 수 있습니다.

**중요:** `rtk.exe`를 더블 클릭하지 마세요. 해당 프로그램은 사용법을 출력한 뒤 즉시 종료되는 CLI 도구입니다. 항상 터미널(Command Prompt, PowerShell, 또는 Windows Terminal)에서 실행하세요.

### WSL

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install)에서도 사용할 수 있으며, Linux와 동일하게 동작합니다.

```bash
# WSL 내부에서 실행
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

| 기능 | Native Windows | WSL |
|---------|----------------|-----|
| 필터 (cargo, git 등) | 전체 지원 | 전체 지원 |
| 자동 명령어 변환 Hook | 지원 (네이티브 바이너리) | 지원 |
| `rtk init -g` | Hook 모드 | Hook 모드 |
| `rtk gain` / 분석 기능 | 전체 지원 | 전체 지원 |

## 지원가능 AI

RTK는 15개의 코딩 AI를 지원합니다. 각 통합 기능은 명령어 가로채기를 지원하는 에이전트 환경에서 셸 명령을 `rtk` 명령으로 변환하여 60~90%의 토큰 절감을 제공합니다.

| Tool | Install | Method |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook (native binary) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook — transparent rewrite |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion (CLI limitation) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md instructions |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules (project-scoped) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (project-scoped) |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS (tool.execute.before) |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS (before_tool_call) |
| **Pi** | `rtk init -g --agent pi` (global) | TypeScript extension (tool_call) |
| **Hermes** | `rtk init --agent hermes` | Python plugin adapter (terminal command mutation via `rtk rewrite`) |
| **Mistral Vibe** | Planned ([#800](https://github.com/rtk-ai/rtk/issues/800)) | Blocked on upstream |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md (project-scoped) |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md (project-scoped) |
| **Kimi AI** | `rtk init --agent kimi` | AGENTS.md (project-scoped) |
| **Factory Droid** | `rtk init -g --agent droid` (or per-project) | PreToolUse hook in `~/.factory/hooks.json` (matcher `Execute`) |

각 에이전트별 설정 방법, 재정의 옵션, 그리고 안전한 대체 동작(graceful degradation)에 대한 자세한 내용은 [지원되는 에이전트 가이드](https://www.rtk-ai.app/guide/getting-started/supported-agents)를 참고하세요. Hermes 플러그인의 소스 코드와 테스트는 `hooks/hermes/`에 있으며, 설치된 Hermes 런타임 파일은 여전히 `~/.hermes/plugins/rtk-rewrite/`에 위치합니다.

## 설정

`~/.config/rtk/config.toml` (맥OS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # skip rewrite for these

[tee]
enabled = true          # save raw output on failure (default: true)
mode = "failures"       # "failures", "always", or "never"
```

명령이 실패하면 RTK는 필터링되지 않은 전체 출력을 저장하므로, LLM은 명령을 다시 실행하지 않고도 해당 출력을 확인할 수 있습니다:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

전체 설정 참조(모든 섹션, 환경 변수, 프로젝트별 필터 설정)는 [Configuration guide](https://www.rtk-ai.app/guide/getting-started/configuration)를 참고하세요.

### 제거하기

```bash
rtk init -g --uninstall      # hook, RTK.md, settings.json 제거
cargo uninstall rtk          # 바이너리 제거
brew uninstall rtk           # Homebrew로 다운로드 받은 경우
```

## 문서

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 전체 사용자 가이드 (설치, 지원 에이전트, 최적화 대상, 분석 기능, 설정, 문제 해결)
- **[INSTALL.md](INSTALL.md)** — 상세 설치 참고 문서
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** — 시스템 설계 및 기술적 결정 사항
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 기여 가이드
- **[SECURITY.md](SECURITY.md)** — 보안 정책

## 개인정보 보호 및 텔레메트리

RTK는 하루에 한 번 **익명화된 집계 사용량 지표**를 수집할 수 있습니다. 텔레메트리 기능은 **기본적으로 비활성화**되어 있으며, `rtk init` 실행 중 또는 `rtk telemetry enable` 명령을 통해 **명시적인 사용자 동의**(GDPR 제6조, 제7조)가 필요합니다.

이 데이터는 더 나은 제품을 개발하는 데 사용됩니다. 예를 들어 어떤 명령에 필터가 필요한지, 어떤 필터를 개선해야 하는지, 그리고 RTK가 얼마나 많은 가치를 제공하는지를 파악하는 데 도움을 줍니다.

수집되는 전체 필드 목록, 데이터 처리 방식, 기여자 가이드라인은 [docs/TELEMETRY.md](./docs/TELEMETRY.md)를 참고하세요.

**수집되는 데이터와 목적:**

| 카테고리 | 데이터 | 목적 |
|----------|------|-----|
| 식별 정보 | 솔트 처리된 디바이스 해시 (SHA-256, 역추적 불가) | 개인을 추적하지 않고 고유 설치 수 파악 |
| 환경 정보 | RTK 버전, OS, 아키텍처, 설치 방법 | 지원 및 테스트가 필요한 플랫폼 파악 |
| 사용량 | 명령어 수 (24시간), 전체 명령 수, 절약된 토큰 수 (24시간/30일/전체) | 사용률과 제공되는 가치 측정 |
| 품질 | 상위 5개 패스스루 명령 (절감률 0%), 파싱 실패 횟수, 절감률 30% 미만인 명령 | 부족한 필터와 개선이 필요한 필터 식별 |
| 생태계 | 명령어 카테고리 분포 (예: git 45%, cargo 20%, js 15%) | 인기 있는 생태계에 맞춰 필터 개발 우선순위 결정 |
| 유지율 | 최초 사용 후 경과 일수, 최근 30일 동안 활성 사용 일수 | 사용자 참여도 파악 및 이탈 감지 |
| 도입 현황 | AI 에이전트 Hook 유형 (claude/gemini/codex), 사용자 정의 TOML 필터 개수 | 통합 지원 범위와 DSL 사용 현황 추적 |
| 설정 | config.toml 존재 여부, 제외된 명령 수, 프로젝트 수 | 사용자 설정 수준과 커스터마이징 패턴 파악 |
| 기능 | 메타 명령어(gain, discover, proxy, verify) 사용 횟수 | 어떤 RTK 기능이 가치 있게 사용되는지 파악 |
| 경제적 가치 | 예상 USD 절감액 (API 토큰 가격 기준) | RTK가 사용자에게 제공하는 가치 정량화 |

모든 데이터는 **집계된 수치 또는 익명화된 명령어 이름**입니다 (처음 3개 단어만 수집하며, 인자는 포함하지 않음). 상위 명령어는 도구 이름만 보고합니다 (예: `"git"`, `"cargo"`). 전체 명령어 라인은 절대 수집하지 않습니다.

**수집하지 않는 정보:** 소스 코드, 파일 경로, 명령어 인자, 비밀 정보, 환경 변수, 개인 데이터, 저장소 내용

**텔레메트리 관리:**
```bash
rtk telemetry status     # 현재 동의 상태 확인
rtk telemetry enable     # 동의 허용 (대화형 안내)
rtk telemetry disable    # 동의 철회 — 모든 데이터 수집 즉시 중단
rtk telemetry forget     # 동의 철회 + 로컬 데이터 삭제 + 서버 측 데이터 삭제 요청
```

**환경 변수로 비활성화:**
```bash
export RTK_TELEMETRY_DISABLED=1   # 동의 여부와 관계없이 텔레메트리 차단
```

## Star History

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

## 핵심 멤버

- **Patrick Szymkowiak** — Founder
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — Core contributor
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — Core contributor
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)
- **Nicolas Le Cam** — Core contributor
  [Github](https://github.com/kush) · [LinkedIn](https://www.linkedin.com/in/nicolas-le-cam-386387160/)
- **Takayuki Maeda** — Core contributor
  [GitHub](https://github.com/TaKO8Ki) · [LinkedIn](https://www.linkedin.com/in/tako8ki/)

## 기여하기
기여를 환영합니다! [GitHub](https://github.com/rtk-ai/rtk)에서 Issue 또는 PR을 등록해 주세요.

[Discord](https://discord.gg/RySmvNF5kF)에서 커뮤니티에 참여할 수 있습니다.

## 라이선스

Apache License 2.0 — 자세한 내용은 [LICENSE](LICENSE)를 참고하세요.

## 면책 조항

자세한 내용은 [DISCLAIMER.md](DISCLAIMER.md)를 참고하세요.