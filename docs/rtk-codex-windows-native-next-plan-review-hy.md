# 独立审查报告（第二次）：RTK Codex Windows Native Next Plan

**审查对象：** `docs/rtk-codex-windows-native-next-plan.md`（修订版，1257 行）
**前次报告：** `docs/rtk-codex-windows-native-next-plan-review.md`
**审查方式：** 独立、基于代码的二次核查。重新通读修订版全文，并对照源码二次验证：(1) 前次 8 项发现是否被修订吸收；(2) 修订新增的 C0.5 Windows 传输层的前提是否成立；(3) 修订是否引入回归。所有结论附 `文件:行号` 证据。
**审查结论：** 修订版**质量显著提升**——前次 8 项发现全部被吸收（多数处理得当）。但修订**引入一处高风险回归**（删除了 `which`/`pwd` 等对新子命令必需的 `RTK_META_COMMANDS` 注册，会直接导致已有测试 `test_every_subcommand_is_classified` 失败、CI 阻断），并新增若干 C0.5 传输层的实现细节缺口。修正回归 + 补齐 C0.5 细节后，计划可进入实施。

---

## 1. 总体评价

| 维度 | 第一次 | 第二次（修订版） | 变化 |
|------|--------|------------------|------|
| 证据驱动 | 良好 | 良好（新增 `--codex-path` 覆盖与 `--check-provider` 诊断） | 改善 |
| 优先级排序 | 良好 | 良好（C0.5 前置为 P0） | 改善 |
| 安全模型 | 优秀 | 优秀（新增传输/优化二元区分、Unix 回归防护） | 维持 |
| 结构假设准确性 | 基本准确 | 准确（C0.5 前提经代码验证成立） | 改善 |
| 事实前提 | 需修正 3 处 | 原 3 处已修正；但**引入 1 处回归** | 变差→需再修 |
| 架构一致性 | 有风险 | 风险已消解（`PowerShellRewrite` 取消、复用 `Classification`；Get-ChildItem argv 统一） | 改善 |
| 测试覆盖 | 全面 | 更全面（双 surface 一致性、C0.5 引用测试、schema 探针） | 改善 |

**建议：** 修订方向正确，吸收了我前次几乎所有意见。但**必须恢复 `which`/`pwd`/`touch`/`mkdir`（及可能采用的 `head`/`tail`）向 `RTK_META_COMMANDS` 的注册**，并补齐 C0.5 的"未知命令在 PowerShellTransport 与 DirectExternal 间的判别"与"传输主机解析"两处实现细节。修正后即可实施，P0/P1 顺序保持不变。

---

## 2. 前次 8 项发现的处置结果

| # | 前次发现 | 修订版处置 | 评价 |
|---|----------|------------|------|
| 1 | "`rtk Get-Content` 当前走 PowerShell fallback" 不成立，实际会失败 127 | 修订版未沿用该错误表述；新增 C0.5，**准确**描述为"当前经 `Commands::Other` 的 `powershell -Command` 包装执行（可运行简单 cmdlet，但会丢失引号/argv 语义）"。见 2.1 "Why"（153 行）。**注意：经代码复核，我前次的"失败 127"判断是基于错误假设（我以为未知子命令走 `run_fallback`）；实际 `Commands::Other` 是 catch-all 变体，会把 `rtk Get-Content` 包进 `powershell -Command` 而成功执行）。修订版的当前表述才是正确的。** | 已修正（且暴露了我前次判断偏差） |
| 2 | head/tail 已被 `rewrite_line_range` 覆盖，`rtk read --max-lines/--tail-lines` 已原生可用 | 1.4 实现注记（116 行）、3.6/3.7 "Goal" 与 step 0 均明确"优先扩展 `rewrite_line_range`，仅当 Codex 确实发 `rtk head` 时才加直链"，并禁止重复行窗口逻辑。 | 已修正（超出预期） |
| 3 | run 函数返回类型异构（`Result<()>` vs `Result<i32>`） | "Cross-cutting conventions"（335 行）显式要求归一化为单一 `Result<i32>`；3.2 step 4 逐一点名 `read::run`/`find_cmd::run_from_args`→`Result<()>`，`search::run`/`ls::run`→`Result<i32>`。 | 已修正 |
| 4 | Get-ChildItem argv 自相矛盾 | 3.4 step 3/4 与 supported-shapes（640 行）、rewrite 测试（668 行）**统一为 `rtk find src -name *.rs`**；并显式"不要与 `rtk find <glob> <path>` 混用"。 | 已修正 |
| 5 | `PowerShellRewrite` 与 `Classification` 重复 | 3.2 step 1 改为"返回现有 `Classification` 数据与 rewrite 字符串，**不创建第二套并行分类管线**"。`PowerShellRewrite` 结构已删除。 | 已修正（架构更干净） |
| 6 | 缺双 surface 一致性测试 | 新增 `direct_and_rewrite_get_content_match`（535）、`direct_and_rewrite_get_child_item_find_match`（672）、head/tail 一致性测试（3.6 step 6 / 3.7 step 6）。 | 已修正 |
| 7 | Codex schema 未验证 | 新增 `--codex-path`、`candidate_db_paths` 多候选、以及 `--check-provider` 诊断模式（438 行）与 `codex_check_provider_reports_schema` 测试（459 行），并要求 schema 探针测试。 | 已修正（强） |
| 8 | `search::run` 默认值未指定 | 3.3 step 4 改为"使用 `Commands::Grep` 分发臂的同一组 limit 值，不要新默认值"。 | 已修正 |

**小结：** 前次意见被认真采纳，修订质量明显提升。以下聚焦**修订引入的新问题**与 **C0.5 传输层评估**。

---

## 3. 新发现（修订引入 / C0.5 专项）

### 3.1 【高风险回归】删除新子命令的 `RTK_META_COMMANDS` 注册，会破坏已有测试

**证据：**
- 修订版 3.5（721 行）"Do not add `which` to `RTK_META_COMMANDS`…"；3.8（916 行）"Do not add `pwd` to `RTK_META_COMMANDS`…"。`head`/`tail`/`touch`/`mkdir` 的 Files 表与步骤中**也不再提** `RTK_META_COMMANDS`（原版在 6 处均要求加入）。
- 但源码 `src/main.rs:3029` 的 `test_every_subcommand_is_classified` 断言：**每一个 `Commands` 变体名必须位于 `RTK_META_COMMANDS` 或 `PASSTHROUGH`**，否则测试失败（`main.rs:3096-3101` 报错 "unclassified subcommand(s)"）。
- `PASSTHROUGH`（main.rs:3032-3086）是"包装真实外部工具"的命令清单（`ls`/`git`/`grep`/`cargo`…）。`which`/`pwd`/`touch`/`mkdir` 是 **RTK 原生实现**（不包装外部二进制），**不属 `PASSTHROUGH`**，因此**必须进入 `RTK_META_COMMANDS`**。

**为什么修订版会错：** 其理由是"除非你刻意想让 `rtk which --badarg` fail-closed，否则不要加"。这误读了 `RTK_META_COMMANDS` 的用途——该常量不是"坏参数 fail-closed 开关"（坏参数由各变体的 clap 定义天然处理），而是**全量子命令注册表**，供 `test_every_subcommand_is_classified` 做 fail-closed 分类校验。漏注册会直接让已有单元测试失败、CI 红灯。

**影响：** 高。只要新增 `Which`/`Pwd`（以及若采用直链的 `Head`/`Tail`、`Touch`、`Mkdir`）变体而不注册 `RTK_META_COMMANDS`，`cargo test` 立即失败，整批无法合入。

**修正：** 恢复原版动作——在 3.5/3.8/3.6/3.7/3.9/3.10 中明确"将新变体加入 `RTK_META_COMMANDS`"，并给出正确理由："`test_every_subcommand_is_classified` 要求每个 `Commands` 变体登记于 `RTK_META_COMMANDS` 或 `PASSTHROUGH`；`which`/`pwd` 等为 RTK 原生、无对应外部二进制，故入 `RTK_META_COMMANDS`。" 顺带：原版 3.5 step 4 的"加 `which` 到 `RTK_META_COMMANDS`"动作正确，修订版把它删掉是回归。

### 3.2 【已验证·修正我前次偏差】C0.5 前提成立

二次核对确认 C0.5 要修的 bug 真实存在：
- `Commands::Other`（main.rs:2310）是 catch-all 变体，`rtk <未知>` 会被它捕获；其 Windows 分支（2319-2320）执行 `args.join(" ")` 再 `powershell -Command <raw>`。
- `Commands::Run`（2383）同样在 Windows 用 `powershell -Command`（2394）。
- 这确实会丢失引号/argv 语义（把 argv 向量拼成字符串再让 PowerShell 重新解析）。例如 `rtk powershell -NoProfile -Command "Get-ChildItem | Where-Object { $_.Name -match 'src' }"` 会被二次包装、管道与 `$_` 语义脆弱。

**结论：** C0.5（Windows 传输层）解决的问题真实、必要，且修订版的修复方向（`is_shell_host` 直接 argv 执行、`-EncodedCommand` 保真传输）正确。这也意味着**我前次报告第 3.1 条"当前失败 127"的判断是错的**——未知 cmdlet 实际经 `Commands::Other` 的 `powershell -Command` 包装成功执行。修订版当前表述准确，特此更正。

### 3.3 【中】C0.5 未明确"未知命令在 PowerShellTransport 与 DirectExternal 间如何判别"

`WindowsFallbackDecision`（357 行）列出 `PowerShellTransport` 与 `DirectExternal` 两个分支，但对**非 shell-host、非已知优化形状**的未知命令，应由谁落入哪支**未给出判别规则**。

**建议补充规则：** 先用 `which::which(args[0])` 解析；若可解析 → `DirectExternal`（真实 exe，直接 argv 执行，更干净）；若不可解析 → 视为 cmdlet/别名 → `PowerShellTransport`（`-EncodedCommand`）。否则实现者可能把真实 exe 误送进 `powershell -EncodedCommand`（tty/stdin 行为差异）或把 cmdlet 直接执行（失败）。建议在 3.0 增加该判别说明与测试（如 `unknown_exe_resolves_directly` / `unknown_cmdlet_uses_transport`）。

### 3.4 【中】PowerShellTransport 硬编码 `powershell`，未解析可用主机

3.0 step 7 写死 `powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand <encoded>`。但在仅装 `pwsh`（PowerShell 7，常见于 CI/容器）而无 `powershell` 的 Windows 环境，该路径会失败。

**建议：** 传输主机解析与 shell-host 检测共用同一逻辑——优先 `pwsh`，回退 `powershell`（或反之，按环境），而非硬编码。与 3.0 step 2 的 `is_shell_host` 列表（`powershell`/`pwsh`/`cmd`）保持一致。

### 3.5 【低】`base64` 依赖需先确认是否已有传递依赖

3.0（351 行）"若无可用直接依赖则新增 `base64`"。已确认 `Cargo.toml` 无 `base64` 直接依赖（grep 无结果）。它可能是某传递依赖。

**建议：** 先查 `Cargo.lock` 是否有 `base64`（传递）；若有，直接复用现有 crate/path，避免新增直接依赖与版本错位；若确无，再 `cargo add base64`。属低优先级，但应在开工第一步确认。

### 3.6 【低】C1 直链处理器与 C0.5 传输层的调用归属需显式化

计划两处都提到彼此（3.2 step 5"顶层 fallback 先查已知 PowerShell 命令，不支持则转 Windows 传输"；3.0 step 1 `run_other` 为入口），但**单一入口与调用顺序未在一处画清**。

**建议：** 在 3.0 或 3.2 增加一段明确的调用图：
`Commands::Other / Commands::Run / run_fallback` → `powershell_compat::intercept(args)`（C1，仅认 cmdlet 形状）→ 若 `Handled` 返回；若 `Unsupported/Unknown` → `windows_shell::run_other(args)`（C0.5：shell-host 直链 / 未知命令 DirectExternal 或 PowerShellTransport）。
并明确边界：`powershell_compat` 只认 cmdlet（Get-Content 等），**不认** shell-host（`powershell`/`pwsh`/`cmd`），后者交给 `windows_shell`。这样避免双重处理与歧义。

### 3.7 【低】head/tail 若采用直链也必须入 `RTK_META_COMMANDS`

3.6/3.7 把直链设为"条件选项"。一旦采用，新增的 `Head`/`Tail` 变体同样受 3.1 的 `test_every_subcommand_is_classified` 约束，必须入 `RTK_META_COMMANDS`。建议在 3.6/3.7 条件分支里补一句"若选择直链，则将 `Head`/`Tail` 注册进 `RTK_META_COMMANDS`"，与 3.1 的发现一致。

---

## 4. 修订版相较前版的明确优点（应予肯定）

1. **新增 C0.5 传输层**：把"传输安全"与"语义优化"两类问题单列（0.3），并先修 transport 再上加语义重写——顺序合理，且用 `WindowsFallbackDecision` 枚举清晰建模。
2. **Unix 回归防护到位**：`#[cfg(windows)]` 门控（3.0 step 8/10）、`non_windows_fallback_remains_sh_c` 测试（390 行）、Unix 回归验证命令（402 行），直接对应第 7 节"High"风险项。
3. **安全模型升级**：第 5 节新增"transport-only 不得声称语义优化"、第 8 节 DoD 新增第 6/7 条（传输/优化可区分、无二次 `powershell -Command` 包装），均可验证。
4. **C0 诊断能力**：`--check-provider` 直接消解我前次"schema 未验证导致静默 0 sessions"的担忧。
5. **双 surface 一致性测试**：消除了 rewrite 与 direct handler 漂移风险。

---

## 5. 给作者的修订清单（按优先级）

| # | 类型 | 位置 | 动作 |
|---|------|------|------|
| 1 | **回归（高）** | 3.5(721) / 3.8(916) / 3.6 / 3.7 / 3.9 / 3.10 | 恢复"将新变体（`which`/`pwd`，及若采用的 `head`/`tail`/`touch`/`mkdir`）加入 `RTK_META_COMMANDS`"。理由：`test_every_subcommand_is_classified`（main.rs:3029）强制要求。删注册会令 `cargo test` 失败。 |
| 2 | 实现细节（中） | 3.0 | 明确未知命令在 `PowerShellTransport` 与 `DirectExternal` 间的判别（建议：先 `which::which` 解析，可解析→DirectExternal，否则→PowerShellTransport），并加测试。 |
| 3 | 实现细节（中） | 3.0 step 7 | `PowerShellTransport` 不要硬编码 `powershell`；与 `is_shell_host` 共用主机解析（pwsh/powershell）。 |
| 4 | 依赖（低） | 3.0(351) | 先查 `Cargo.lock` 是否有传递 `base64`；有则复用，无再加。 |
| 5 | 架构（低） | 3.0 / 3.2 | 在一处写明单一调用图：`顶层 → powershell_compat::intercept（仅 cmdlet）→ 不支持则 windows_shell::run_other`；明确 `powershell_compat` 不认 shell-host。 |
| 6 | 一致性（低） | 3.6 / 3.7 | 若选直链，补"将 `Head`/`Tail` 注册进 `RTK_META_COMMANDS`"。 |

---

## 6. 结论

修订版是一次**实质性的质量提升**：准确吸收了我前次 8 项发现中的全部要点，并新增了架构合理、测试严密的 C0.5 传输层与更强的 Codex 诊断能力。其当前表述（含 C0.5 前提）已**经源码二次验证为准确**——这也反过来纠正了我前次报告中"Get-Content 当前失败 127"的判断偏差（实际经 `Commands::Other` 的 `powershell -Command` 包装成功执行）。

唯一阻断性问题是**第 3.1 条的回归**：删除 `RTK_META_COMMANDS` 注册会让已有的 fail-closed 分类测试失败。该问题一行即可修正（恢复注册），且无架构争议。

**建议：按第 5 节清单修订后进入实施。优先级顺序（C0.5-P0 → C1-P0/P1 → C2-P2/P3）保持不变，评估为可执行。**
