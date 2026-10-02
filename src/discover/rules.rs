use super::report::RtkStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineSafety {
    None,
    ProducerOnly,
    #[allow(dead_code)]
    FinalOnly,
    Both,
}

impl PipelineSafety {
    pub fn producer_safe(self) -> bool {
        matches!(self, Self::ProducerOnly | Self::Both)
    }

    pub fn final_safe(self) -> bool {
        matches!(self, Self::FinalOnly | Self::Both)
    }
}

pub struct RtkRule {
    pub pattern: &'static str,
    pub rtk_cmd: &'static str,
    /// Pipeline stage positions this command may be rewritten in (#3171).
    pub pipeline_safety: PipelineSafety,
    pub rewrite_prefixes: &'static [&'static str],
    pub category: &'static str,
    pub savings_pct: f64,
    pub subcmd_savings: &'static [(&'static str, f64)],
    pub subcmd_status: &'static [(&'static str, RtkStatus)],
}

impl RtkRule {
    // `Default::default()` isn't a const fn on stable, so `RULES` (a const array) can't call
    // it via `..Default::default()` — hence this associated const as the const-context
    // workaround. Once `const_trait_impl` stabilizes, drop this and derive/impl
    // `const Default` instead, then switch call sites back to `..RtkRule::default()`.
    pub const DEFAULT: RtkRule = RtkRule {
        pattern: "",
        rtk_cmd: "",
        pipeline_safety: PipelineSafety::None,
        rewrite_prefixes: &[],
        category: "",
        savings_pct: 60.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    };
}

impl Default for RtkRule {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A rule declaring subcommands also needs its tool in
/// `core::tracking::SUBCOMMAND_ROUTERS`, or its telemetry label stops at the tool name.
pub const RULES: &[RtkRule] = &[
    RtkRule {
        pattern: r"^(?:git|yadm)[ \t\n]+(?:-[Cc][ \t\n]+[^ \t\n]+[ \t\n]+)*(status|log|diff|show|add|commit|checkout|push|pull|branch|fetch|stash|worktree)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk git",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["git", "yadm"],
        category: "Git",
        savings_pct: 70.0,
        subcmd_savings: &[
            ("diff", 80.0),
            ("show", 80.0),
            ("add", 59.0),
            ("commit", 59.0),
        ],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^gh[ \t\n]+(pr|issue|run|repo|api|release)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk gh",
        rewrite_prefixes: &["gh"],
        category: "GitHub",
        savings_pct: 82.0,
        subcmd_savings: &[("pr", 87.0), ("run", 82.0), ("issue", 80.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^glab[ \t\n]+(mr|issue|ci|pipeline|api|release)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk glab",
        rewrite_prefixes: &["glab"],
        category: "GitLab",
        savings_pct: 82.0,
        subcmd_savings: &[("mr", 87.0), ("ci", 82.0), ("issue", 80.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^cargo[ \t\n]+(build|test|clippy|check|fmt|install)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk cargo",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["cargo"],
        category: "Cargo",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("check", 80.0)],
        subcmd_status: &[("fmt", RtkStatus::Passthrough)],
    },
    RtkRule {
        pattern: r"^pnpm[ \t\n]+(exec|i|install|list|ls|outdated|run|run-script)",
        rtk_cmd: "rtk pnpm",
        rewrite_prefixes: &["pnpm"],
        category: "PackageManager",
        savings_pct: 80.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^npm[ \t\n]+(exec|run|run-script|rum|urn|x)([ \t\n]|$)",
        rtk_cmd: "rtk npm",
        rewrite_prefixes: &["npm"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^npx[ \t\n]+",
        rtk_cmd: "rtk npx",
        rewrite_prefixes: &["npx"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(cat|head|tail)[ \t\n]+",
        rtk_cmd: "rtk read",
        rewrite_prefixes: &["cat", "head", "tail"],
        category: "Files",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^grep[ \t\n]+",
        rtk_cmd: "rtk grep",
        pipeline_safety: PipelineSafety::Both,
        rewrite_prefixes: &["grep"],
        category: "Files",
        savings_pct: 75.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^rg[ \t\n]+",
        rtk_cmd: "rtk rg",
        pipeline_safety: PipelineSafety::Both,
        rewrite_prefixes: &["rg"],
        category: "Files",
        savings_pct: 75.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ast-grep[ \t\n]+",
        rtk_cmd: "rtk ast-grep",
        // Unlike grep/rg, `rtk ast-grep`'s run() captures with stdin null on the
        // path it filters, so it must not be rewritten as a pipeline's final
        // stage — that would silently drop the pipe input. `run --stdin` and the
        // other subcommands do reach the child's stdin, but only when invoked
        // directly: this rule keeps the hook from producing that form at all.
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ast-grep"],
        category: "Files",
        savings_pct: 85.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ls([ \t\n]|$)",
        rtk_cmd: "rtk ls",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ls"],
        category: "Files",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^find[ \t\n]+",
        rtk_cmd: "rtk find",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["find"],
        category: "Files",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?tsc([ \t\n]|$)",
        rtk_cmd: "rtk tsc",
        rewrite_prefixes: &[
            "npm exec tsc",
            "npm rum tsc",
            "npm run tsc",
            "npm run-script tsc",
            "npm tsc",
            "npm urn tsc",
            "npm x tsc",
            "npx tsc",
            "pnpm dlx tsc",
            "pnpm exec tsc",
            "pnpm run tsc",
            "pnpm run-script tsc",
            "pnpm tsc",
            "pnpx tsc",
            "tsc",
        ],
        category: "Build",
        savings_pct: 83.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?(biome|eslint|lint)([ \t\n]|$)",
        rtk_cmd: "rtk lint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "biome",
            "eslint",
            "lint",
            "npm biome",
            "npm eslint",
            "npm exec biome",
            "npm exec eslint",
            "npm lint",
            "npm rum biome",
            "npm rum eslint",
            "npm rum lint",
            "npm run biome",
            "npm run eslint",
            "npm run lint",
            "npm run-script biome",
            "npm run-script eslint",
            "npm run-script lint",
            "npm urn biome",
            "npm urn eslint",
            "npm urn lint",
            "npm x biome",
            "npm x eslint",
            "npx biome",
            "npx eslint",
            "npx lint",
            "pnpm biome",
            "pnpm dlx biome",
            "pnpm dlx eslint",
            "pnpm eslint",
            "pnpm exec biome",
            "pnpm exec eslint",
            "pnpm lint",
            "pnpm run biome",
            "pnpm run eslint",
            "pnpm run lint",
            "pnpm run-script biome",
            "pnpm run-script eslint",
            "pnpm run-script lint",
            "pnpx biome",
            "pnpx eslint",
            "pnpx lint",
        ],
        category: "Build",
        savings_pct: 84.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?prettier(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk prettier",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "npm exec prettier",
            "npm prettier",
            "npm rum prettier",
            "npm run prettier",
            "npm run-script prettier",
            "npm urn prettier",
            "npm x prettier",
            "npx prettier",
            "pnpm dlx prettier",
            "pnpm exec prettier",
            "pnpm prettier",
            "pnpm run prettier",
            "pnpm run-script prettier",
            "pnpx prettier",
            "prettier",
        ],
        category: "Build",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?next[ \t\n]+build(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk next",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "next build",
            "npm exec next build",
            "npm next build",
            "npm rum next build",
            "npm run next build",
            "npm run-script next build",
            "npm urn next build",
            "npm x next build",
            "npx next build",
            "pnpm dlx next build",
            "pnpm exec next build",
            "pnpm next build",
            "pnpm run next build",
            "pnpm run-script next build",
            "pnpx next build",
        ],
        category: "Build",
        savings_pct: 87.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?jest([ \t\n]+run)?([ \t\n]|$)",
        rtk_cmd: "rtk jest",
        rewrite_prefixes: &[
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm rum jest run",
            "npm rum jest",
            "npm run jest run",
            "npm run jest",
            "npm run-script jest run",
            "npm run-script jest",
            "npm urn jest run",
            "npm urn jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpm run jest run",
            "pnpm run jest",
            "pnpm run-script jest run",
            "pnpm run-script jest",
            "pnpx jest run",
            "pnpx jest",
        ],
        category: "Tests",
        savings_pct: 99.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?vitest([ \t\n]+run)?([ \t\n]|$)",
        rtk_cmd: "rtk vitest",
        rewrite_prefixes: &[
            "npm exec vitest run",
            "npm exec vitest",
            "npm rum vitest run",
            "npm rum vitest",
            "npm run vitest run",
            "npm run vitest",
            "npm run-script vitest run",
            "npm run-script vitest",
            "npm urn vitest run",
            "npm urn vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm run vitest run",
            "pnpm run vitest",
            "pnpm run-script vitest run",
            "pnpm run-script vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ],
        category: "Tests",
        savings_pct: 99.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ctest(?:[ \t\n]|$)",
        rtk_cmd: "rtk ctest",
        rewrite_prefixes: &["ctest"],
        category: "Tests",
        savings_pct: 80.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?playwright(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk playwright",
        rewrite_prefixes: &[
            "npm exec playwright",
            "npm playwright",
            "npm rum playwright",
            "npm run playwright",
            "npm run-script playwright",
            "npm urn playwright",
            "npm x playwright",
            "npx playwright",
            "playwright",
            "pnpm dlx playwright",
            "pnpm exec playwright",
            "pnpm playwright",
            "pnpm run playwright",
            "pnpm run-script playwright",
            "pnpx playwright",
        ],
        category: "Tests",
        savings_pct: 94.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^((p?np(m|x)|p?npm[ \t\n]+(exec|run|run-script)|npm[ \t\n]+(rum|urn|x)|pnpm[ \t\n]+dlx)[ \t\n]+)?prisma(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk prisma",
        rewrite_prefixes: &[
            "npm exec prisma",
            "npm prisma",
            "npm rum prisma",
            "npm run prisma",
            "npm run-script prisma",
            "npm urn prisma",
            "npm x prisma",
            "npx prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm prisma",
            "pnpm run prisma",
            "pnpm run-script prisma",
            "pnpx prisma",
            "prisma",
        ],
        category: "Build",
        savings_pct: 88.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^docker[ \t\n]+(ps|images|logs|run|exec|build|compose[ \t\n]+(ps|logs|build))(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk docker",
        rewrite_prefixes: &["docker"],
        category: "Infra",
        savings_pct: 85.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^kubectl[ \t\n]+(get|logs|describe|apply)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk kubectl",
        rewrite_prefixes: &["kubectl"],
        category: "Infra",
        savings_pct: 85.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^oc[ \t\n]+(get|logs|describe|apply|status|adm)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk oc",
        rewrite_prefixes: &["oc"],
        category: "Infra",
        savings_pct: 85.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^tree([ \t\n]|$)",
        rtk_cmd: "rtk tree",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["tree"],
        category: "Files",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^diff[ \t\n]+",
        rtk_cmd: "rtk diff",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["diff"],
        category: "Files",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^curl[ \t\n]+",
        rtk_cmd: "rtk curl",
        rewrite_prefixes: &["curl"],
        category: "Network",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^wget[ \t\n]+",
        rtk_cmd: "rtk wget",
        rewrite_prefixes: &["wget"],
        category: "Network",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(python3?[ \t\n]+-m[ \t\n]+)?mypy([ \t\n]|$)",
        rtk_cmd: "rtk mypy",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["python3 -m mypy", "python -m mypy", "mypy"],
        category: "Build",
        savings_pct: 80.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ruff[ \t\n]+(check|format)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk ruff",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ruff"],
        category: "Python",
        savings_pct: 80.0,
        subcmd_savings: &[("check", 80.0), ("format", 75.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^sqlfluff[ \t\n]+lint(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk sqlfluff",
        rewrite_prefixes: &["sqlfluff"],
        category: "Python",
        savings_pct: 75.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(python[0-9.]*[ \t\n]+-m[ \t\n]+)?pytest([ \t\n]|$)",
        rtk_cmd: "rtk pytest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["python3 -m pytest", "python -m pytest", "pytest"],
        category: "Python",
        savings_pct: 90.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(pip3?|uv[ \t\n]+pip)[ \t\n]+(list|outdated|install|show)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk pip",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pip3", "pip", "uv pip"],
        category: "Python",
        savings_pct: 75.0,
        subcmd_savings: &[("list", 75.0), ("outdated", 80.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^uv[ \t\n]+run(?:[ \t\n]|$)",
        rtk_cmd: "rtk uv",
        rewrite_prefixes: &["uv"],
        category: "Python",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^go[ \t\n]+(test|build|vet)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk go",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["go"],
        category: "Go",
        savings_pct: 85.0,
        subcmd_savings: &[("test", 90.0), ("build", 80.0), ("vet", 75.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:golangci-lint|golangci)[ \t\n]+(run)(?:[ \t\n]|$)",
        rtk_cmd: "rtk golangci-lint run",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["golangci-lint run", "golangci run"],
        category: "Go",
        savings_pct: 85.0,
        ..RtkRule::DEFAULT
    },
    // Scala/SBT
    RtkRule {
        pattern: r#"^sbt[ \t\n]+["']?(testOnly|testQuick|test|compile|run|clean|assembly|package)(?:[ \t\n"']|$)"#,
        rtk_cmd: "rtk sbt",
        rewrite_prefixes: &["sbt"],
        category: "Build",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("testOnly", 90.0), ("compile", 75.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^bundle[ \t\n]+(install|update)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk bundle",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle"],
        category: "Ruby",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:bundle[ \t\n]+exec[ \t\n]+)?(?:bin/)?(?:rake|rails)[ \t\n]+test(?:[ \t\n:]|$|[;|&()<>])",
        rtk_cmd: "rtk rake",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "bundle exec rails",
            "bundle exec rake",
            "bin/rails",
            "rails",
            "rake",
        ],
        category: "Ruby",
        savings_pct: 85.0,
        subcmd_savings: &[("test", 90.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:bundle[ \t\n]+exec[ \t\n]+)?rspec(?:[ \t\n]|$)",
        rtk_cmd: "rtk rspec",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle exec rspec", "bin/rspec", "rspec"],
        category: "Tests",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:bundle[ \t\n]+exec[ \t\n]+)?rubocop(?:[ \t\n]|$)",
        rtk_cmd: "rtk rubocop",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle exec rubocop", "rubocop"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    // PHP tooling
    RtkRule {
        pattern: r"^php[ \t\n]+artisan(?:[ \t\n]|$)",
        rtk_cmd: "rtk php",
        rewrite_prefixes: &["php"],
        category: "Build",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^php[ \t\n]+-l(?:[ \t\n]|$)",
        rtk_cmd: "rtk php",
        rewrite_prefixes: &["php"],
        category: "Build",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^php[ \t\n]+run-tests\.php(?:[ \t\n]|$)",
        rtk_cmd: "rtk phpt",
        rewrite_prefixes: &["php run-tests.php"],
        category: "Tests",
        savings_pct: 99.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:php[ \t\n]+)?(?:\./)?(?:(?:vendor/)?bin/)?phpunit(?:[ \t\n]|$)",
        rtk_cmd: "rtk phpunit",
        pipeline_safety: PipelineSafety::ProducerOnly,
        // rewrite_segment_inner normalizes the php wrapper, `./`, vendor/bin and
        // composer bin-dir before matching, so only the residual forms remain:
        // a plain `bin/` (not a Composer dir, so it survives normalization) and
        // the bare tool name.
        rewrite_prefixes: &["bin/phpunit", "phpunit"],
        category: "Tests",
        savings_pct: 75.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:php[ \t\n]+)?(?:\./)?(?:(?:vendor/)?bin/)?phpstan[ \t\n]+analy[sz]e(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk phpstan",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bin/phpstan", "phpstan"],
        category: "Build",
        savings_pct: 65.0,
        subcmd_savings: &[("analyse", 65.0), ("analyze", 65.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?pest(?:[ \t\n]|$)",
        rtk_cmd: "rtk pest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pest"],
        category: "Tests",
        savings_pct: 80.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?paratest(?:[ \t\n]|$)",
        rtk_cmd: "rtk paratest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["paratest"],
        category: "Tests",
        savings_pct: 80.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?ecs(?:[ \t\n]|$)",
        rtk_cmd: "rtk ecs",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ecs"],
        category: "Build",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?pint(?:[ \t\n]|$)",
        rtk_cmd: "rtk pint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pint"],
        category: "Build",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^aws[ \t\n]+",
        rtk_cmd: "rtk aws",
        rewrite_prefixes: &["aws"],
        category: "Infra",
        savings_pct: 80.0,
        subcmd_savings: &[
            ("sts", 80.0),
            ("s3", 60.0),
            ("ec2", 85.0),
            ("ecs", 90.0),
            ("rds", 80.0),
            ("cloudformation", 90.0),
            ("logs", 88.0),
            ("lambda", 90.0),
            ("iam", 85.0),
            ("dynamodb", 70.0),
            ("s3api", 75.0),
            ("eks", 87.0),
            ("sqs", 78.0),
            ("secretsmanager", 75.0),
        ],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^psql([ \t\n]|$)",
        rtk_cmd: "rtk psql",
        rewrite_prefixes: &["psql"],
        category: "Infra",
        savings_pct: 75.0,
        ..RtkRule::DEFAULT
    },
    // Bun/Deno
    RtkRule {
        pattern: r"^bun[ \t\n]+(install|add|remove|test|build|run|pm[ \t\n]+ls|pm|x)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk bun",
        rewrite_prefixes: &["bun"],
        category: "PackageManager",
        savings_pct: 75.0,
        subcmd_savings: &[("test", 90.0), ("install", 70.0), ("pm ls", 70.0)],
        // Audited against what each subcommand actually does. "pm ls" is
        // filtered and every other "bun pm" is passthrough, which is why the
        // pattern captures the two-word form separately. "build" writes its
        // bundle to stdout unless an output flag is present, so its common form
        // runs unfiltered and it cannot claim the headline number.
        subcmd_status: &[
            ("run", RtkStatus::Passthrough),
            ("pm", RtkStatus::Passthrough),
            ("build", RtkStatus::Passthrough),
        ],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^bunx[ \t\n]+",
        rtk_cmd: "rtk bunx",
        rewrite_prefixes: &["bunx"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^deno[ \t\n]+(test|lint|check|run|task|compile|install)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk deno",
        rewrite_prefixes: &["deno"],
        category: "Build",
        savings_pct: 75.0,
        // Measured against real deno 2.9.6 output: the lint and check filters
        // remove ANSI, download and blank lines and keep every diagnostic, so
        // they save bytes rather than content.
        subcmd_savings: &[("test", 90.0), ("lint", 40.0), ("check", 50.0)],
        // Audited alongside the bun rule above: test, lint and check are
        // filtered, the rest run unchanged.
        subcmd_status: &[
            ("run", RtkStatus::Passthrough),
            ("task", RtkStatus::Passthrough),
            ("install", RtkStatus::Passthrough),
            ("compile", RtkStatus::Passthrough),
        ],
        ..RtkRule::DEFAULT
    },
    // TOML-filtered commands
    RtkRule {
        pattern: r"^ansible-playbook(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk ansible-playbook",
        rewrite_prefixes: &["ansible-playbook"],
        category: "Infra",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^brew[ \t\n]+(install|upgrade)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk brew",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["brew"],
        category: "PackageManager",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^composer[ \t\n]+(install|update|require)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk composer",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["composer"],
        category: "PackageManager",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^df([ \t\n]|$)",
        rtk_cmd: "rtk df",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["df"],
        category: "System",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^dotnet[ \t\n]+build(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk dotnet",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["dotnet"],
        category: "Build",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^du(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk du",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["du"],
        category: "System",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^fail2ban-client(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk fail2ban-client",
        rewrite_prefixes: &["fail2ban-client"],
        category: "Infra",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^gcloud(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk gcloud",
        rewrite_prefixes: &["gcloud"],
        category: "Infra",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./gradlew|gradlew\.bat|gradlew|gradle)(?:[ \t\n]+(test|build|clean|assemble\w*|install\w*|check|lint\w*|dependencies))?([ \t\n]|$)",
        rtk_cmd: "rtk gradlew",
        rewrite_prefixes: &["./gradlew", "gradlew.bat", "gradlew", "gradle"],
        category: "Build",
        savings_pct: 75.0,
        subcmd_savings: &[("test", 90.0), ("build", 80.0), ("check", 80.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^hadolint(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk hadolint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["hadolint"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^helm(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk helm",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["helm"],
        category: "Infra",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^iptables(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk iptables",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["iptables"],
        category: "Infra",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^make(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk make",
        rewrite_prefixes: &["make"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^markdownlint(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk markdownlint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["markdownlint"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^mix[ \t\n]+(compile|format)([ \t\n]|$)",
        rtk_cmd: "rtk mix",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["mix"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^(?:\./mvnw|mvnw\.cmd|mvnw|mvn)(?:[ \t\n]+[^ \t\n]+)*?[ \t\n]+(compile|test-compile|test|integration-test|package|install|verify|deploy)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk mvn",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["./mvnw", "mvnw.cmd", "mvnw", "mvn"],
        category: "Build",
        savings_pct: 82.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        // `mvnd` is a separate binary, not a `mvn` wrapper — it must keep its
        // own rtk_cmd so the daemon is what actually runs. mvnd ships
        // `mvnd.cmd` on Windows; listed explicitly (longer prefix first) on
        // both the pattern and rewrite_prefixes, mirroring the mvn rule's
        // `mvnw.cmd` handling — `mvnd` alone stops at the `.` of `mvnd.cmd`,
        // where `[ \t\n]+(compile|...)` cannot follow, so the command would
        // not classify at all.
        pattern: r"^(?:mvnd\.cmd|mvnd)(?:[ \t\n]+[^ \t\n]+)*?[ \t\n]+(compile|test-compile|test|integration-test|package|install|verify|deploy)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk mvnd",
        rewrite_prefixes: &["mvnd.cmd", "mvnd"],
        category: "Build",
        savings_pct: 82.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ping(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk ping",
        rewrite_prefixes: &["ping"],
        category: "Network",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^pio[ \t\n]+run(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk pio",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pio"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^poetry[ \t\n]+(install|lock|update)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk poetry",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["poetry"],
        category: "Python",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^pre-commit(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk pre-commit",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pre-commit"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^ps([ \t\n]|$)",
        rtk_cmd: "rtk ps",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ps"],
        category: "System",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^pulumi[ \t\n]+(preview|up|destroy|refresh|stack)([ \t\n]|$)",
        rtk_cmd: "rtk pulumi",
        rewrite_prefixes: &["pulumi"],
        category: "Infra",
        savings_pct: 45.0,
        subcmd_savings: &[
            ("up", 66.0),
            ("destroy", 72.0),
            ("refresh", 35.0),
            ("preview", 25.0),
            ("stack", 29.0),
        ],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^quarto[ \t\n]+render(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk quarto",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["quarto"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^rsync(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk rsync",
        rewrite_prefixes: &["rsync"],
        category: "Network",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^shellcheck(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk shellcheck",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["shellcheck"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^shopify[ \t\n]+theme[ \t\n]+(push|pull)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk shopify",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["shopify"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^sops(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk sops",
        rewrite_prefixes: &["sops"],
        category: "Infra",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^swift[ \t\n]+(build|test)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk swift",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["swift"],
        category: "Build",
        savings_pct: 65.0,
        subcmd_savings: &[("test", 90.0)],
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^systemctl[ \t\n]+status(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk systemctl",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["systemctl"],
        category: "System",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^terraform[ \t\n]+plan(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk terraform",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["terraform"],
        category: "Infra",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^tofu[ \t\n]+(fmt|init|plan|validate)([ \t\n]|$)",
        rtk_cmd: "rtk tofu",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["tofu"],
        category: "Infra",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^trunk[ \t\n]+build(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk trunk",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["trunk"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^uv[ \t\n]+(sync|pip[ \t\n]+install)(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk uv",
        rewrite_prefixes: &["uv"],
        category: "Python",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^yamllint(?:[ \t\n]|$|[;|&()<>])",
        rtk_cmd: "rtk yamllint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["yamllint"],
        category: "Build",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^wc([ \t\n]|$)",
        rtk_cmd: "rtk wc",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["wc"],
        category: "Files",
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^gt[ \t\n]+",
        rtk_cmd: "rtk gt",
        rewrite_prefixes: &["gt"],
        category: "Git",
        savings_pct: 70.0,
        ..RtkRule::DEFAULT
    },
    RtkRule {
        pattern: r"^liquibase(?:[ \t\n]|$)",
        rtk_cmd: "rtk liquibase",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["liquibase"],
        category: "Infra",
        savings_pct: 65.0,
        ..RtkRule::DEFAULT
    },
];

pub const IGNORED_PREFIXES: &[&str] = &[
    "cd ",
    "cd\t",
    "echo ",
    "printf ",
    "export ",
    "source ",
    "mkdir ",
    "rm ",
    "mv ",
    "cp ",
    "chmod ",
    "chown ",
    "touch ",
    "which ",
    "type ",
    "test ",
    "true",
    "false",
    "sleep ",
    "wait",
    "kill ",
    "set ",
    "unset ",
    "sort ",
    "uniq ",
    "tr ",
    "cut ",
    "awk ",
    "sed ",
    "python3 -c",
    "python -c",
    "node -e",
    "ruby -e",
    "pwd",
    "bash ",
    "sh ",
    "then\n",
    "then ",
    "else\n",
    "else ",
    "do\n",
    "do ",
    "for ",
    "while ",
    "if ",
    "case ",
];

pub const IGNORED_EXACT: &[&str] = &[
    "cd", "echo", "true", "false", "wait", "pwd", "bash", "sh", "fi", "done",
];
