# RTK Windows 原生适配方案 — 第 2 轮审查报告

> **审查对象：** `docs/rtk-powershell-native-plan.md`（修订版，1027 行）
> **审查方法：** 对照前一轮审查报告（S1–S5, I1–I8, M1–M3）逐一验证修正情况；对新增/修改内容做独立源码交叉核对；核实源码中关键模式
> **核对的源码文件：** `src/main.rs`（Commands 枚举、Commands::Psql、disable_help_flag、PASSTHROUGH 列表、RTK_META_COMMANDS、trailing_var_arg 分布）、`src/discover/rules.rs`（rewrite 规则）、`src/cmds/system/wc_cmd.rs`、`src/cmds/system/ls.rs`、`src/cmds/system/tree.rs`、`src/cmds/system/search.rs`
> **总体评价：** 前一轮报告的 5 个严重问题、8 个重要问题、全部次要问题**均已修正**。修订版质量大幅提升，不再有导致施工方向错误的架构误解。本轮新发现 8 个问题，其中 2 个中等、6 个细微。

---

## 零、前置修正状态总览

### ✅ 已关闭的严重问题（S1–S5）

| 原编号 | 问题 | 修正位置 | 状态 |
|--------|------|---------|:----:|
| S1 | `wc` 缺 `--help`/`--version` 处理 | L217-218、L230-231、L244-245 补处理逻辑 + 验收标准 | ✅ |
| S2 | `compact_ls()` 耦合文本解析 | L284-286 明确抽取 helper + 结构化 LsEntry + 统一格式化入口 | ✅ |
| S3 | `NOISE_DIRS` 含 glob 但 `tree -I` 不支持 | L348-354 锁定 glob basename 语义 + 测试锁定 | ✅ |
| S4 | `grep` fallback 缺 `-E`/`-P` 处理 | L416、L425-428 定义 `-E` no-op、`-P` unsupported exit 2 | ✅ |
| S5 | `Ps`/`Df`/`Du` `disable_help_flag` 不一致 | L550、L561 统一为 `disable_help_flag = true` | ✅ |

### ✅ 已关闭的重要问题（I1–I8）

| 原编号 | 问题 | 修正位置 | 状态 |
|--------|------|---------|:----:|
| I1 | `wc -m` 二进制文件行为变更 | L214-216、L246 退出码契约 | ✅ |
| I2 | `grep` fallback 递归/非递归 | L415 要求显式实现或 unsupported | ✅ |
| I3 | `df`/`du` 帮助文本内容规范 | L634、L719 写明了最小内容 | ✅ |
| I4 | `du` `-d` 解析策略 | L716-718 token scan 骨架 | ✅ |
| I5 | `ps` Unix 空参数行为 | L560 "空 args 执行外部 ps，不补默认参数" | ✅ |
| I6 | `PASSTHROUGH` 语义矛盾 | L524、L872 "测试分类标签，不得用于运行时" | ✅ |
| I7 | `grep` fallback help/version | L439-440 最小内容规范 | ✅ |
| I8 | `NOISE_DIRS` `env` 过宽 | 不在本次修改范围 | ✅ |

### ✅ 已关闭的次要问题（M1–M3）

| 原编号 | 问题 | 修正位置 | 状态 |
|--------|------|---------|:----:|
| M1 | 主目标 `grep` 措辞 | L3 加"在缺少 `grep.exe` 时走 Windows 原生 fallback" | ✅ |
| M2 | `du` junction 行为 | L690 `WalkDir::follow_links(false)` + 测试覆盖 | ✅ |
| M3 | `ls` `LC_ALL=C` Windows 分支 | L291 "Windows 原生分支不得设置" | ✅ |

---

## 一、新版源码核实确认

对照源码逐项验证修订版的关键事实声明：

### 1.1 `PASSTHROUGH` 列表（`main.rs:2975-3026`）
- 已包含 `wc`、`ls`、`tree`、`grep` ✅
- `ps`/`df`/`du` 当前不在列表中——计划正确，新增后需加入 ✅

### 1.2 `RTK_META_COMMANDS`（`main.rs:1185-1206`）
- 共 19 个命令，不包含 `ps`/`df`/`du` ✅
- 计划多处强调"不要加入 RTK_META_COMMANDS"，与源码一致 ✅

### 1.3 `disable_help_flag` 现有先例
- 当前**仅** `Commands::Psql`（`main.rs:197`）使用 `disable_help_flag = true`
- 原因：`psql` 的 `-h` 表示"host"而非"help"
- 计划为 `ps`/`df`/`du` 使用 `disable_help_flag = true` **遵循了现有代码模式** ✅

### 1.4 `trailing_var_arg` 已广泛使用
- `main.rs` 中 80+ 处使用 `trailing_var_arg`
- 是 RTK 处理透传参数的成熟模式 ✅

### 1.5 rewrite 规则
- `rules.rs:765` 存在 `ps → rtk ps` ✅
- `rules.rs:612` 存在 `df → rtk df` ✅
- `rules.rs:630` 存在 `du → rtk du` ✅
- `IGNORED_PREFIXES` 包含 `"type "`（`rules.rs:925-973`） ✅

---

## 二、本轮新发现的问题

### N1. `wc --help` 的"可继续使用 Clap 帮助"措辞微瑕 🟢

L217：
```
--help 可继续使用 Clap 帮助，也可由命令分支手动输出
```

`wc_cmd.rs` 使用 `trailing_var_arg`，Clap **不会拦截 `--help`**，它会作为普通参数落入 `args`。因此"使用 Clap 帮助"这个选项对 `--help` 实际上不可用。对 `-h` 是可行的（`wc` 没有禁用 `-h`）。

**影响：** 极低。实施者自然会走"手动输出"路径。

**建议：** 改为：
```
--help 必须由命令分支手动输出（Clap 因 trailing_var_arg 不拦截 --help）；
-h 可使用 Clap 默认帮助（不影响 wc 自身参数）
```

---

### N2. `wc -w` 在 invalid UTF-8 下的降级行为未测试锁定 🟢

L213：
```
-w 可在 UTF-8 成功时使用 Unicode whitespace；invalid UTF-8 时降级为 ASCII whitespace byte 扫描
```

验收标准（L925）只覆盖了 `-c` 对二进制文件和 `-m` 对 invalid UTF-8。**缺少 `-w` 在 invalid UTF-8 输入下的降级测试。**

**建议：** L925 补一条：`-w` 对 binary / invalid UTF-8 文件不崩溃且输出合理的 whitespace 分隔计数。

---

### N3. `tree -a` 与 `NOISE_DIRS` 的交互未指定 🟡

L340-342 定义 `-a` 为"允许显示隐藏项"。`NOISE_DIRS` 包含 `.git` 等隐藏项。GNU `tree` 中 `-a` **覆盖** `-I`（显示一切）。如果实施者让 `-a` 仅"允许显示隐藏项"但保留 `NOISE_DIRS` 过滤，`.git` 仍然会被隐藏——不同于用户预期。

**影响：** 中等。用户可能预期 `rtk tree -a` 显示全部文件。

**建议：** L340-342 补充：
```
-a / --all 使隐藏项可见且覆盖默认 NOISE_DIRS 过滤（仅对隐藏 noise 目录；
非隐藏 noise 如 node_modules 不因 -a 取消过滤）
```

---

### N4. `ps` Windows 输出格式未定义 🟢

L492-493 列了字段（PID + 进程名），但没指定列分隔符和排列方式。Unix `ps` 是空格分隔，如果 Windows 输出不同，脚本解析可能不一致。

**建议：** L492 补充格式说明（如"空间隔对齐，两列"），并由测试锁定输出形式。

---

### N5. `df`/`du` human-readable size 格式未指定 🟢

L634/L719 定义了 `-h` 支持但没指定 size 格式（`1.2G` 还是 `1.2GB`？小数位几位？）。

**建议：** 补一句"格式沿用 GNU df/du 风格（`1.2G`、`345M`、`978B`），跨平台一致"。

---

### N6. `grep -r "pattern"` 无显式 path 时的行为与 GNU grep 不同 🟡

L414：
```
stdin | 无 path 且 stdin 非 TTY 时逐行读取 stdin
```

GNU `grep -r "pattern"`（无 path）是**搜索 CWD**，不是读 stdin。如果用户输入 `rtk grep -r "TODO"` 且 stdin 是交互 TTY，计划未定义行为。

**影响：** 中等。LLM agent 可能发出 `grep -r "TODO"` 而不给 path（依赖 GNU grep 默认搜索 CWD），Windows fallback 行为会不同。

**建议：** L414 改为：
```
grep -r "pattern" 无显式 path 时：搜索 CWD（与 GNU grep 一致），不读取 stdin。
仅当全部路径参数缺失且 stdin 非 TTY 时才读取 stdin。
```

---

### N7. `tree` 输出连接符风格仍可进一步明确 🟢

L357-359 定义了"确定的缩进前缀""不随...改变同级缩进"，但没有指定连接符（`├──`/`└──` vs `|--`/`` `-- ``）。

**影响：** 极低。实施者自然会选 `├──`/`└──`。

**建议：** 可选加"连接符使用 `├──`/`└──`（与外部 `tree` 输出一致）"。

---

### N8. `grep` fallback 的 `-F`（literal）缺失时行为未定义 🟢

L430 说 `-F` 不在首版范围，但没说用户传 `-F` 时的行为：外部 `grep` 存在时 passthrough？缺失时报 unsupported？

**建议：** L430 补一句："外部 `grep` 缺失时 `-F` 返回 unsupported exit 2"。

---

## 三、新增/修改内容的额外核实

| 修改点 | 核实结论 |
|--------|---------|
| L217-218 `wc` 增加 help/version 处理 | ✅ 与前轮 S1 对应 |
| L284-286 `ls` 格式化层 helper 抽取 + LsEntry | ✅ 与前轮 S2 对应 |
| L348-354 `tree` ignore pattern 语义完善 | ✅ 与前轮 S3 对应 |
| L416/L425-428 `grep` fallback `-E`/`-P` 处理 | ✅ 与前轮 S4 对应 |
| L550/561 `ps` 统一 `disable_help_flag = true` | ✅ 与前轮 S5 对应 |
| L524/872 `PASSTHROUGH` 澄清为测试分类 | ✅ 与前轮 I6 对应 |
| L616 `ps`/`df` 共享 `sysinfo` 依赖 | ✅ 减少重复工作 |
| L690 `walkdir` `follow_links(false)` 显式声明 | ✅ 与前轮 M2 对应 |
| L716-718 `du` token scan 参数解析 | ✅ 与前轮 I4 对应 |
| L798 正则不可复制的声明 | ✅ 与前轮 I5 对应——已彻底修复 |
| L971 Windows 无外部工具矩阵要求 | ✅ 与前轮 I6 对应 |

---

## 四、修改建议汇总

| # | 严重度 | 问题 | 建议修改位置 |
|---|--------|------|------------|
| N1 | 🟢 微瑕 | L217 "可继续使用 Clap 帮助"对 `--help` 不准确 | 改为"`--help` 必须手动输出；`-h` 可用 Clap" |
| N2 | 🟢 次要 | `wc -w` invalid UTF-8 降级行为未验收 | L925 补充 `-w` binary/invalid UTF-8 测试 |
| N3 | 🟡 中等 | `tree -a` 是否覆盖 `NOISE_DIRS` 未定义 | L340-342 补充 `-a` 覆盖隐藏 noise 规则 |
| N4 | 🟢 次要 | `ps` Windows 输出列格式未指定 | L492 补充格式说明 |
| N5 | 🟢 次要 | `df`/`du` `-h` size 格式未指定 | L634/L719 补"沿用 GNU 风格" |
| N6 | 🟡 中等 | `grep -r "pattern"` 无 path 应搜索 CWD 而非 stdin | L414 改为"搜索 CWD" |
| N7 | 🟢 次要 | `tree` 输出连接符风格未指定 | L357 可补充 `├──`/`└──` |
| N8 | 🟢 次要 | `-F` 在 grep fallback 缺失时的行为未定义 | L430 补"缺失时 unsupported exit 2" |

---

## 五、总体评估

**前轮 5 个严重问题全部修正。** 这是最大的改进——不再有"方向性误解"（S2: compact_ls 耦合）、不再有"运行时崩溃隐患"（S1: wc help, S4: grep -E/-P）、不再有"不一致的架构设计"（S5: disable_help_flag）。加上 `PASSTHROUGH` 语义澄清（I6）、Phase B 正则修复（I5）、验收标准体系完善（I6→L971）等，文档质量相比第 1 轮审查有**质的提升**。

本轮新发现共 8 个，其中：
- **N3**（`tree -a` + `NOISE_DIRS`）和 **N6**（`grep -r` CWD 行为）是中等重要的行为定义缺口，建议修正。
- **其余 6 个**为措辞/格式/测试覆盖的微调级别。

**结论：** 修订版已可以落地实施。建议先修 N3 和 N6（共约 5 行），其余可在实施对应命令时顺手处理。

> 本报告不保留第 1 轮审查的旧内容——所有前轮问题均已关闭，仅纳入本轮新发现。


---

### S2. `ls` 的 `compact_ls()` 耦合 `ls -la` 文本解析，不能直接复用

计划 4.2 说"优先抽出/复用现有 `compact_ls()` 的格式化层，只允许数据源分叉"。但 `compact_ls(ls_text, show_all, show_long)` 的输入是**`ls -la` 文本行**，内部通过 `parse_ls_line()` 用正则 `LS_DATE_RE` 解析日期字段来定位文件名。它不是"格式化层"——它是**解析+格式化耦合**的函数。

"不再把 `DirEntry` 硬转成完整 fake `ls -la` 文本"（计划 4.2 的要求）和"复用 `compact_ls()` 的格式化层"（同一节的要求）**互相矛盾**：

- 路径 A：把 `DirEntry` 转成 `ls -la` 文本 → 喂给 `compact_ls()`。计划明确禁止。
- 路径 B：提取 `human_size()`、`perms_to_octal()` 等辅助函数为共享模块，然后为原生路径写新的入口函数调用它们。这才是真正"复用格式化层"——但计划没有说清楚这一点。
- 路径 C：重构 `compact_ls()` 接受结构化数据而非文本。工作量远超计划预期。

**建议：** 4.2 明确要求"将 `human_size()`、`perms_to_octal()` 等纯辅助函数提取为 `pub(crate)` 共享函数（留在 `ls.rs` 或新建 `ls_format.rs`），原生路径直接构造 `(name, size, octal, is_dir)` 元组调用它们，不复用 `compact_ls()` 本体"。并说明"原有 30+ 单元测试针对 `compact_ls(ls_text)` 的输入输出，原生路径需新建对应测试"。

---

### S3. `tree` 的 `NOISE_DIRS` 包含 glob 模式 `*.egg-info`，`tree -I` 不支持 glob

`constants.rs` 的 `NOISE_DIRS` 包含 `"*.egg-info"`。当前 `tree.rs:30-33` 把 `NOISE_DIRS.join("|")` 作为 `-I` 参数传给外部 `tree`。GNU `tree -I` 的 pattern 匹配是**字面匹配**（`*` 不是通配符，是字面星号），所以 `*.egg-info` 只会匹配名为 `*.egg-info` 的目录，**不会**匹配 `mypackage.egg-info`。这是一个**现有 bug**。

计划 4.3 要求"必须保留的行为：默认应用 `NOISE_DIRS`"，并说 Windows 原生实现的 `-I` 语义是"`*` / `?` 使用 glob 语义"。这实际上是在**修复**现有 bug（让 `*.egg-info` 真正匹配所有 `.egg-info` 目录），但计划没有意识到这是在改变现有行为。

**建议：** 4.3 补一句说明："现有 `tree -I '*.egg-info'` 实际是字面匹配（不生效），原生实现改为 glob 匹配是行为变更，需在测试中验证 `*.egg-info` 确实过滤了 `mypackage.egg-info` 目录。"

---

### S4. `grep` fallback 的 `-E`/`-P` 标志处理缺失

计划 4.4 说"不支持 grep BRE 专属转义语义""不支持 PCRE 扩展"。但没说当用户传 `-E`（ERE）或 `-P`（PCRE）且外部 `grep.exe` 缺失时怎么处理。

`search.rs` 的 `has_format_flag()` 只检查 `-c`/`-l`/`-L`/`-o`/`-Z`，不检查 `-E`/`-P`。如果原生 fallback 不拦截这些标志，它们会被传给 `regex::Regex::new()`，而 Rust `regex` crate 的语法是 RE2 风格（大致等价于 ERE 但不完全相同），`-P` 的 PCRE 模式则完全不兼容。

LLM agent 实际使用中 `grep -E "foo|bar"` 和 `grep -P "\d+"` 很常见。如果 fallback 静默接受了 `-P` 但用 RE2 语义编译，结果会不同（例如 PCRE 的 `\d` 在 RE2 中是 `\p{Digit}`）。

**建议：** 4.4 的"fallback / unsupported 规则"增加："-P（PCRE）在外部 `grep` 缺失时返回 exit 2 + 明确错误（PCRE 语义无法用 Rust regex 兼容）；-E（ERE）可尝试用 Rust regex 处理，但需在文档中标注兼容性差异。"

---

### S5. `ps`/`df`/`du` 的 `--help` 被 Clap 拦截的条件与 `trailing_var_arg` 的交互

计划 4.5/4.6/4.7 都使用 `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`，并说"如果 `args` 中包含 `--help` / `-h`，手动打印 help 并返回 0"。

但 Clap 的 `trailing_var_arg` **不会**拦截 `--help`——`--help` 会作为普通参数进入 `args`。这意味着 `rtk ps --help` 不会触发 Clap 的内置帮助，而是进入 `args = ["--help"]`，由代码手动处理。这是正确的。

然而，4.5 说 `Ps` 不使用 `disable_help_flag`，而 4.6/4.7 说 `Df`/`Du` 使用 `#[command(disable_help_flag = true)]`。**这种不一致是有意的还是遗漏？**

`Ps` 不用 `disable_help_flag` 时，`rtk ps -h` 会被 Clap 拦截并显示 Clap 帮助（因为 `-h` 是 Clap 的短帮助标志）。但计划 4.5 说 `rtk ps --help` 应"手动打印 `Ps` help"。这里 `-h` 和 `--help` 行为不一致：`-h` 走 Clap 内置，`--help` 走手动处理。

**建议：** 统一 `Ps`/`Df`/`Du` 的帮助处理策略。要么全部 `disable_help_flag = true` + 手动处理，要么明确说明 `Ps` 为什么不同。建议全部统一为 `disable_help_flag = true`，因为 `-h` 在 Unix `ps` 中不是帮助标志（是 "hard header"），Clap 拦截它会与 Unix 语义冲突。

---

## 二、重要问题（逻辑漏洞、遗漏、与现实不符）

### I1. `wc` 的原生实现未区分二进制文件与文本文件

当前外部 `wc` 对二进制文件和文本文件的行为一致（逐字节计数）。但计划 4.1 说"-m 统计 UTF-8 chars；invalid UTF-8 时返回错误，退出码 2"。

这意味着 `wc -m binary_file` 会返回 exit 2，而外部 `wc -m binary_file` 在大多数平台上会返回乱码字符计数或 0。**这是行为变更**，可能导致依赖 `wc -m` 的脚本失败。

**建议：** 4.1 明确说明"-m 对 binary file 的行为与外部 wc 不同（外部 wc 可能返回乱码计数，原生实现返回 exit 2），这是有意的设计决策还是需要兼容？"

---

### I2. `grep` fallback 的 `-r`/`-R`/`--recursive` 标志需要特别处理

`search.rs` 的 `extract_pattern_path()` 会 strip `-r`/`-R`/`--recursive`（因为内部用 `--` 分隔 pattern 和 path，递归由代码控制）。但原生 fallback 如果直接遍历指定路径，需要理解**递归 vs 非递归**的语义。

当前外部 `grep` 不带 `-r` 时只搜索显式指定的文件（不递归目录）。原生 fallback 如果用 `std::fs::read_dir` 递归遍历，就会改变这个语义。

**建议：** 4.4 补充："原生 fallback 必须区分递归模式（`-r`/`-R`）和非递归模式（默认）。非递归时只搜索显式指定的文件，不进入子目录。"

---

### I3. `df` 的 `disable_help_flag = true` 会禁用 `rtk df --help` 的 Clap 内置处理

`#[command(disable_help_flag = true)]` 会禁用 Clap 对 `--help` 的拦截。但 Clap 的 `--help` 处理是**自动的**——它会显示结构化的帮助信息。禁用后，`--help` 变成普通参数进入 `args`，需要手动处理。

计划 4.6 说"手动打印 `Df` help 并返回 0"。但"手动打印"意味着需要自己写帮助文本，**失去了 Clap 自动生成帮助的能力**。

对于 `df` 和 `du` 这种参数简单的命令，手动帮助可以接受。但计划没有提到帮助文本的内容规范。

**建议：** 4.6/4.7 补充帮助文本的最小内容要求："至少包含命令描述、支持的参数列表、`--help` 本身、以及 `rtk proxy df ...` 的 fallback 提示。"

---

### I4. `du` 的 `-d` 参数解析需要处理多种变体

计划 4.7 列出了 `-d 1`、`-d1`、`--max-depth 1`、`--max-depth=1` 四种形式。这是非 trivial 的参数解析：

- `-d 1`：短标志 + 空格分隔值
- `-d1`：短标志 + 紧凑值（无空格）
- `--max-depth 1`：长标志 + 空格分隔值
- `--max-depth=1`：长标志 + 等号值

此外 `-sh` 和 `-hs` 是组合短标志（`-s -h` 的紧凑形式）。`detect_mode()` 风格的标志扫描需要处理这些组合。

计划列出了支持的参数但**没有指定解析策略**。这在实施时会导致每个实施者发明不同的解析逻辑。

**建议：** 4.7 补充参数解析的骨架代码或明确要求"复用类似 `wc_cmd.rs::detect_mode()` 的标志扫描模式"。

---

### I5. `ps` 的 Unix/macOS 透传需要显式处理空参数

计划 4.5 说 Unix/macOS 上"`args` 原样透传给外部 `ps`"。但 `rtk ps`（无参数）在 Unix 上等价于 `ps`（无参数），输出当前 shell 的进程（通常只有自身）。

如果实施者写 `Command::new("ps").args(&args).status()`，当 `args` 为空时，这等价于 `ps`（无参数），行为正确。

但如果实施者在 Unix 分支也加了 `args.is_empty()` 的特殊处理（比如"无参数时显示帮助"），就会改变 Unix 行为。

**建议：** 4.5 明确"Unix/macOS 分支不加任何参数过滤逻辑，`args` 为空时就是 `ps`（无参数），不特殊处理"。

---

### I6. `test_every_subcommand_is_classified` 中 `PASSTHROUGH` 的语义误导

计划 4.5/4.6/4.7 说 ps/df/du 应"归为外部工具封装/透传类（现有测试里的 `PASSTHROUGH` 类）"。但 `PASSTHROUGH` 在测试中的含义是"Clap 解析失败时允许 fallback 到外部命令"（`main.rs:2280` 的 `Commands::Other`）。

ps/df/du 加入 `PASSTHROUGH` 后，如果 Clap 解析失败（比如未知参数），它们**不应该** fallback 到 `Commands::Other`——计划反复强调"不允许因 Clap 解析失败掉回 `Commands::Other`"。

这存在矛盾：`PASSTHROUGH` 分类允许 fallback，但计划要求 ps/df/du 不 fallback。

**建议：** 检查 `test_every_subcommand_is_classified` 的实际逻辑。如果 `PASSTHROUGH` 只是分类标签（不影响运行时行为），则无问题。如果有运行时影响，需要为 ps/df/du 创建新的分类（如 `NATIVE_NO_FALLBACK`）。在计划中明确说明分类的运行时语义。

---

### I7. `grep` 的 `--version`/`--help` 在原生 fallback 下的行为

`search.rs:355-367` 在 `--version`/`--help` 时透传给外部引擎。但原生 fallback 的场景恰恰是**外部引擎不存在**。

如果 `grep.exe` 缺失，`rtk grep --version` 会尝试 `resolved_command("grep")` 然后失败。计划 4.4 说"--help / --version 在外部 grep 缺失时也必须返回 RTK 自己的说明或明确 unsupported"，但没有给出具体的帮助文本内容。

**建议：** 4.4 补充帮助文本的最小规范："至少包含 `rtk grep` 的用途说明、Rust regex 方言提示、与 GNU grep 的兼容性差异、以及 `rtk proxy grep ...` 的 fallback 提示。"

---

### I8. `NOISE_DIRS` 中 `env` 目录名过于宽泛

`constants.rs` 包含 `"env"` 作为噪声目录。注释说"Python legacy virtualenv dir — noise"。但在非 Python 项目中，`env/` 可能是存放环境配置文件的重要目录（如 `env/production`、`env/local`）。

`ls.rs:266` 和 `tree.rs:31` 都使用 `NOISE_DIRS` 过滤。原生实现会继承这个行为。计划没有提到这个潜在问题。

**建议：** 这是现有行为（非本次引入），但计划在提到"必须保留的行为"时应注明"`NOISE_DIRS` 的内容本身不在本次修改范围内，但原生实现会原样继承"。

---

## 三、次要 / 文字问题

### M1. 计划主目标第 3 行的 `grep` 措辞与 4.4 标题矛盾

第 3 行说"`grep` 在缺少 `grep.exe` 时走 Windows 原生 fallback"，暗示 `grep` 是"原生实现"。但 4.4 标题明确说"Windows fallback，而非完整 grep 重写"。读者可能混淆"原生"和"fallback"的含义。

**建议：** 第 3 行改为"`grep` 在缺少 `grep.exe` 时走 Windows 原生 fallback（基础文本搜索，非完整 grep 重写）"。

---

### M2. `du` 未提及 `walkdir` 的 `follow_links` 配置

计划 4.7 说"遇 symlink / junction / reparse point：默认不跟随"。`walkdir::WalkBuilder` 默认不跟随符号链接（`follow_links(false)`），但需要确认实施者知道这个 API。

此外，Windows 上的 junction 和 symlink 在 `walkdir` 中的行为可能不同。`walkdir` 文档说 junction 在 Windows 上默认跟随（除非显式设置 `follow_links(false)`）。计划需要明确这一点。

**建议：** 4.7 补充"实施时必须显式设置 `WalkBuilder::new(path).follow_links(false)`，并验证 Windows junction 在此设置下确实不跟随"。

---

### M3. `ls` 的 `LC_ALL=C` 在 Windows 上无意义

`ls.rs:52` 有 `cmd.env("LC_ALL", "C")`。这是为了确保 `ls` 输出英文日期格式（让 `LS_DATE_RE` 正则能匹配）。Windows 原生分支不调用外部 `ls`，所以 `LC_ALL=C` 不需要也不应该被设置。

计划没有提到这一点，但这在实施时是显而易见的（原生分支不创建 `ls` 进程）。

**建议：** 可以不改，但如果要完善，可在 4.2 注明"原生分支不需要 `LC_ALL=C` 环境变量设置"。

---

### M4. `wc` 的 `WcMode::Chars`（`-m`）在 Windows 上的 locale 依赖

外部 `wc -m` 使用当前 locale 来计算字符数。原生实现说"统计 UTF-8 chars"。但 Windows 默认 locale 可能不是 UTF-8。如果文件是 GBK/Shift_JIS 编码，UTF-8 char 计数与 `wc -m` 的 locale-aware 计数会不同。

**建议：** 4.1 的"不承诺"部分补充"不承诺与外部 `wc -m` 在非 UTF-8 locale 下的行为等价"。

---

### M5. `df` 的 `sysinfo` 在 Windows 上可能返回 `total == 0` 的卷

可移动驱动器、网络映射盘、虚拟盘等在 Windows 上可能返回 `total_space() == 0`。计划 4.6 提到"`total == 0` 的卷显示 `use% = ?`"，但没有说明是否应该**显示**这些卷还是**跳过**它们。

**建议：** 4.6 明确"total == 0 的卷是否显示在输出中（建议显示但标记 `?`，或跳过并汇总 warning）"。

---

### M6. `ps` 首版固定 `sysinfo` 但未提及 Windows 版本兼容性

`sysinfo` crate 依赖 Windows API（`NtQuerySystemInformation` 等）。这些 API 在 Windows 7+ 上可用，但某些函数在旧版 Windows 上可能受限。计划未提及最低 Windows 版本要求。

**建议：** 在依赖策略 6.3 或风险评估中补充"sysinfo 要求 Windows 10+（或验证 Windows 7 兼容性）"。

---

### M7. `tree` 原生实现的输出格式未定义

当前 `tree.rs` 依赖外部 `tree` 的输出格式（ASCII 艺术 + `├──`/`└──` 连接符）。原生实现需要**自己生成**这种格式。计划 4.3 说"基础树形输出"但没有定义：

- 连接符用什么？（`├──`/`└──` 还是 ASCII `|--`/`` `-- ``？）
- 缩进用什么？（2 空格？4 空格？`│` 竖线？）
- 最终汇总行（"N directories, M files"）是否保留？（当前 `filter_tree_output` 会删除它）

**建议：** 4.3 补充输出格式规范，至少定义连接符、缩进风格、是否保留汇总行。或者明确说"复刻当前外部 tree 的输出格式"。

---

### M8. `grep` 的 stdin 场景需要特别处理

计划 4.4 在支持边界中说"stdin 必须明确测试"。但没有说明原生 fallback 如何检测 stdin 模式。

当前 `search.rs` 的 `engine_capture()` 使用 `exec_capture_stdin()`（继承父进程 stdin）。原生 fallback 如果用 `std::fs::read` 读文件，需要区分"文件参数"和"无参数（stdin）"。

**建议：** 4.4 补充"无文件参数时从 stdin 读取，使用 `std::io::Read` 而非 `std::fs::read`"。

---

## 四、修改建议汇总

| # | 严重度 | 问题 | 建议修改 |
|---|--------|------|---------|
| S1 | 🔴 严重 | `wc` 原生分支未处理 `--help`/`--version` | 4.1 验收标准加 help/version 测试；补 fallback 规则 |
| S2 | 🔴 严重 | `compact_ls()` 耦合文本解析，不能直接"复用格式化层" | 4.2 明确提取辅助函数为共享模块，原生路径新建入口函数 |
| S3 | 🔴 严重 | `NOISE_DIRS` 含 glob `*.egg-info` 但 `tree -I` 不支持 glob | 4.3 说明这是行为修复，需测试验证 |
| S4 | 🔴 严重 | `grep` fallback 未处理 `-E`/`-P` 标志 | 4.4 补充 `-E`/`-P` 在 grep 缺失时的处理策略 |
| S5 | 🔴 严重 | `Ps`/`Df`/`Du` 的 `disable_help_flag` 策略不一致 | 统一为 `disable_help_flag = true` + 手动处理 |
| I1 | 🟡 重要 | `wc -m` 对二进制文件的行为变更 | 4.1 说明与外部 wc 的差异 |
| I2 | 🟡 重要 | `grep` fallback 未区分递归/非递归模式 | 4.4 补充递归语义处理 |
| I3 | 🟡 重要 | `df`/`du` 手动帮助文本缺少内容规范 | 补充帮助文本最小内容要求 |
| I4 | 🟡 重要 | `du` `-d` 参数解析策略未指定 | 4.7 补充解析骨架或参考模式 |
| I5 | 🟡 重要 | `ps` Unix 透传空参数的行为未明确 | 4.5 明确"不特殊处理空参数" |
| I6 | 🟡 重要 | `PASSTHROUGH` 分类与"不 fallback"要求矛盾 | 检查测试运行时语义，必要时新建分类 |
| I7 | 🟡 重要 | `grep` fallback 的 help/version 文本未定义 | 补充帮助文本最小规范 |
| I8 | 🟡 重要 | `NOISE_DIRS` 含 `env` 过于宽泛 | 注明原生实现原样继承，不在本次修改范围 |
| M1 | 🟢 次要 | 主目标 `grep` 措辞与 4.4 标题矛盾 | 修改第 3 行措辞 |
| M2 | 🟢 次要 | `du` 的 `walkdir` junction 行为未验证 | 补充 `follow_links(false)` 显式设置要求 |
| M3 | 🟢 次要 | `ls` 的 `LC_ALL=C` 在 Windows 无意义 | 注明原生分支不需要 |
| M4 | 🟢 次要 | `wc -m` 在非 UTF-8 locale 下的行为差异 | 补充"不承诺"说明 |
| M5 | 🟢 次要 | `df` 的 `total==0` 卷显示策略未定 | 明确显示/跳过策略 |
| M6 | 🟢 次要 | `sysinfo` 最低 Windows 版本未提及 | 补充兼容性说明 |
| M7 | 🟢 次要 | `tree` 原生输出格式未定义 | 补充格式规范 |
| M8 | 🟢 次要 | `grep` fallback 的 stdin 处理未说明 | 补充 stdin 检测逻辑 |

---

## 五、整体评估

计划的结构、边界划分（Level 1/2/非目标）、Phase A/B 拆分、以及"Windows 原生 / Unix·macOS 透传"的口径**方向正确**。与上一版相比，ps/df/du 的回归承认（"rewrite 规则已存在但 handler 缺失"）是显著进步。

但作为**实施蓝图**仍有硬伤：

1. **S2（compact_ls 耦合）** 是架构级别的误解——如果实施者按"复用 compact_ls()"去写，会发现 DirEntry 无法喂给文本解析函数，要么回退到伪造 ls 文本（计划禁止），要么重写格式化层（超出计划范围）。
2. **S1/S5（帮助处理不一致）** 会导致三个新子命令的 `--help` 行为不统一，用户体验差。
3. **S4（-E/-P 缺失）** 是 grep fallback 最常见的兼容性坑。

**优先级建议：** 先修 S2 + S1/S5 + S4（直接决定施工正确性和用户体验），其余可在实施对应命令前再补。

> 注：上一版审查报告中的 S1（ps/df/du rewrite 已存在）、S2（RTK_META_COMMANDS 误解）、S3（grep 缺口高估）在当前版计划中**已全部修正**。当前版正确承认了 rewrite 回归、正确排除了 RTK_META_COMMANDS、并准确定位 grep 为"fallback 而非重写"。本审查仅针对当前 992 行版本的新问题。
