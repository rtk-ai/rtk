<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>LLM 토큰 소비를 60-90% 줄이는 고성능 CLI 프록시</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">웹사이트</a> &bull;
  <a href="#설치">설치</a> &bull;
  <a href="https://www.rtk-ai.app/guide/troubleshooting">문제 해결</a> &bull;
  <a href="ARCHITECTURE.md">아키텍처</a> &bull;
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

rtk는 명령 출력이 LLM 컨텍스트에 도달하기 전에 필터링하고 압축합니다. 단일 Rust 바이너리, 100개 이상의 명령어 지원, 10ms 미만의 오버헤드.

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

> 중간 규모의 TypeScript/Rust 프로젝트 기준 추정치입니다. 실제 절약량은 프로젝트 규모에 따라 달라집니다.

## 설치

### Homebrew (권장)

```bash
brew install rtk
```

### 빠른 설치 (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

> `~/.local/bin`에 설치됩니다. 필요 시 PATH에 추가하세요:
> ```bash
> echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc  # 또는 ~/.zshrc
> ```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 사전 빌드 바이너리

[릴리스](https://github.com/rtk-ai/rtk/releases)에서 다운로드:
- macOS: `rtk-x86_64-apple-darwin.tar.gz` / `rtk-aarch64-apple-darwin.tar.gz`
- Linux: `rtk-x86_64-unknown-linux-musl.tar.gz` / `rtk-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `rtk-x86_64-pc-windows-msvc.zip`

> **Windows 사용자**: zip을 압축 해제한 뒤 `rtk.exe`를 PATH에 포함된 경로(예: `C:\Users\<사용자>\.local\bin`)에 두세요. **명령 프롬프트**, **PowerShell**, 또는 **Windows Terminal**에서 실행해야 합니다 — `.exe`를 더블 클릭하지 마세요 (순간적으로 떴다가 닫힙니다). 완전한 기능을 원한다면 [WSL](https://learn.microsoft.com/ko-kr/windows/wsl/install) 사용을 권장합니다 — 훅 시스템이 네이티브로 동작합니다. 자세한 내용은 아래 [Windows 설정](#windows) 섹션을 참조하세요.

### 설치 확인

```bash
rtk --version   # "rtk 0.28.2" 표시되어야 함
rtk gain        # 토큰 절약 통계 표시되어야 함
```

> **이름 충돌 경고**: crates.io에 "rtk"라는 이름의 다른 프로젝트(Rust Type Kit)가 존재합니다. `rtk gain`이 실패하면 잘못된 패키지를 설치한 것입니다. 위의 `cargo install --git` 명령을 대신 사용하세요.

## 빠른 시작

```bash
# 1. 사용하는 AI 도구에 맞게 설치
rtk init -g                     # Claude Code / Copilot (기본)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init --agent windsurf       # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity

# 2. AI 도구를 재시작한 후 테스트
git status  # 자동으로 rtk git status로 재작성됨
```

훅이 실행 전에 Bash 명령을 투명하게 재작성합니다 (예: `git status` -> `rtk git status`). Claude는 재작성을 인지하지 못하며, 압축된 출력만 받습니다.

**중요:** 훅은 Bash 도구 호출에서만 동작합니다. Claude Code 내장 도구인 `Read`, `Grep`, `Glob`은 Bash 훅을 거치지 않으므로 자동 재작성되지 않습니다. 해당 워크플로에서도 RTK의 압축 출력을 얻으려면 셸 명령(`cat`/`head`/`tail`, `rg`/`grep`, `find`)을 사용하거나 `rtk read`, `rtk grep`, `rtk find`를 직접 호출하세요.

## 작동 원리

```
  rtk 없이:                                        rtk 사용:

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  RTK  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens (원본)        |             |   ~200 tokens        | 필터     |
    +-----------------------------------+             +------- (필터링) -----+----------+
```

명령 유형별로 네 가지 전략이 적용됩니다:

1. **스마트 필터링** - 노이즈 제거 (주석, 공백, 보일러플레이트)
2. **그룹화** - 유사 항목 집계 (디렉토리별 파일, 유형별 에러)
3. **잘라내기** - 관련 컨텍스트 유지, 중복 삭제
4. **중복 제거** - 반복 로그 라인을 카운트와 함께 통합

## 명령어

### 파일
```bash
rtk ls .                        # 토큰 최적화된 디렉토리 트리
rtk read file.rs                # 스마트 파일 읽기
rtk read file.rs -l aggressive  # 시그니처만 (본문 제거)
rtk smart file.rs               # 2줄 휴리스틱 코드 요약
rtk find "*.rs" .               # 컴팩트한 find 결과
rtk grep "pattern" .            # 그룹화된 검색 결과
rtk diff file1 file2            # 압축된 diff
```

### Git
```bash
rtk git status                  # 컴팩트 상태
rtk git log -n 10               # 한 줄 커밋
rtk git diff                    # 압축된 diff
rtk git add                     # -> "ok"
rtk git commit -m "msg"         # -> "ok abc1234"
rtk git push                    # -> "ok main"
rtk git pull                    # -> "ok 3 files +10 -2"
```

### GitHub CLI
```bash
rtk gh pr list                  # 컴팩트 PR 목록
rtk gh pr view 42               # PR 상세 + 체크
rtk gh issue list               # 컴팩트 이슈 목록
rtk gh run list                 # 워크플로 실행 상태
```

### 테스트 러너
```bash
rtk jest                        # Jest 컴팩트 (실패만)
rtk vitest                      # Vitest 컴팩트 (실패만)
rtk playwright test             # E2E 결과 (실패만)
rtk pytest                      # Python 테스트 (-90%)
rtk go test                     # Go 테스트 (NDJSON, -90%)
rtk cargo test                  # Cargo 테스트 (-90%)
rtk rake test                   # Ruby minitest (-90%)
rtk rspec                       # RSpec 테스트 (JSON, -60%+)
rtk err <cmd>                   # 어떤 명령이든 에러만 필터링
rtk test <cmd>                  # 제네릭 테스트 래퍼 - 실패만 (-90%)
```

### 빌드 & 린트
```bash
rtk lint                        # ESLint 규칙/파일별 그룹화
rtk lint biome                  # 다른 린터 지원
rtk tsc                         # TypeScript 에러 파일별 그룹화
rtk next build                  # Next.js 빌드 컴팩트
rtk prettier --check .          # 포매팅 필요한 파일
rtk cargo build                 # Cargo 빌드 (-80%)
rtk cargo clippy                # Cargo clippy (-80%)
rtk ruff check                  # Python 린트 (JSON, -80%)
rtk golangci-lint run           # Go 린트 (JSON, -85%)
rtk rubocop                     # Ruby 린트 (JSON, -60%+)
```

### 패키지 매니저
```bash
rtk pnpm list                   # 컴팩트 의존성 트리
rtk pip list                    # Python 패키지 (uv 자동 감지)
rtk pip outdated                # 오래된 패키지
rtk bundle install              # Ruby gems (Using 라인 제거)
rtk prisma generate             # 스키마 생성 (ASCII 아트 없음)
```

### AWS
```bash
rtk aws sts get-caller-identity # 한 줄 신원 정보
rtk aws ec2 describe-instances  # 컴팩트 인스턴스 목록
rtk aws lambda list-functions   # 이름/런타임/메모리 (시크릿 제거)
rtk aws logs get-log-events     # 타임스탬프 메시지만
rtk aws cloudformation describe-stack-events  # 실패 우선
rtk aws dynamodb scan           # 타입 어노테이션 해제
rtk aws iam list-roles          # 정책 문서 제거
rtk aws s3 ls                   # 잘라내기 + tee 복구
```

### 컨테이너
```bash
rtk docker ps                   # 컴팩트 컨테이너 목록
rtk docker images               # 컴팩트 이미지 목록
rtk docker logs <container>     # 중복 제거된 로그
rtk docker compose ps           # Compose 서비스
rtk kubectl pods                # 컴팩트 파드 목록
rtk kubectl logs <pod>          # 중복 제거된 로그
rtk kubectl services            # 컴팩트 서비스 목록
```

### 데이터 & 분석
```bash
rtk json config.json            # 값 없는 구조만
rtk deps                        # 의존성 요약
rtk env -f AWS                  # 필터링된 환경 변수
rtk log app.log                 # 중복 제거된 로그
rtk curl <url>                  # 잘라내기 + 전체 출력 저장
rtk wget <url>                  # 다운로드, 진행 바 제거
rtk summary <long command>      # 휴리스틱 요약
rtk proxy <command>             # 원시 패스스루 + 추적
```

### 토큰 절약 분석
```bash
rtk gain                        # 요약 통계
rtk gain --graph                # ASCII 그래프 (최근 30일)
rtk gain --history              # 최근 명령 이력
rtk gain --daily                # 일별 분석
rtk gain --all --format json    # 대시보드용 JSON 내보내기

rtk discover                    # 놓친 절약 기회 발견
rtk discover --all --since 7    # 모든 프로젝트, 최근 7일

rtk session                     # 최근 세션에서 RTK 채택률 표시
```

## 전역 플래그

```bash
-u, --ultra-compact    # ASCII 아이콘, 인라인 포맷 (추가 토큰 절약)
-v, --verbose          # 상세도 증가 (-v, -vv, -vvv)
```

## 예시

**디렉토리 목록:**
```
# ls -la (45줄, ~800 토큰)               # rtk ls (12줄, ~150 토큰)
drwxr-xr-x  15 user staff 480 ...       my-project/
-rw-r--r--   1 user staff 1234 ...       +-- src/ (8 files)
...                                      |   +-- main.rs
                                         +-- Cargo.toml
```

**Git 작업:**
```
# git push (15줄, ~200 토큰)             # rtk git push (1줄, ~10 토큰)
Enumerating objects: 5, done.             ok main
Counting objects: 100% (5/5), done.
Delta compression using up to 8 threads
...
```

**테스트 출력:**
```
# cargo test (실패 시 200+ 줄)           # rtk test cargo test (~20줄)
running 15 tests                          FAILED: 2/15 tests
test utils::test_parse ... ok               test_edge_case: assertion failed
test utils::test_format ... ok              test_overflow: panic at utils.rs:18
...
```

## 자동 재작성 훅

rtk를 가장 효과적으로 사용하는 방법입니다. 훅은 Bash 명령을 투명하게 가로채어 실행 전에 rtk 상응물로 재작성합니다.

**결과**: 모든 대화와 서브에이전트에서 100% rtk 채택, 토큰 오버헤드 제로.

**적용 범위 안내:** Bash 도구 호출에만 적용됩니다. `Read`, `Grep`, `Glob` 같은 Claude Code 내장 도구는 훅을 우회하므로, 해당 경로에서도 RTK 필터링을 원하면 셸 명령이나 명시적 `rtk` 명령을 사용하세요.

### 설정

```bash
rtk init -g                 # 훅 + RTK.md 설치 (권장)
rtk init -g --opencode      # OpenCode 플러그인 (Claude Code 대신)
rtk init -g --auto-patch    # 비대화형 (CI/CD)
rtk init -g --hook-only     # 훅만, RTK.md 없음
rtk init --show             # 설치 확인
```

설치 후 **Claude Code를 재시작하세요**.

## Windows

RTK는 Windows에서 일부 제한이 있지만 동작합니다. 자동 재작성 훅(`rtk-rewrite.sh`)은 Unix 셸이 필요하므로, 네이티브 Windows에서는 RTK가 **CLAUDE.md 주입 모드**로 폴백합니다 — AI 어시스턴트는 RTK 지시사항을 받지만 명령이 자동 재작성되지는 않습니다.

### 권장: WSL (완전 지원)

최상의 경험을 위해 [WSL](https://learn.microsoft.com/ko-kr/windows/wsl/install) (Windows Subsystem for Linux)을 사용하세요. WSL 내부에서 RTK는 Linux와 완전히 동일하게 동작합니다 — 훅 완전 지원, 자동 재작성, 모든 기능:

```bash
# WSL 내부에서
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g
```

### 네이티브 Windows (제한 지원)

네이티브 Windows (cmd.exe / PowerShell)에서 RTK 필터는 동작하지만 훅의 자동 재작성은 동작하지 않습니다:

```powershell
# 1. 릴리스에서 rtk-x86_64-pc-windows-msvc.zip 다운로드 및 압축 해제
# 2. rtk.exe를 PATH에 추가
# 3. 초기화 (CLAUDE.md 주입으로 폴백됨)
rtk init -g
# 4. rtk를 명시적으로 사용
rtk cargo test
rtk git status
```

**중요**: `rtk.exe`를 더블 클릭하지 마세요 — 사용법만 출력하고 즉시 종료하는 CLI 도구입니다. 항상 터미널(명령 프롬프트, PowerShell, Windows Terminal)에서 실행하세요.

| 기능 | WSL | 네이티브 Windows |
|------|-----|-----------------|
| 필터 (cargo, git 등) | 완전 지원 | 완전 지원 |
| 자동 재작성 훅 | 예 | 아니오 (CLAUDE.md 폴백) |
| `rtk init -g` | 훅 모드 | CLAUDE.md 모드 |
| `rtk gain` / 분석 | 완전 지원 | 완전 지원 |

## 지원되는 AI 도구

RTK는 12개의 AI 코딩 도구를 지원합니다. 각 통합은 셸 명령을 `rtk` 상응물로 투명하게 재작성하여 60-90% 토큰을 절약합니다.

| 도구 | 설치 | 방식 |
|------|------|------|
| **Claude Code** | `rtk init -g` | PreToolUse 훅 (bash) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse 훅 — 투명 재작성 |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion (CLI 제약) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse 훅 (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool 훅 |
| **Codex** | `rtk init -g --codex` | AGENTS.md + RTK.md 지시사항 |
| **Windsurf** | `rtk init --agent windsurf` | .windsurfrules (프로젝트 범위) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (프로젝트 범위) |
| **OpenCode** | `rtk init -g --opencode` | 플러그인 TS (tool.execute.before) |
| **OpenClaw** | `openclaw plugins install ./openclaw` | 플러그인 TS (before_tool_call) |
| **Mistral Vibe** | 예정 ([#800](https://github.com/rtk-ai/rtk/issues/800)) | 업스트림 대기 중 |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md (프로젝트 범위) |
| **Google Antigravity** | `rtk init --agent antigravity` | .agents/rules/antigravity-rtk-rules.md (프로젝트 범위) |

에이전트별 상세 설정, 오버라이드 제어, graceful degradation은 [Supported Agents 가이드](https://www.rtk-ai.app/guide/getting-started/supported-agents)를 참조하세요.

## 설정

`~/.config/rtk/config.toml` (macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[hooks]
exclude_commands = ["curl", "playwright"]  # 이 명령들은 재작성 건너뜀

[tee]
enabled = true          # 실패 시 원본 출력 저장 (기본: true)
mode = "failures"       # "failures", "always", 또는 "never"
```

명령이 실패하면 RTK가 필터링하지 않은 전체 출력을 저장하므로, LLM이 재실행 없이 읽을 수 있습니다:

```
FAILED: 2/15 tests
[full output: ~/.local/share/rtk/tee/1707753600_cargo_test.log]
```

전체 설정 레퍼런스(모든 섹션, 환경 변수, 프로젝트별 필터)는 [Configuration 가이드](https://www.rtk-ai.app/guide/getting-started/configuration)를 참조하세요.

### 제거

```bash
rtk init -g --uninstall     # 훅, RTK.md, settings.json 항목 제거
cargo uninstall rtk          # 바이너리 제거
brew uninstall rtk           # Homebrew 설치 시
```

## 문서

- **[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** — 전체 사용자 가이드 (설치, 지원 에이전트, 최적화 대상, 분석, 설정, 문제 해결)
- **[INSTALL.md](INSTALL.md)** — 상세 설치 레퍼런스
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — 시스템 설계 및 기술적 결정
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — 기여 가이드
- **[SECURITY.md](SECURITY.md)** — 보안 정책

## 개인 정보 및 원격 측정

RTK는 **익명의 집계 사용 지표**를 하루 1회 수집할 수 있습니다. 원격 측정은 **기본 비활성화**되어 있으며, `rtk init` 중 또는 `rtk telemetry enable`을 통한 **명시적 옵트인 동의**(GDPR 제6조, 제7조)가 필요합니다. 이 데이터는 더 나은 제품을 만드는 데 도움이 됩니다: 어떤 명령에 필터가 필요한지, 어떤 필터를 개선해야 하는지, RTK가 얼마나 가치를 전달하는지를 파악합니다. 전체 필드 목록, 데이터 처리 방식, 기여자 가이드라인은 **[docs/TELEMETRY.md](docs/TELEMETRY.md)**를 참조하세요.

**수집 항목과 이유:**

| 카테고리 | 데이터 | 이유 |
|----------|--------|------|
| 식별자 | 솔트 적용 기기 해시 (SHA-256, 역산 불가) | 개인을 추적하지 않고 고유 설치 수 집계 |
| 환경 | RTK 버전, OS, 아키텍처, 설치 방식 | 지원/테스트할 플랫폼 파악 |
| 사용량 | 명령 수(24시간), 전체 명령 수, 절약 토큰(24시간/30일/누적) | 채택도와 전달 가치 측정 |
| 품질 | 상위 5개 패스스루 명령(0% 절약), 파싱 실패 수, <30% 절약 명령 | 누락된/약한 필터 식별 |
| 생태계 | 명령 카테고리 분포 (예: git 45%, cargo 20%, js 15%) | 인기 생태계에 필터 개발 우선순위 배정 |
| 리텐션 | 최초 사용 이후 경과일, 최근 30일 활성일 | 참여도 이해 및 이탈 감지 |
| 채택 | AI 에이전트 훅 유형(claude/gemini/codex), 커스텀 TOML 필터 수 | 통합 범위 및 DSL 채택률 추적 |
| 설정 | config.toml 존재 여부, 제외 명령 수, 프로젝트 수 | 사용자 숙련도 및 커스터마이징 패턴 파악 |
| 기능 | 메타 명령(gain, discover, proxy, verify) 사용 횟수 | 사용자가 가치를 두는 RTK 기능 파악 |
| 경제성 | 추정 USD 절약액 (API 토큰 가격 기반) | RTK가 사용자에게 제공하는 가치 정량화 |

모든 데이터는 **집계 카운트 또는 익명화된 명령 이름**(앞 3단어, 인수 제외)입니다. 상위 명령은 도구 이름(예: "git", "cargo")만 보고하며 전체 명령줄은 절대 보고하지 않습니다.

**수집하지 않는 항목:** 소스 코드, 파일 경로, 명령 인수, 시크릿, 환경 변수, 개인 정보, 저장소 내용.

**원격 측정 관리:**
```bash
rtk telemetry status     # 현재 동의 상태 확인
rtk telemetry enable     # 동의 (대화형 프롬프트)
rtk telemetry disable    # 동의 철회 — 즉시 모든 수집 중단
rtk telemetry forget     # 동의 철회 + 로컬 데이터 삭제 + 서버 측 삭제 요청
```

**환경 변수로 오버라이드:**
```bash
export RTK_TELEMETRY_DISABLED=1   # 동의 여부와 무관하게 원격 측정 차단
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

## 핵심 팀

- **Patrick Szymkowiak** — 창립자
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — 핵심 기여자
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — 핵심 기여자
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)

## 기여

기여를 환영합니다! [GitHub](https://github.com/rtk-ai/rtk)에서 이슈 또는 PR을 생성해 주세요.

[Discord](https://discord.gg/RySmvNF5kF) 커뮤니티에 참여하세요.

## 라이선스

MIT 라이선스 - 자세한 내용은 [LICENSE](LICENSE)를 참조하세요.

## 면책 조항

[DISCLAIMER.md](DISCLAIMER.md)를 참조하세요.

---
위 블록 전체(```markdown ... ```)의 안쪽 내용만 복사해서 README_ko.md 를 통째로 교체하시면 됩니다.

주요 번역 결정사항 (리뷰 시 참고):

- 용어: hook→훅, binary→바이너리, overhead→오버헤드, filter→필터링, deduplication→중복 제거, passthrough→패스스루, opt-in→옵트인, telemetry→원격 측정, fork/push 같은 git 용어는 원문 유지
- 링크 변경 없음: 모든 URL/앵커는 원문 그대로 (내부 앵커 #installation은 기존 ko 컨벤션 따라 #설치로 매핑)
- Windows WSL 가이드 링크: ko-kr 로케일로 교체 (/en-us/ → /ko-kr/)
- 테이블 헤더: Operation→작업, Frequency→빈도, Standard→표준, Savings→절약 등 한국어로
- 어투: 기존 README_ko.md 의 "~합니다/됩니다" 톤 유지, 기술 고유명사·명령어·브랜드명은 원문 그대로
- Windows 섹션: 기존 ko에 아예 없던 섹션 신설 — 영문판의 주의사항(더블클릭 금지, WSL 권장) 모두 포함
- Privacy & Telemetry: 기존 ko에 완전히 누락되어 있었던 핵심 섹션 — 10개 카테고리 표까지 전체 번역

PR 커밋 메시지 제안 (참고용):
docs(README_ko): 영문 최신본(0.28.2) 기준 한국어 문서 동기화

- 추가: Windows 섹션, Privacy & Telemetry 전체, Supported AI Tools 표(12개), Examples,
  Auto-Rewrite Hook 상세, Configuration, Uninstall, Core team, Star History
- 갱신: 버전 0.27.x → 0.28.2, rtk init --global → rtk init -g, 도구별 init 서브커맨드 8종
- 토큰 절약 표 12행 완전 반영, 명령어 커버리지(Files/Git/GitHub CLI/Tests/Build/Package/AWS/Containers/Data) 확장
