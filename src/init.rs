use anyhow::Result;
use std::fs;
use std::path::PathBuf;

const RTK_INSTRUCTIONS: &str = r#"# Instructions: Utiliser rtk pour économiser des tokens

## Règle obligatoire
Remplacer les commandes Unix standard par `rtk` pour minimiser la consommation de tokens.

**IMPORTANT:** Même lors du chaînage de commandes avec `&&`, utiliser `rtk`:
```bash
# ❌ Ne pas faire
git add . && git commit -m "msg" && git push

# ✅ Faire
rtk git add && rtk git commit -m "msg" && rtk git push
```

## Substitutions

| ❌ Ne pas utiliser | ✅ Utiliser |
|-------------------|-------------|
| `ls`, `tree` | `rtk ls <path>` |
| `cat`, `head`, `tail` | `rtk read <file>` |
| `cat` pour comprendre du code | `rtk read <file> -l aggressive` |
| `find`, `fd` | `rtk find <pattern>` |
| `diff file1 file2` | `rtk diff <f1> <f2>` |
| `git status` | `rtk git status` |
| `git log` | `rtk git log` |
| `git diff` | `rtk git diff` |
| `git add .` | `rtk git add` |
| `git commit -m "msg"` | `rtk git commit -m "msg"` |
| `git push` | `rtk git push` |
| `git pull` | `rtk git pull` |
| `cargo test`, `pytest`, `npm test` | `rtk test <cmd>` |
| `<cmd> 2>&1 \| grep -i error` | `rtk err <cmd>` |
| `cat file.log` | `rtk log <file>` |
| `cat package.json` | `rtk json <file>` |
| `cat Cargo.toml` (pour deps) | `rtk deps` |
| `env`, `printenv` | `rtk env` |
| `docker ps` | `rtk docker ps` |
| `docker images` | `rtk docker images` |
| `docker logs <c>` | `rtk docker logs <c>` |
| `kubectl get pods` | `rtk kubectl pods` |
| `kubectl logs <pod>` | `rtk kubectl logs <pod>` |
| `grep -rn`, `rg` | `rtk grep <pattern>` |
| `<longue commande>` | `rtk summary <cmd>` |

## Commandes rtk (15 total)

```bash
# Fichiers
rtk ls .                        # Arbre filtré (-82% tokens)
rtk read file.rs -l aggressive  # Signatures seules (-74% tokens)
rtk smart file.rs               # Résumé 2 lignes
rtk find "*.rs" .               # Find compact groupé par dossier
rtk diff f1.txt f2.txt          # Diff ultra-condensé

# Git
rtk git status                  # Status compact
rtk git log -n 10               # 10 commits compacts
rtk git diff                    # Diff compact
rtk git add                     # Add → "ok ✓"
rtk git commit -m "msg"         # Commit → "ok ✓ abc1234"
rtk git push                    # Push → "ok ✓ main"
rtk git pull                    # Pull → "ok ✓ 3 files"
rtk grep "pattern"              # Grep groupé par fichier

# Commandes
rtk test cargo test             # Échecs seuls (-90% tokens)
rtk err npm run build           # Erreurs seules (-80% tokens)
rtk summary <cmd>               # Résumé heuristique
rtk log app.log                 # Logs dédupliqués (erreurs ×N)

# Données
rtk json config.json            # Structure sans valeurs
rtk deps                        # Résumé dépendances
rtk env -f AWS                  # Vars filtrées

# Conteneurs
rtk docker ps                   # Conteneurs compacts
rtk docker images               # Images compactes
rtk docker logs <container>     # Logs dédupliqués
rtk kubectl pods                # Pods compacts
rtk kubectl services            # Services compacts
rtk kubectl logs <pod>          # Logs dédupliqués
```
"#;

pub fn run(global: bool, verbose: u8) -> Result<()> {
    let path = if global {
        dirs::home_dir()
            .map(|h| h.join(".claude").join("CLAUDE.md"))
            .unwrap_or_else(|| PathBuf::from("~/.claude/CLAUDE.md"))
    } else {
        PathBuf::from("CLAUDE.md")
    };

    if global {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }

    if verbose > 0 {
        eprintln!("Writing rtk instructions to: {}", path.display());
    }

    // Check if file exists
    if path.exists() {
        let existing = fs::read_to_string(&path)?;

        // Check if rtk instructions already present
        if existing.contains("rtk") && existing.contains("Utiliser rtk") {
            println!("✅ {} already contains rtk instructions", path.display());
            return Ok(());
        }

        // Append to existing file
        let new_content = format!("{}\n\n{}", existing.trim(), RTK_INSTRUCTIONS);
        fs::write(&path, new_content)?;
        println!("✅ Added rtk instructions to existing {}", path.display());
    } else {
        // Create new file
        fs::write(&path, RTK_INSTRUCTIONS)?;
        println!("✅ Created {} with rtk instructions", path.display());
    }

    if global {
        println!("   Claude Code will now use rtk in all sessions");
    } else {
        println!("   Claude Code will use rtk in this project");
    }

    Ok(())
}

/// Show current rtk configuration
pub fn show_config() -> Result<()> {
    let home_path = dirs::home_dir().map(|h| h.join(".claude").join("CLAUDE.md"));
    let local_path = PathBuf::from("CLAUDE.md");

    println!("📋 rtk Configuration:\n");

    // Check global
    if let Some(hp) = &home_path {
        if hp.exists() {
            let content = fs::read_to_string(hp)?;
            if content.contains("rtk") {
                println!("✅ Global (~/.claude/CLAUDE.md): rtk enabled");
            } else {
                println!("⚪ Global (~/.claude/CLAUDE.md): exists but rtk not configured");
            }
        } else {
            println!("⚪ Global (~/.claude/CLAUDE.md): not found");
        }
    }

    // Check local
    if local_path.exists() {
        let content = fs::read_to_string(&local_path)?;
        if content.contains("rtk") {
            println!("✅ Local (./CLAUDE.md): rtk enabled");
        } else {
            println!("⚪ Local (./CLAUDE.md): exists but rtk not configured");
        }
    } else {
        println!("⚪ Local (./CLAUDE.md): not found");
    }

    println!("\nUsage:");
    println!("  rtk init          # Add rtk to local CLAUDE.md");
    println!("  rtk init --global # Add rtk to global ~/.claude/CLAUDE.md");

    Ok(())
}
