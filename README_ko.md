<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>에이전트가 읽는 bash 출력을 최대 90% 줄이는 고성능 CLI 프록시</strong>
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">웹사이트</a> &bull;
  <a href="#설치">설치</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">문제 해결</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">아키텍처</a> &bull;
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

rtk는 명령 출력이 LLM 컨텍스트에 도달하기 전에 필터링하고 압축합니다. 단일 Rust 바이너리, 의존성 없음, 10ms 미만의 오버헤드.

## RTK가 하는 일

RTK는 셸 명령을 가로채 에이전트가 읽기 전에 출력을 압축합니다.

| 작업 | RTK가 출력에 하는 일 |
|------|----------------------|
| `ls` / `tree` | 항목당 한 줄 대신 파일 개수가 포함된 트리 형식 |
| `cat` / `read` | 스마트 파일 읽기: 전체 본문 대신 시그니처와 구조 |
| `grep` / `rg` | 긴 줄을 잘라내고 매치를 파일별로 그룹화 |
| `git status` | 컴팩트한 stat 형식, 상태별 그룹화 |
| `git diff` | 컨텍스트 축소, 헤더 제거 |
| `git log` | 해시, 작성자, 제목만 |
| `git add/commit/push` | 전체 진행 출력 대신 확인 한 줄 |
| `cargo test` / `npm test` | 실패만 표시, 통과한 테스트는 개수로 축약 |
| `ruff check` | 규칙과 파일별로 그룹화 |
| `pytest` | 실패만 표시, 트레이스백 축약 |
| `go test` | NDJSON 파싱, 실패만 표시 |
| `docker ps` | 핵심 필드만 |

## 절약이 계산되는 방식

RTK는 에이전트가 읽는 **bash 출력을 최대 90%** 줄입니다. 이것이 RTK가 측정하는 값이며, 요금이 90% 줄어드는 것과는 다릅니다.

bash 출력은 프롬프트, 시스템 프롬프트, 대화 기록과 함께 **입력 토큰을 구성하는 요소 중 하나**입니다. 그리고 입력 토큰 역시 출력 토큰까지 포함하는 **요금의 일부일 뿐**입니다. 감소 효과는 각 단계에서 희석됩니다.

RTK가 보고하는 토큰 수는 `바이트 / 4`로 추정됩니다. RTK에는 토크나이저가 포함되어 있지 않으므로 **비율은 신뢰할 수 있지만 토큰 절대값은 근사치**입니다.

> 전체 설명: [RTK의 절약이 계산되는 방식](docs/guide/resources/savings-explained.md)

## 설치

### Homebrew (권장)

```bash
brew install rtk
```

### 빠른 설치 (Linux/macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/rtk-ai/rtk
```

### 확인

```bash
rtk --version   # "rtk 0.27.x" 표시되어야 함
rtk gain        # 토큰 절약 통계 표시되어야 함
```

## 빠른 시작

```bash
# 1. Claude Code용 hook 설치 (권장)
rtk init --global

# 2. Claude Code 재시작 후 테스트
git status  # 자동으로 rtk git status로 재작성
```

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

> 아래 백분율은 RTK의 `바이트 / 4` 추정기로 측정한 **bash 출력 바이트 감소율**입니다. [절약이 계산되는 방식](#절약이-계산되는-방식)을 참조하세요.

### 파일
```bash
rtk ls .                        # 최적화된 디렉토리 트리
rtk read file.rs                # 스마트 파일 읽기
rtk find "*.rs" .               # 컴팩트한 검색 결과
rtk grep "pattern" .            # 파일별 그룹화 검색
```

### Git
```bash
rtk git status                  # 컴팩트 상태
rtk git log -n 10               # 한 줄 커밋
rtk git diff                    # 압축된 diff
rtk git push                    # -> "ok main"
```

### 테스트
```bash
rtk jest                        # Jest 컴팩트
rtk vitest                      # Vitest 컴팩트
rtk pytest                      # Python 테스트 (-90%)
rtk go test                     # Go 테스트 (-90%)
rtk test <cmd>                  # 실패만 표시 (-90%)
```

### 빌드 & 린트
```bash
rtk lint                        # ESLint 규칙별 그룹화
rtk tsc                         # TypeScript 에러 그룹화
rtk cargo build                 # Cargo 빌드 (-80%)
rtk ruff check                  # Python 린트 (-80%)
```

### 분석
```bash
rtk gain                        # 절약 통계
rtk gain --graph                # ASCII 그래프 (30일)
rtk discover                    # 놓친 절약 기회 발견
```

## 문서

- **[TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)** - 일반적인 문제 해결
- **[INSTALL.md](INSTALL.md)** - 상세 설치 가이드
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - 기술 아키텍처

## 기여

기여를 환영합니다! [GitHub](https://github.com/rtk-ai/rtk)에서 issue 또는 PR을 생성해 주세요.

[Discord](https://discord.gg/RySmvNF5kF) 커뮤니티에 참여하세요.

## 라이선스

Apache 2.0 라이선스 - 자세한 내용은 [LICENSE](LICENSE)를 참조하세요.

## 면책 조항

자세한 내용은 [DISCLAIMER.md](DISCLAIMER.md)를 참조하세요.
