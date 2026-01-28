# RTK Roadmap - Plan d'Action Complet

## 🎯 Vue d'Ensemble

**Mission**: Transformer RTK d'un CLI proxy MVP vers un outil production-ready pour T3 Stack et au-delà.

**Horizon**: 4 phases sur 12 semaines

**Critères de Succès**:
- ✅ 70%+ token reduction validé en production (Méthode Aristote)
- ✅ Adoption par 3+ projets/équipes
- ✅ PRs mergées upstream ou fork maintenu comme standard

---

## 📊 État Actuel (Baseline)

### ✅ Achievements (Phase 0)
- Fork RTK upstream créé
- 3 Issues ouvertes (#2, #3, #4)
- 2 PRs créées:
  - PR #5: Git argument parsing fix → CLOSED (merged dans #6)
  - PR #6: Git + pnpm support → **OPEN** (5 commits, 429 LOC)
- Branche feat/vitest-support créée (1 commit, 522 LOC)
- Installation validée sur Méthode Aristote
- **43.5K tokens économisés** sur 18 commandes (68.8%)

### ❌ Gaps Identifiés
- Async/await: 0% du codebase
- Observability: Pas de tracing structuré
- Type safety: Pas de newtypes métier
- Cross-platform: Tests uniquement macOS
- Upstream engagement: Pas de réponse maintainer (1 semaine)

---

## 🚀 Phase 1: Production Readiness (Semaines 1-2)

**Objectif**: Stabiliser RTK pour usage quotidien sur Méthode Aristote

### 1.1 Issues Upstream (Priorité: 🔴 CRITIQUE)

**Issue #2: Git argument parsing bug → RÉSOLU** ✅
- Status: Résolu par PR #6
- Action: Monitorer merge de PR #6

**Issue #3: T3 Stack support (pnpm + Vitest) → EN COURS** 🔄
- pnpm: ✅ Implémenté (PR #6)
- Vitest: ✅ Implémenté (feat/vitest-support branch)
- Action: Tester Vitest sur Aristote, créer PR #7

**Issue #4: grep/ls bugs → TODO** ⏳
- Priorité: MEDIUM (pas bloquant)
- Effort: 1-2 jours
- Action: Repro bugs sur Aristote, fix + tests

### 1.2 Vitest Support - Finalisation (Priorité: 🟡 HIGH)

**Status**: Module implémenté (298 LOC + tests), non testé en production

**Actions restantes**:
1. ✅ **Test sur Aristote** (1h)
   ```bash
   cd /Users/florianbruniaux/Sites/MethodeAristote/app
   pnpm test | tee /tmp/vitest-raw.txt
   rtk vitest run | tee /tmp/vitest-rtk.txt
   wc -c /tmp/vitest-*.txt  # Compare token counts
   ```

2. **Mesurer économies réelles** (30 min)
   - Target: 90% reduction (10.5K → 1K chars)
   - Valider format: `PASS (n) FAIL (n) + failures + timing`

3. **Documenter dans README** (1h)
   - Ajouter section Vitest
   - Exemples before/after
   - Token savings metrics

4. **Créer PR #7** (si PR #6 pas mergée après 2 semaines)
   - Vitest standalone OU
   - Combiner avec #6 si maintainer responsive

**Estimation**: 3-4h total

### 1.3 Documentation & Onboarding (Priorité: 🟡 MEDIUM)

**Objectif**: Faciliter adoption par d'autres équipes

**Livrables**:
1. **README.md exhaustif** (2h)
   - Quick start (3 étapes max)
   - Tous les use cases T3 Stack
   - Troubleshooting FAQ
   - Benchmarks visuels (graphes token savings)

2. **CONTRIBUTING.md** (1h)
   - Guidelines pour PRs
   - Architecture overview
   - Testing strategy
   - Code review checklist

3. **Video demo** (optionnel, 2h)
   - Screencast 5-10 min
   - Installation → Usage → Savings
   - Publier sur YouTube + embed README

**Estimation**: 5h total

### 1.4 Testing & Quality (Priorité: 🟡 MEDIUM)

**Objectif**: Confiance pour déployer chez d'autres

**Actions**:
1. **Cross-platform validation** (2h)
   - macOS: ✅ OK
   - Linux: À tester (via Docker)
   - Windows: À tester (via WSL ou VM)

2. **Integration tests** (3h)
   - Tester sur 2-3 projets T3 Stack publics
   - Vérifier: next, vitest, pnpm, prisma
   - Documenter edge cases

3. **CI/CD enhancement** (2h)
   - Ajouter tests dans GitHub Actions
   - Test matrix: [macOS, Linux, Windows]
   - Clippy lints + cargo fmt check

**Estimation**: 7h total

---

## 🎯 Phase 2: Upstream Engagement (Semaines 3-4)

**Objectif**: Merger PRs upstream OU établir fork comme standard

### 2.1 Stratégie de Merge (Priorité: 🔴 CRITICAL)

**Scénario A: PR #6 mergée rapidement** ✅
- Action: Créer PR #7 (Vitest) dès merge de #6
- Timeframe: 1 semaine après merge #6

**Scénario B: Pas de réponse après 2 semaines** ⚠️
- Action: Pivot vers fork maintenu indépendamment
- Communication:
  ```markdown
  ## Fork Status

  This fork contains critical fixes and modern JS stack support:
  - Git argument parsing (upstream PR #6 pending)
  - pnpm support for T3 Stack
  - Vitest test runner integration

  **Use this fork** until upstream merges these features.

  Installation: `cargo install --git https://github.com/FlorianBruniaux/rtk`
  ```

**Scénario C: Maintainer demande des changements** 🔄
- Action: Appliquer feedback rapidement (< 48h)
- Priorité: Maintenir momentum

### 2.2 Community Building (Priorité: 🟢 LOW)

**Objectif**: Créer traction pour adoption

**Actions**:
1. **Blog post technique** (4h)
   - Titre: "Reducing LLM Token Usage by 70% with RTK on T3 Stack"
   - Contenu: Problem → Solution → Results → Code
   - Publier: dev.to, Medium, X (Twitter)

2. **Engagement Reddit/HN** (2h)
   - Post sur r/rust, r/typescript, r/nextjs
   - Show HN si traction forte
   - Focus: Real metrics, production usage

3. **Issue templates upstream** (1h)
   - Faciliter contributions d'autres users
   - Bug report, feature request, support

**Estimation**: 7h total

---

## 🎯 Phase 3: Advanced Features (Semaines 5-8)

**Objectif**: Étendre RTK au-delà du MVP

### 3.1 Architecture Moderne (Priorité: 🟡 MEDIUM)

**3.1.1 Async/Await Refactor** (Priorité: 🔴 HIGH si LLM integration)

**Problème actuel**:
```rust
// Blocking sync code
let output = Command::new("git").output()?;
```

**Target**:
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let output = tokio::process::Command::new("git")
        .output()
        .await?;
}
```

**Bénéfices**:
- Parallel command execution
- Future LLM API integration (`rtk ask "explain this"`)
- Streaming responses

**Effort**: 2-3 semaines (refactor complet)

**Décision**: ⚠️ **Attendre validation métier**
- Si RTK reste CLI proxy → Pas nécessaire
- Si évolution vers agent LLM → Indispensable

**Action immédiate**: Prototyper branch `feat/async-refactor` sans merger

### 3.1.2 Observability avec Tracing** (Priorité: 🟡 MEDIUM)

**Problème actuel**:
```rust
if verbose > 0 {
    eprintln!("pnpm list (filtered):");
}
```

**Target**:
```rust
use tracing::{info, instrument};

#[instrument(skip(args))]
fn run_pnpm_list(args: &[String]) -> Result<()> {
    info!(command = "pnpm list", "Executing");
    // ...
    info!(
        input_tokens = %input,
        output_tokens = %output,
        savings_pct = %savings,
        "Command completed"
    );
}
```

**Bénéfices**:
- Structured logs (JSON export)
- Performance debugging
- Production monitoring

**Effort**: 1 semaine

**Actions**:
1. Ajouter `tracing` + `tracing-subscriber` deps
2. Replace `eprintln!` par `tracing::*` macros
3. Add `--log-format json` flag
4. Export to OpenTelemetry (optionnel)

### 3.1.3 Type Safety avec Newtypes** (Priorité: 🟢 LOW)

**Problème actuel**:
```rust
pub fn track(original_cmd: &str, rtk_cmd: &str, ...)
// Facile de confondre les deux
```

**Target**:
```rust
#[derive(Debug)]
struct OriginalCommand(String);

#[derive(Debug)]
struct RtkCommand(String);

pub fn track(
    original: OriginalCommand,
    rtk: RtkCommand,
    savings: TokenSavings
)
```

**Bénéfices**:
- Compile-time safety
- Self-documenting code
- Refactoring confidence

**Effort**: 2-3 jours

**Actions**:
1. Créer `types.rs` module
2. Define newtypes métier
3. Migrate incrementally (one module at a time)

### 3.2 Features Utilisateurs (Priorité: 🟡 MEDIUM)

**3.2.1 Config File Support** (Priorité: 🟢 LOW)

**Use case**: Personnaliser filtres par projet

**Target** (`~/.config/rtk/config.toml`):
```toml
[filters]
git_status_max_files = 50
pnpm_list_max_depth = 2

[tokens]
estimate_multiplier = 4  # 1 char ≈ 4 tokens

[integrations]
export_format = "json"
```

**Effort**: 2-3 jours

**Actions**:
1. Extend `config.rs` module
2. Add `--config` flag
3. Merge with existing hardcoded defaults
4. Add `rtk config show` command

**3.2.2 Watch Mode** (Priorité: 🟢 LOW)

**Use case**: Monitor file changes + auto-execute

**Target**:
```bash
rtk watch "pnpm test" --on-change "src/**/*.ts"
# Re-runs tests on file save, filtered output
```

**Effort**: 1 semaine (needs `notify` crate)

**Décision**: ⚠️ **Bas ROI** - Existe déjà dans test runners

**3.2.3 LLM Integration** (Priorité: 🔴 HIGH si adoption forte)

**Use case**: Ask questions about codebase

**Target**:
```bash
rtk ask "Explain this git log"
rtk ask "What changed in last commit?" --context "git show"
```

**Architecture**:
```rust
use anthropic_sdk::Client;

async fn ask_command(prompt: &str, context_cmd: Option<&str>) {
    let context = if let Some(cmd) = context_cmd {
        execute_and_filter(cmd).await?
    } else {
        String::new()
    };

    let response = client.messages()
        .create(MessagesRequest {
            model: "claude-opus-4-5",
            messages: vec![Message {
                role: "user",
                content: format!("{}\n\nContext:\n{}", prompt, context),
            }],
            max_tokens: 1000,
        })
        .await?;

    println!("{}", response.content);
}
```

**Effort**: 2-3 semaines (requires async refactor)

**Bénéfices**:
- RTK devient agent, pas juste proxy
- Killer feature vs upstream

**Risques**:
- Needs API keys (friction onboarding)
- Costs money (user concern)
- Async refactor mandatory

**Décision**: ⚠️ **Phase 4** - Après validation adoption RTK classique

---

## 🎯 Phase 4: Ecosystem & Scale (Semaines 9-12)

**Objectif**: RTK comme standard T3 Stack tooling

### 4.1 Package Distribution (Priorité: 🔴 CRITICAL)

**4.1.1 Homebrew Tap** (macOS users)

**Actions**:
1. Create `homebrew-tap` repo
2. Add Formula (déjà existe: `Formula/rtk.rb`)
3. Automate releases via GitHub Actions
4. Test: `brew install florianbruniaux/tap/rtk`

**Effort**: 1 jour

**4.1.2 Binary Releases** (multi-platform)

**Target platforms**:
- macOS (Intel + Apple Silicon)
- Linux (x86_64 + ARM64)
- Windows (x86_64)

**Actions**:
1. Enhance `.github/workflows/release.yml`
2. Cross-compile with `cross` tool
3. Upload to GitHub Releases
4. Add checksums (SHA256)

**Effort**: 1-2 jours (déjà 80% fait)

**4.1.3 npm Package** (optionnel, JavaScript devs)

**Use case**: `npx rtk git status` sans installer Rust

**Implementation**:
```json
{
  "name": "@rtk/cli",
  "bin": {
    "rtk": "./bin/rtk"
  },
  "postinstall": "node scripts/download-binary.js"
}
```

**Effort**: 2-3 jours

**Décision**: ⚠️ **Évaluer demand** - Peut être overkill

### 4.2 IDE Integrations (Priorité: 🟡 MEDIUM)

**4.2.1 VSCode Extension**

**Features**:
- Inline token savings preview
- Command palette: `RTK: Run Command`
- Status bar: Token savings today
- Settings: Configure filters

**Effort**: 1-2 semaines (TypeScript + Extension API)

**4.2.2 Cursor/Windsurf Integration**

**Use case**: Native RTK support in AI IDEs

**Actions**:
1. Propose integration to Cursor team
2. Provide SDK/API for tool invocation
3. Documentation for integration

**Effort**: 1 semaine (mostly coordination)

### 4.3 Community & Support (Priorité: 🟢 LOW)

**4.3.1 Documentation Site** (optionnel)

**Stack**: Nextra (Next.js docs framework)

**Sections**:
- Getting Started
- Command Reference
- Integration Guides (T3 Stack, Remix, etc.)
- FAQ
- Blog

**Effort**: 1 semaine

**URL**: `rtk-docs.vercel.app` OU GitHub Pages

**4.3.2 Discord Community** (optionnel)

**Use case**: User support, feature requests

**Effort**: Setup 1h, moderation ongoing

**Décision**: ⚠️ **Seulement si adoption >100 users**

---

## 🎓 Skills Rust à Développer

**Basé sur analyse guide "Rust + Claude AI"**

### Niveau 1: Fondations (Semaines 1-2)

**Async/Await + Tokio** (Priorité: 🔴 HIGH)
- Resource: [Rust Async Book](https://rust-lang.github.io/async-book/)
- Projet: Refactor `git.rs` vers async
- Validation: Parallel `rtk git status && rtk git log`

**Tracing/Observability** (Priorité: 🟡 MEDIUM)
- Resource: [tracing crate docs](https://docs.rs/tracing)
- Projet: Add structured logging to all commands
- Validation: `rtk --log-format json | jq`

### Niveau 2: Intermédiaire (Semaines 3-4)

**Error Handling Patterns** (Priorité: 🟡 MEDIUM)
- Resource: [thiserror + anyhow guide](https://nick.groenen.me/posts/rust-error-handling/)
- Projet: Create custom error types with context
- Validation: Error messages 100% actionables

**Type Safety Patterns** (Priorité: 🟢 LOW)
- Resource: [Rust newtypes pattern](https://doc.rust-lang.org/rust-by-example/generics/new_types.html)
- Projet: Introduce `Command`, `TokenCount` newtypes
- Validation: Compile errors on type confusion

### Niveau 3: Avancé (Semaines 5-8)

**Production Deployment** (Priorité: 🟡 MEDIUM)
- Resource: [Building reliable systems in Rust](https://www.shuttle.rs/blog)
- Projet: Health checks, metrics, graceful shutdown
- Validation: Deploy as systemd service

**Cross-platform Development** (Priorité: 🟡 MEDIUM)
- Resource: [cross tool](https://github.com/cross-rs/cross)
- Projet: Windows support (path handling, commands)
- Validation: CI tests on Windows/Linux/macOS

### Niveau 4: Expert (Semaines 9-12)

**LLM API Integration** (Priorité: 🔴 HIGH si feature activée)
- Resource: [Claude Agent SDK](https://lib.rs/crates/claude-agent-sdk)
- Projet: `rtk ask` command with streaming
- Validation: Interactive Q&A with codebase context

**Performance Optimization** (Priorité: 🟢 LOW)
- Resource: [Criterion benchmarking](https://github.com/bheisler/criterion.rs)
- Projet: Benchmark filters, optimize hot paths
- Validation: <100ms overhead on commands

---

## 📊 Métriques de Succès

### Phase 1 (Semaines 1-2)
- ✅ Vitest testé sur Aristote (90% token reduction)
- ✅ PR #6 mergée OU fork documenté comme stable
- ✅ 5+ projets adoptent RTK (dont 2 externes)

### Phase 2 (Semaines 3-4)
- ✅ Blog post publié (500+ vues)
- ✅ 10+ GitHub stars
- ✅ 1+ contribution externe (issue/PR)

### Phase 3 (Semaines 5-8)
- ✅ Tracing intégré (structured logs)
- ✅ Async refactor prototypé (si applicable)
- ✅ Config file support shipped

### Phase 4 (Semaines 9-12)
- ✅ Homebrew formula published
- ✅ 50+ GitHub stars
- ✅ Utilisé en production par 3+ companies

---

## 🚨 Risques & Mitigations

### Risque 1: Maintainer upstream inactif
**Impact**: PRs jamais mergées
**Probabilité**: MEDIUM (1 semaine sans réponse)
**Mitigation**: Fork maintenu indépendamment, doc claire

### Risque 2: Vitest breaking changes
**Impact**: Module obsolète
**Probabilité**: LOW (API stable)
**Mitigation**: Tests version-pinned, monitor releases

### Risque 3: Async refactor trop coûteux
**Impact**: 3 semaines perdues sans ROI
**Probabilité**: MEDIUM
**Mitigation**: Prototyper d'abord, valider use case avant commit

### Risque 4: Adoption faible
**Impact**: Effort gaspillé
**Probabilité**: LOW (besoin réel validé)
**Mitigation**: Focus Méthode Aristote d'abord, élargir après

### Risque 5: Concurrence (autre tool similaire)
**Impact**: RTK devient obsolète
**Probabilité**: VERY LOW
**Mitigation**: Niche T3 Stack, first-mover advantage

---

## 🎯 Décisions Stratégiques Immédiates

### Décision 1: Upstream vs Fork Indépendant
**Deadline**: Fin Semaine 2
**Critère**: Réponse maintainer sur PR #6
**Action**: Si pas de réponse → Pivot vers fork

### Décision 2: Async Refactor Go/No-Go
**Deadline**: Fin Phase 2
**Critère**: Use cases LLM integration validés
**Action**: Si pas de demand → Skip Phase 3.1.1

### Décision 3: VSCode Extension Go/No-Go
**Deadline**: Fin Phase 3
**Critère**: 50+ GitHub stars + 10+ actifs users
**Action**: Si pas atteint → Focus CLI uniquement

---

## 🗓️ Timeline Visuelle

```
Semaines 1-2: ███████████████████████ Phase 1 (Production Ready)
├─ Vitest testing
├─ Issue #4 fix
├─ Documentation
└─ Cross-platform tests

Semaines 3-4: ███████████████████████ Phase 2 (Upstream Engagement)
├─ PR monitoring
├─ Blog post
└─ Community building

Semaines 5-8: ███████████████████████ Phase 3 (Advanced Features)
├─ Tracing integration
├─ Async prototype (conditional)
└─ Config file support

Semaines 9-12: ██████████████████████ Phase 4 (Ecosystem & Scale)
├─ Homebrew tap
├─ Binary releases
└─ IDE integrations (conditional)
```

---

## 📞 Points de Contrôle

**Weekly Reviews** (chaque lundi):
- Progrès vs roadmap
- Blockers identification
- Pivot decisions

**Monthly Retrospectives**:
- Métriques adoption
- User feedback synthesis
- Roadmap adjustments

**Stakeholders**:
- Florian (lead dev)
- Claude Code (AI pair programmer)
- Méthode Aristote team (beta users)
- Open source community (feedback loop)

---

## 🎬 Prochaine Action Immédiate

**TODAY** (2h):
1. ✅ Test Vitest sur Aristote
2. ✅ Measure token savings
3. ✅ Update README avec metrics

**THIS WEEK** (5h):
1. Fix Issue #4 (grep/ls bugs)
2. Cross-platform test (Linux via Docker)
3. Create PR #7 (Vitest) si PR #6 stale

**NEXT 2 WEEKS**:
- Decision point: Upstream vs Fork
- Community engagement (blog post)
- Onboard 2-3 external projects

---

**Dernière mise à jour**: 2026-01-28
**Auteur**: Florian Bruniaux
**Status**: ACTIVE - Phase 1 en cours
