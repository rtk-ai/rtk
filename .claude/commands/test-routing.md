---
model: haiku
description: Test RTK command routing without execution (dry-run) - verifies which commands have filters
---

# /test-routing

Vérifie le routing de commandes RTK sans exécution (dry-run). Utile pour tester si une commande a un filtre disponible avant de l'exécuter.

## Usage

```
/test-routing <command> [args...]
```

## Exemples

```bash
/test-routing git status
# Output: ✅ RTK filter available: git status → rtk git status

/test-routing npm install
# Output: ⚠️  No RTK filter, would execute raw: npm install

/test-routing cargo test
# Output: ✅ RTK filter available: cargo test → rtk cargo test
```

## Quand utiliser

- **Avant d'exécuter une commande**: Vérifier si RTK a un filtre
- **Debugging hook integration**: Tester le command routing sans side-effects
- **Documentation**: Identifier quelles commandes RTK supporte
- **Testing**: Valider routing logic sans exécuter de vraies commandes

## Implémentation

### Option 1: Check RTK Help Output

```bash
COMMAND="$1"
shift
ARGS="$@"

# Check if RTK has subcommand for this command
if rtk --help | grep -E "^  $COMMAND" >/dev/null 2>&1; then
    echo "✅ RTK filter available: $COMMAND $ARGS → rtk $COMMAND $ARGS"
    echo ""
    echo "Expected behavior:"
    echo "  - Command will be filtered through RTK"
    echo "  - Output condensed for token efficiency"
    echo "  - Exit code preserved from original command"
else
    echo "⚠️  No RTK filter available, would execute raw: $COMMAND $ARGS"
    echo ""
    echo "Expected behavior:"
    echo "  - Command executed without RTK filtering"
    echo "  - Full command output (no token savings)"
    echo "  - Original command behavior unchanged"
fi
```

### Option 2: Check RTK Source Code

```bash
COMMAND="$1"
shift
ARGS="$@"

# List of supported RTK commands (from src/main.rs)
RTK_COMMANDS=(
    "git"
    "grep"
    "ls"
    "read"
    "err"
    "test"
    "log"
    "json"
    "lint"
    "tsc"
    "next"
    "prettier"
    "playwright"
    "prisma"
    "gh"
    "vitest"
    "pnpm"
    "ruff"
    "pytest"
    "pip"
    "go"
    "golangci-lint"
    "docker"
    "cargo"
    "smart"
    "summary"
    "diff"
    "env"
    "discover"
    "gain"
    "proxy"
)

# Check if command in supported list
if [[ " ${RTK_COMMANDS[@]} " =~ " ${COMMAND} " ]]; then
    echo "✅ RTK filter available: $COMMAND $ARGS → rtk $COMMAND $ARGS"
    echo ""

    # Show filter details if available
    case "$COMMAND" in
        git)
            echo "Filter: git operations (status, log, diff, etc.)"
            echo "Token savings: 60-80% depending on subcommand"
            ;;
        cargo)
            echo "Filter: cargo build/test/clippy output"
            echo "Token savings: 80-90% (failures only for tests)"
            ;;
        gh)
            echo "Filter: GitHub CLI (pr, issue, run)"
            echo "Token savings: 26-87% depending on subcommand"
            ;;
        pnpm)
            echo "Filter: pnpm package manager"
            echo "Token savings: 70-90% (dependency trees)"
            ;;
        *)
            echo "Filter: Available for $COMMAND"
            echo "Token savings: >=20% (floor)"
            ;;
    esac
else
    echo "⚠️  No RTK filter available, would execute raw: $COMMAND $ARGS"
    echo ""
    echo "Note: You can still use 'rtk bypass $COMMAND $ARGS' to:"
    echo "  - Execute command without filtering"
    echo "  - Track usage in 'rtk gain --history'"
    echo "  - Measure potential for new filter development"
fi
```

### Option 3: Interactive Mode

```bash
COMMAND="$1"
shift
ARGS="$@"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪 RTK Command Routing Test"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Command: $COMMAND $ARGS"
echo ""

# Check if RTK installed
if ! command -v rtk >/dev/null 2>&1; then
    echo "❌ ERROR: RTK not installed"
    echo "   Install with: cargo install --path ."
    exit 1
fi

# Check RTK version
RTK_VERSION=$(rtk --version 2>/dev/null | awk '{print $2}')
echo "RTK Version: $RTK_VERSION"
echo ""

# Check if command has filter
if rtk --help | grep -E "^  $COMMAND" >/dev/null 2>&1; then
    echo "✅ Filter: Available"
    echo ""
    echo "Routing:"
    echo "  Input:  $COMMAND $ARGS"
    echo "  Route:  rtk $COMMAND $ARGS"
    echo "  Filter: Applied"
    echo ""

    # Estimate token savings (based on historical data)
    case "$COMMAND" in
        git)
            echo "Expected Token Savings: 60-80%"
            echo "Startup Time: <10ms"
            ;;
        cargo)
            echo "Expected Token Savings: 80-90%"
            echo "Startup Time: <10ms"
            ;;
        gh)
            echo "Expected Token Savings: 26-87%"
            echo "Startup Time: <10ms"
            ;;
        *)
            echo "Expected Token Savings: >=20% (floor)"
            echo "Startup Time: <10ms"
            ;;
    esac
else
    echo "⚠️  Filter: Not available"
    echo ""
    echo "Routing:"
    echo "  Input:  $COMMAND $ARGS"
    echo "  Route:  $COMMAND $ARGS (raw, no RTK)"
    echo "  Filter: None"
    echo ""
    echo "Alternatives:"
    echo "  - Use 'rtk bypass $COMMAND $ARGS' to track usage"
    echo "  - Consider contributing a filter for this command"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
```

## Expected Output

### Cas 1: Commande avec filtre

```bash
/test-routing git status

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🧪 RTK Command Routing Test
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Command: git status

RTK Version: 0.16.0

✅ Filter: Available

Routing:
  Input:  git status
  Route:  rtk git status
  Filter: Applied

Expected Token Savings: 60-80%
Startup Time: <10ms

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### Cas 2: Commande sans filtre

```bash
/test-routing npm install express

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🧪 RTK Command Routing Test
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Command: npm install express

RTK Version: 0.16.0

⚠️  Filter: Not available

Routing:
  Input:  npm install express
  Route:  npm install express (raw, no RTK)
  Filter: None

Alternatives:
  - Use 'rtk bypass npm install express' to track usage
  - Consider contributing a filter for this command

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### Cas 3: RTK non installé

```bash
/test-routing cargo test

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🧪 RTK Command Routing Test
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Command: cargo test

❌ ERROR: RTK not installed
   Install with: cargo install --path .
```

## Use Cases

### Use Case 1: Pre-Flight Check

Avant d'exécuter une commande coûteuse, vérifier si RTK a un filtre :

```bash
/test-routing cargo build --all-targets
# ✅ Filter available → use rtk cargo build
# ⚠️  No filter → use raw cargo build
```

### Use Case 2: Hook Debugging

Tester le hook integration sans side-effects :

```bash
# Test several commands
/test-routing git log -10
/test-routing gh pr view 123
/test-routing docker ps

# Verify routing logic works for all
```

### Use Case 3: Documentation

Générer liste de commandes supportées :

```bash
# Test all common commands
for cmd in git cargo gh pnpm docker npm yarn; do
    /test-routing $cmd
done

# Output shows which have filters
```

### Use Case 4: Contributing New Filter

Identifier commandes sans filtre qui pourraient bénéficier :

```bash
/test-routing pytest
# ⚠️  No filter

# Consider contributing pytest filter
# Expected savings: 90% (failures only)
# Complexity: Medium (JSON output parsing)
```

## Integration avec Claude Code

Dans Claude Code, cette command permet de :

1. **Vérifier hook integration** : Test si hooks rewrites commands correctement
2. **Debugging** : Identifier pourquoi certaines commandes ne sont pas filtrées
3. **Documentation** : Montrer à l'utilisateur quelles commandes RTK supporte

**Exemple workflow** :

```
User: "Is git status supported by RTK?"
Assistant: "Let me check with /test-routing git status"
[Runs command]
Assistant: "Yes! RTK has a filter for git status with 60-80% token savings."
```

## Limitations

- **Dry-run only** : Ne teste pas l'exécution réelle (pas de validation output)
- **No side-effects** : Aucune commande n'est exécutée
- **Routing check only** : Vérifie seulement la disponibilité du filtre, pas la qualité

Pour tester le filtre complet, utiliser :
```bash
rtk <cmd>  # Exécution réelle avec filtre
```
