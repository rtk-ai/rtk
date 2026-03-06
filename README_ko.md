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
  <a href="https://discord.gg/pvHdzAec"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">웹사이트</a> &bull;
  <a href="#설치">설치</a> &bull;
  <a href="docs/TROUBLESHOOTING.md">문제 해결</a> &bull;
  <a href="ARCHITECTURE.md">아키텍처</a> &bull;
  <a href="https://discord.gg/pvHdzAec">Discord</a>
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

## 토큰 절약 (30분 Claude Code 세션)

| 작업 | 빈도 | 표준 | rtk | 절약 |
|------|------|------|-----|------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **합계** | | **~118,000** | **~23,900** | **-80%** |

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
rtk test cargo test             # 실패만 표시 (-90%)
rtk vitest run                  # Vitest 컴팩트
rtk pytest                      # Python 테스트 (-90%)
rtk go test                     # Go 테스트 (-90%)
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
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - 기술 아키텍처

## 기여

기여를 환영합니다! [GitHub](https://github.com/rtk-ai/rtk)에서 issue 또는 PR을 생성해 주세요.

[Discord](https://discord.gg/pvHdzAec) 커뮤니티에 참여하세요.

## 라이선스

MIT 라이선스 - 자세한 내용은 [LICENSE](LICENSE)를 참조하세요.
