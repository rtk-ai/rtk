use crate::core::child_command::ChildCommand;
use crate::core::{user_dirs, user_env};
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

const GIT_URL: &str = "https://github.com/rtk-ai/rtk";

fn plugins_dir() -> Result<PathBuf> {
    let dir = user_dirs::config()
        .context("no config directory")?
        .join("plugins");
    Ok(dir)
}

fn source_root() -> Option<PathBuf> {
    user_env::var_os("RTK_PLUGIN_SOURCE").map(PathBuf::from)
}

fn dep_line(crate_name: &str, subpath: &str) -> String {
    match source_root() {
        Some(root) => {
            let path = root.join(subpath);
            format!(
                "{crate_name} = {{ path = {:?} }}",
                path.display().to_string()
            )
        }
        None => {
            let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
            format!("{crate_name} = {{ git = \"{GIT_URL}\", tag = \"{tag}\" }}")
        }
    }
}

fn to_struct_name(method: &str) -> String {
    method
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

pub fn run_new(name: &str) -> Result<i32> {
    if !is_valid_name(name) {
        bail!("plugin name must be alphanumeric with - or _, starting with a letter: `{name}`");
    }
    let dir = plugins_dir()?.join(name);
    if dir.exists() {
        bail!("plugin `{name}` already exists at {}", dir.display());
    }
    std::fs::create_dir_all(dir.join("src"))
        .with_context(|| format!("create {}", dir.display()))?;

    let cargo = format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [lib]\n\
         \n\
         [dependencies]\n\
         {}\n",
        dep_line("rtk-plugin", "plugin"),
    );
    std::fs::write(dir.join("Cargo.toml"), cargo).context("write Cargo.toml")?;

    let struct_name = to_struct_name(name);
    let lib = format!(
        "use rtk_plugin::{{rtk_plugin, Block, Method, MethodCtx, MethodInfo, Outcome}};\n\
         \n\
         struct {struct_name};\n\
         \n\
         impl Method for {struct_name} {{\n\
         \x20   fn name(&self) -> &'static str {{\n\
         \x20       \"{name}\"\n\
         \x20   }}\n\
         \n\
         \x20   fn info(&self) -> MethodInfo {{\n\
         \x20       MethodInfo::LOSSLESS_TRANSFORM\n\
         \x20   }}\n\
         \n\
         \x20   fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {{\n\
         \x20       Outcome::Continue(block.text.to_string())\n\
         \x20   }}\n\
         }}\n\
         \n\
         rtk_plugin!({struct_name});\n",
    );
    std::fs::write(dir.join("src").join("lib.rs"), lib).context("write src/lib.rs")?;

    println!("created plugin `{name}` at {}", dir.display());
    println!(
        "  edit {}/src/lib.rs — the method name is `{name}`",
        dir.display()
    );
    println!("  then: rtk plugins build");
    Ok(0)
}

fn discover_crates() -> Result<Vec<(String, PathBuf)>> {
    let dir = plugins_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.join("Cargo.toml").is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && !name.starts_with('.')
        {
            out.push((name.to_string(), path));
        }
    }
    out.sort();
    Ok(out)
}

pub fn run_list() -> Result<i32> {
    let crates = discover_crates()?;
    if crates.is_empty() {
        println!("no plugins — scaffold one with `rtk plugins new <name>`");
    } else {
        println!("plugin crates in {}:", plugins_dir()?.display());
        for (name, path) in &crates {
            println!("  {name:<24} {}", path.display());
        }
    }

    let mut shipped: std::collections::BTreeSet<String> = rtk_engine::registry::METHODS
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    shipped.extend(
        rtk_engine::registry::CHAIN_NAMES
            .iter()
            .map(|n| n.to_string()),
    );
    shipped.extend(
        rtk_engine::registry::PROXY_METHOD_INFO
            .iter()
            .map(|(n, _)| n.to_string()),
    );
    let extra: Vec<String> = rtk_engine::registry::known_names()
        .into_iter()
        .filter(|n| !shipped.contains(n))
        .collect();
    if !extra.is_empty() {
        println!("plugin methods linked into this binary:");
        for name in extra {
            if let Some(info) = rtk_engine::registry::info_of(&name) {
                println!(
                    "  {name:<24} lossy={} recovery={:?}",
                    info.lossy, info.recovery
                );
            } else {
                println!("  {name}");
            }
        }
    }
    Ok(0)
}

pub fn run_build() -> Result<i32> {
    let crates = discover_crates()?;
    if crates.is_empty() {
        bail!("no plugins to build — scaffold one with `rtk plugins new <name>`");
    }
    let host = plugins_dir()?.join(".host");
    std::fs::create_dir_all(host.join("src")).context("create host dir")?;

    let mut deps = String::new();
    deps.push_str(&dep_line("rtk", "."));
    deps.push('\n');
    deps.push_str(&dep_line("rtk-plugin", "plugin"));
    deps.push('\n');
    for (name, path) in &crates {
        deps.push_str(&format!(
            "{name} = {{ path = {:?} }}\n",
            path.display().to_string()
        ));
    }
    let cargo = format!(
        "[package]\n\
         name = \"rtk-with-plugins\"\n\
         version = \"{}\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [[bin]]\n\
         name = \"rtk\"\n\
         path = \"src/main.rs\"\n\
         \n\
         [dependencies]\n\
         {deps}",
        env!("CARGO_PKG_VERSION"),
    );
    std::fs::write(host.join("Cargo.toml"), cargo).context("write host Cargo.toml")?;

    let mut main = String::from("fn main() {\n");
    for (name, _) in &crates {
        let ident = crate_ident(name);
        main.push_str(&format!(
            "    for m in {ident}::__rtk_plugin_methods() {{\n        let _ = rtk_plugin::register(m);\n    }}\n    for k in {ident}::__rtk_plugin_kinds() {{\n        let _ = rtk_plugin::register_kind(k);\n    }}\n"
        ));
    }
    main.push_str("    std::process::exit(rtk::entry());\n}\n");
    std::fs::write(host.join("src").join("main.rs"), main).context("write host main.rs")?;

    println!("building a custom rtk with {} plugin(s)...", crates.len());
    let status = ChildCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&host)
        .status()
        .context("run cargo build — is cargo on PATH?")?;
    if !status.success() {
        bail!("plugin build failed");
    }

    println!("installing the custom rtk to ~/.cargo/bin...");
    let install = ChildCommand::new("cargo")
        .args(["install", "--path", ".", "--force"])
        .current_dir(&host)
        .status()
        .context("run cargo install")?;
    if !install.success() {
        bail!(
            "plugin install failed — the binary is at {}",
            host.join("target/release/rtk").display()
        );
    }
    println!("done — your rtk now includes: {}", crate_names(&crates));
    Ok(0)
}

fn crate_names(crates: &[(String, PathBuf)]) -> String {
    crates
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
