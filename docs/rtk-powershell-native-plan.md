# RTK Windows Shell 原生适配方案

> **主目标（必须达成）：** 让 RTK 在 Windows 上不依赖 Unix shell 工具即可运行关键 shell 类子命令：`ls` / `tree` / `wc` 走 Windows 原生实现，`grep` 在缺少 `grep.exe` 时走 Windows 原生 fallback，`ps` / `df` / `du` 新增正式 RTK 子命令并在 Windows 原生实现、Unix/macOS 继续透传外部工具。
>
> **次目标（可选增强）：** 在对应 RTK 子命令已完成 Windows 原生化、且参数语义明确兼容的前提下，再为少量 PowerShell 命令增加受限 rewrite 映射。

---

## 0. 目标分层与边界

### 0.1 Level 1：本计划必须完成的目标

在 Windows 上，使以下 RTK 子命令**不依赖 Unix 外部二进制**即可工作：

- `rtk wc`
- `rtk ls`
- `rtk tree`
- `rtk grep`
- `rtk ps`
- `rtk df`
- `rtk du`

其中 `rtk grep` 指 `Engine::Grep` 对应的 `grep` 子命令，不等同于 `rtk rg`。当前 `search.rs` 明确按用户调用的 engine 执行，不会在 `rtk grep` 缺少 `grep.exe` 时自动替换为 `rg.exe`。

### 0.2 Level 2：本计划只做受限设计，不承诺立即实现的目标

在 Level 1 完成后，为**参数可直接透传**或**可通过简单白名单约束**的 PowerShell 命令增加 rewrite：

- `dir`
- `Get-Process`
- `Get-Content <file>`（仅基础形式）

### 0.3 非目标（明确排除）

本计划**不**试图把 RTK 变成 PowerShell 解释器，也**不**承诺覆盖下列能力：

- PowerShell 对象管道语义（如 `Where-Object`、`ForEach-Object`）
- 通用 PowerShell 参数翻译层
- `Measure-Object` 的对象聚合语义
- `Compare-Object` 的对象集合比较语义
- `type` / `where` 等高歧义 alias 的自动接管
- “所有 PowerShell 内建命令 / cmdlet” 的完整支持

一句话边界：

> 本计划解决的是 **Windows 上 RTK 子命令的原生化**，不是 **PowerShell 语义完整迁移**。

---

## 一、背景与上下文

### 1.1 已完成的工作

RTK v0.42.4 已在 Windows 上完成编译适配和初步跑通：

| 事项 | 进展 |
|------|------|
| 编译 | VS 2022 BuildTools + Rust 1.96.1（stable-x86_64-pc-windows-msvc），`cargo check` 通过 |
| 构建 | `cargo build --release` 成功，产物 7.34 MB |
| 测试 | 2226 passed, 17 failed, 8 ignored（17 个失败均为预存：16 个 `echo` / `cat` 测试 + 1 个 uv rewrite 测试） |
| 部署 | `rtk.exe` 复制到 `D:\Program Files\MCPs\rtk\`（已在 PATH） |
| OpenCode | `rtk init --opencode --global` 安装插件到 `~/.config/opencode/plugins/rtk.ts` |
| 代码修改 | `src/main.rs`（`Commands::Other` catch-all + `powershell -Command` 兼容 + `uv` 补入 PASSTHROUGH）<br>`src/cmds/rust/runner.rs`（`cmd /C` → `powershell -Command`）<br>`src/cmds/python/uv_cmd.rs`（补全 `print_with_hint` 第 5 参） |

### 1.2 发现的问题

OpenCode 插件的工作方式是：对每个 `tool.execute.before`（即用户让 agent 执行的 bash / shell 命令），调用 `rtk rewrite <command>`，若返回重写则替换原命令。

**好的方面**：PowerShell 专有命令在当前 registry 中大多没有匹配，通常会 passthrough，原命令仍可执行。

**坏的方面**：RTK 能省 token 的部分 shell 风格子命令在 Windows 上无法工作，因为：

- 它们通过 `resolved_command()` 在 PATH 中查找对应二进制文件（`ls`、`grep`、`wc` 等）
- 这些二进制文件是 Unix 工具，Windows 默认环境通常不存在
- 找不到就直接报错退出

测试确认：

```text
> rtk ls C:\
rtk: Failed to resolve 'ls' via PATH, falling back to direct exec: Binary 'ls' not found on PATH
rtk: Failed to run ls: Failed to spawn process: program not found
```

### 1.3 本次计划要解决的真实问题

这不是“PowerShell 自身能不能执行”的问题，而是：

> **当 RTK 已经决定重写某个 shell 风格命令时，Windows 上是否还有一个不依赖 Unix 二进制的实现可执行。**

---

## 二、根因分析：RTK 的命令架构

### 2.1 三种实现模式

RTK 子命令目前大致分三类：

| 模式 | 说明 | 示例命令 | Windows 兼容性 |
|------|------|---------|---------------|
| **A: Shell 转发** | 调用外部二进制 + 输出过滤 | `ls`、`tree`、`wc`、`grep`、`rg`、`df`、`du` | ❌ 依赖外部工具是否存在 |
| **B: 自有逻辑** | Rust 原生实现，不依赖外部工具 | `read`、`env`、`json`、`find`、`diff` | ✅ 跨平台 |
| **C: Subprocess 包装** | 调用已知跨平台工具 + 输出过滤 | `git`、`cargo`、`npm`、`pip`、`docker`、`kubectl` | ✅ 工具本身有 Windows 版 |

### 2.2 当前最关键的缺口

| RTK 子命令 | 当前状态 | Windows 问题 | 本次目标 |
|-----------|---------|-------------|---------|
| `rtk wc` | 依赖外部 `wc` | 默认无 `wc.exe` | 完全原生 |
| `rtk ls` | 依赖外部 `ls` + 文本解析 | 默认无 `ls.exe` | Windows 原生实现 |
| `rtk tree` | 依赖外部 `tree` 输出，且找不到 `tree` 时直接 bail | Windows 默认无 Unix `tree.exe`，当前属于已坏状态 | Windows 原生实现 |
| `rtk grep` | `rtk grep` 绑定 `Engine::Grep`，`rtk rg` 绑定 `Engine::Rg` | Windows 默认无 `grep.exe` 时，`rtk grep` 不会自动走 `rg.exe` | Windows fallback |
| `rtk ps` | 已有 `ps -> rtk ps` rewrite 规则，但无顶层 `Commands::Ps` handler，实际落入 `Commands::Other` | Windows 下等价于 `powershell -Command ps`，输出不可控且无 RTK 压缩契约 | 新增独立子命令并修复现有 rewrite 回归 |
| `rtk df` | 已有 `df -> rtk df` rewrite 规则，但无顶层 `Commands::Df` handler，实际落入 `Commands::Other` | Windows 默认无 Unix `df`，`df -h` 被 rewrite 后会在 PowerShell 中失败 | 新增独立子命令并修复现有 rewrite 回归 |
| `rtk du` | 已有 `du -> rtk du` rewrite 规则，但无顶层 `Commands::Du` handler，实际落入 `Commands::Other` | Windows 默认无 Unix `du`，`du ...` 被 rewrite 后会在 PowerShell 中失败 | 新增独立子命令并修复现有 rewrite 回归 |

### 2.3 已经原生化、无需纳入本次改造的命令

| RTK 子命令 | 当前实现 | 说明 |
|-----------|---------|------|
| `rtk find` | `ignore::WalkBuilder` | 已 Rust 原生实现 |
| `rtk diff` | `std::fs` + 自定义 diff | 已 Rust 原生实现 |
| `rtk read` | `std::fs::read_to_string` | 已 Rust 原生实现 |

### 2.4 当前 `Commands::Other` 的处理

```rust
// src/main.rs ~2280
Commands::Other(args) => {
    let raw = args.join(" ");
    let (shell, flag) = if cfg!(windows) {
        ("powershell", "-Command")
    } else {
        ("sh", "-c")
    };
    let status = std::process::Command::new(shell)
        .arg(flag)
        .arg(&raw)
        .status()?;
    ...
}
```

这意味着当前 `rtk ps` 在 Windows 上本质等价于：

```text
powershell -Command ps
```

输出是 PowerShell 默认表格格式，RTK 没有压缩，也没有自己的输出契约。

---

## 三、总方案：先原生化 RTK 子命令，再做受限 rewrite

### 3.1 Phase A：Windows 原生化 RTK 子命令（本计划主体）

先完成下列 RTK 子命令在 Windows 上的原生能力：

- `rtk wc`
- `rtk ls`
- `rtk tree`
- `rtk grep`
- `rtk ps`
- `rtk df`
- `rtk du`

这些完成后，Windows 环境中即使没有 Unix `ls` / `wc` / `grep` / `df` / `du`，RTK 也能直接执行并输出紧凑格式。

### 3.2 Phase B：受限 PowerShell rewrite（可选增强）

只有在满足以下条件时，才为 PowerShell 命令增加 rewrite：

1. 对应 RTK 子命令已原生化
2. 参数可以**直接透传**，或可通过**白名单正则**限制到安全形式
3. 不需要通用 PowerShell 参数翻译层

### 3.3 rewrite 安全模型

当前 `rtk rewrite` 的行为是：

- 匹配规则
- 替换命令前缀
- **其余参数原样保留**

因此本计划必须遵守：

> **凡是参数语义不兼容的 PowerShell 命令，一律不加入 rewrite 白名单。**

这意味着：

- `Get-Content file.txt` 仅可在 Phase B 白名单规则中启用
- `Get-Content file.txt -Tail 10` 不在本计划 rewrite 范围内
- `Get-ChildItem -Recurse -Filter` 不在本计划 rewrite 范围内

---

## 四、逐命令改造方案（Phase A）

### 4.1 `rtk wc` — 完全原生化

**当前实现**（`src/cmds/system/wc_cmd.rs`）：

```rust
let mut cmd = resolved_command("wc");
```

**改造方案**：使用 byte-first 设计读取文件 / stdin。

- 底层读取使用 `std::fs::read` 或 stdin `read_to_end`
- `-c` 统计 bytes，不能依赖 UTF-8 文本解析
- `-l` 统计 `b'\n'`
- `-w` 可在 UTF-8 成功时使用 Unicode whitespace；invalid UTF-8 时降级为 ASCII whitespace byte 扫描
- `-m` 统计 UTF-8 chars；invalid UTF-8 时返回错误，退出码 `2`，不使用 lossy 规则
- 任意 flag 组合只要包含 `-m` 且输入 invalid UTF-8，整个命令返回 `2`，不输出部分统计结果
- 二进制 / 非 UTF-8 输入下，`-c` 始终可用且按 bytes 统计；`-m` 明确要求 UTF-8，不追求 GNU `wc` 在不同 locale 下的字符计数边缘行为
- Windows 原生分支必须显式处理 `--version` 和 `--help`；由于 `trailing_var_arg` 会让 `--help` 进入命令参数列表，不能依赖 Clap 自动拦截。若 `-h` 也进入命令参数列表，同样必须返回帮助而不是当作文件路径读取
- 仅替换 `resolved_command("wc")` 之上的执行层；保留现有 `WcMode`、`detect_mode()`、`filter_wc_output()` 及其输出格式契约（如 `30L 96W 978B`、多文件 `Σ ...`）
- 现有 `wc_cmd` 单元测试必须继续作为回归基准；新增 Windows 原生读取测试不得放宽既有紧凑输出格式

#### 支持边界

| 项目 | 计划 |
|------|------|
| 支持输入 | 文件、stdin |
| 支持参数 | `-l` `-w` `-c` `-m` 及组合 |
| 输出目标 | 保持现有 RTK 的紧凑输出契约 |
| 不承诺 | 逐字节复刻所有平台 / locale 下的 `wc` 边缘行为 |
| 必须避免 | 用 `read_to_string` 实现 `-c`，否则二进制文件和 invalid UTF-8 会失败 |
| help/version | `--help` 返回 RTK wc 帮助；`--version` 返回 RTK 版本或明确的 RTK wc 版本说明；`-h` 若未被 Clap 拦截也必须按帮助处理 |

#### fallback 规则

- Windows：始终走原生实现
- Unix：保留现有外部 `wc` 实现，避免扩大本计划影响面

#### 验收标准

- `rtk wc Cargo.toml`
- `type Cargo.toml | rtk wc`
- `rtk wc -l Cargo.toml`
- `rtk wc -w Cargo.toml`
- `rtk wc -c Cargo.toml`
- `rtk wc --help`
- `rtk wc --version`
- invalid UTF-8 输入：`-c` 可统计 bytes；`-w` 不崩溃并按 ASCII whitespace byte 扫描降级；`-m` / `-cm` 返回 `2`

**复杂度**：低

---

### 4.2 `rtk ls` — Windows 原生目录列表

**当前实现**（`src/cmds/system/ls.rs`）：

```rust
let mut cmd = resolved_command("ls");
cmd.env("LC_ALL", "C");
cmd.arg("-la");
```

当前 `compact_ls()` 解析的是 `ls -la` 文本输出，而不是目录项对象。

#### 目标重新定义

Windows 原生 `rtk ls` 的目标是：

> **保留 RTK 的紧凑目录视图契约，而不是逐字节复刻 GNU `ls -la`。**

#### 支持边界

| 项目 | 计划 |
|------|------|
| 支持路径 | `.`、单路径、多路径 |
| 支持参数 | 首批支持 `-a` / 基础目录列表；其余只保留必要子集 |
| 输出目标 | 继续输出 RTK 的紧凑目录结构 |
| 风险点 | owner / group / permissions 缺失；Windows junction / symlink；排序稳定性 |
| 不承诺 | 完整 GNU `ls` 参数和字段等价 |

#### 实现要求

- Windows 分支中直接读取 `std::fs::read_dir`
- 不再把 `DirEntry` 硬转成完整 fake `ls -la` 文本
- 不直接复用 `compact_ls(ls_text)` 本体；它当前耦合 `ls -la` 文本解析。实施时应抽出 `human_size()`、`perms_to_octal()` 等纯 helper，并新增结构化 `LsEntry` / formatter，让 Unix 文本解析结果和 Windows `DirEntry` 都汇入同一格式化入口
- Windows 目录项与 Unix `ls -la` 解析结果必须汇入同一 RTK 紧凑格式契约；只允许数据源分叉，不允许重新发明另一套输出行格式，也不允许通过伪造 `ls -la` 文本绕回解析器
- 保持现有 `compact_ls()` 单元测试锁定的行为：目录尾随 `/`、文件 size 人类可读、空目录 `(empty)`、管道场景行数稳定、`-l` 时八进制权限前缀等

#### fallback 规则

- Windows：走原生实现
- Unix：保留现有 shell 转发逻辑；`LC_ALL=C` 只应保留在外部 `ls` 分支，Windows 原生分支不得设置该环境变量

#### unsupported 参数策略

首版 Windows 原生分支只支持明确列出的参数。遇到未支持参数时必须返回清晰错误，不得静默忽略，也不得试图调用不存在的 `ls.exe`：

| 输入 | Windows 首版行为 | 说明 |
|------|------------------|------|
| `rtk ls` / `rtk ls .` | 支持 | 基础目录列表 |
| `rtk ls -a .` / `rtk ls --all .` | 支持 | 显示隐藏项 |
| `rtk ls <path1> <path2>` | 支持 | 多路径分别输出 |
| `rtk ls -l` / `-lh` | 支持但降级为 RTK 紧凑格式 | 不复刻 GNU long format；`-h` 对紧凑 size 生效 |
| `rtk ls -R` | 暂不支持，提示使用 `rtk tree` | 避免递归语义混入 `ls` |
| `rtk ls --color=auto` | 静默忽略 | 颜色不属于 RTK 紧凑输出契约 |
| 其他未知 flags | 明确 unsupported | 防止静默语义漂移 |

#### 验收标准

- `rtk ls .`
- `rtk ls -a .`
- `rtk ls src tests`
- 含隐藏文件目录
- 含符号链接 / junction 的目录

**复杂度**：高

---

### 4.3 `rtk tree` — Windows 原生目录树

**当前实现**（`src/cmds/system/tree.rs`）：

```rust
let mut cmd = resolved_command("tree");
```

当前 `tree.rs` 会先检查 `tool_exists("tree")`，找不到外部 `tree` 时直接 `bail!`。因此 Windows 无 `tree.exe` 时 `rtk tree` 不是“输出依赖外部工具但还能工作”，而是当前已经硬失败。本节实现必须修复这个已存在回归。

#### 支持边界

| 项目 | 计划 |
|------|------|
| 支持 | 基础树形输出、递归、默认噪声目录过滤 |
| 支持参数 | `-a` / `--all`、用户显式 ignore 优先于默认 `NOISE_DIRS` |
| 风险点 | `NOISE_DIRS` 逻辑迁移、缩进稳定性、路径分隔符、符号链接检测 |
| 不承诺 | 完整复刻系统 `tree` 的全部 flags |

#### 必须保留的行为

- 默认应用 `NOISE_DIRS`
- 用户显式指定 ignore 时，不重复叠加默认 ignore
- `-a` / `--all` 时，允许显示隐藏项，并禁用 RTK 自动注入的默认 `NOISE_DIRS` 过滤（包括 `.git` 这类隐藏 noise 和 `node_modules` 这类非隐藏 noise）；若用户同时显式传入 `-I` / `--ignore`，仍按用户指定的 ignore pattern 过滤

#### ignore pattern 语义

Windows 原生实现必须固定 `-I` / `--ignore=` 的匹配语义：

- pattern 作用于 basename，不作用于完整路径
- `|` 表示多个 basename pattern 的 OR，例如 `node_modules|target`
- `*` / `?` 使用 glob 语义
- 不使用 regex 语义
- 匹配大小写在 Windows 上大小写不敏感，在 Unix 上保持平台默认行为
- 用户显式 ignore 存在时，完全替代默认 `NOISE_DIRS`，不叠加
- `NOISE_DIRS` 中的 `*.egg-info` 必须按 glob basename 语义匹配 `mypackage.egg-info`；该行为应作为兼容测试锁定，不把它描述成外部 `tree -I` 的字面匹配修复

#### 输出格式契约

- 首版固定使用文本树形缩进，目录和文件排序稳定
- 输出必须包含根路径行；子项使用 `├──` / `└──` 连接符和稳定缩进前缀，不随隐藏项、ignore 或错误 warning 改变同级缩进
- 首版不输出外部 `tree` 的 `N directories, M files` 汇总行，避免在 ignore / 权限错误下引入额外计数语义
- 遇到 symlink / junction / reparse point 时不递归进入；显示名称必须稳定，并在测试中覆盖

#### fallback 规则

- Windows：走原生实现
- Unix：保留现有 shell 转发逻辑

#### unsupported 参数策略

| 输入 | Windows 首版行为 | 说明 |
|------|------------------|------|
| `rtk tree` / `rtk tree <path>` | 支持 | 基础树输出 |
| `rtk tree -a` / `--all` | 支持 | 显示隐藏项 |
| `rtk tree -I <pattern>` | 支持 | 用户 ignore 优先于默认 `NOISE_DIRS` |
| `rtk tree --ignore=<pattern>` | 支持 | 与 `-I <pattern>` 等价 |
| `rtk tree -L <n>` | 支持 | 深度限制是常用能力 |
| 其他未知 flags | 明确 unsupported | 不静默忽略 |

#### 验收标准

- `rtk tree .`
- `rtk tree -a .`
- `rtk tree` 默认跳过 `node_modules` / `.git` 等噪声目录
- `rtk tree` 默认跳过 `mypackage.egg-info` 一类匹配 `*.egg-info` 的噪声目录
- 用户显式 ignore 与默认 ignore 不冲突
- Windows 无 `tree.exe` 时 `rtk tree .` 能运行且不触发 `tree command not found` bail

**复杂度**：中

---

### 4.4 `rtk grep` — Windows fallback，而非完整 grep 重写

**当前实现**（`src/cmds/system/search.rs`）：

```rust
let mut cmd = resolved_command(engine.bin());  // engine = grep | rg
```

`main.rs` 当前将 `rtk grep` dispatch 到 `Engine::Grep`，将 `rtk rg` dispatch 到 `Engine::Rg`。`search.rs` 的契约是运行用户实际调用的 engine，不会在 `rtk grep` 缺少 `grep.exe` 时自动替换为 `rg.exe`。因此即使 Windows 上存在 `rg.exe`，也不能证明 `rtk grep` 可用。

#### 目标重新定义

本计划的目标不是“用 Rust 复刻 grep / rg 所有行为”，而是：

> 当 Windows 上缺少 `grep.exe` 时，为 `rtk grep` 提供**基础文本逐行搜索能力**和现有紧凑输出的最小闭环；`rtk rg` 继续使用真实 `rg.exe`，不纳入本次原生替代范围。

#### 支持边界

| 类别 | 计划 |
|------|------|
| 基础 pattern + path | 原生支持 |
| 多文件搜索 | 原生支持 |
| 多 pattern | 支持 `-e foo -e bar path`；多个 bare pattern 不支持，返回明确 unsupported |
| stdin | 无 path、无递归 flag、且 stdin 非 TTY 时逐行读取 stdin；显式 path 存在时不混读 stdin；递归模式优先于 stdin 判定 |
| 递归 | `-r` / `-R` / `--recursive` 在 Rust fallback 中必须显式实现或返回 unsupported，不能被当作普通 shape flag 透传到缺失引擎；若实现递归，`grep -r <pattern>` 无显式 path 时默认搜索当前工作目录 `.`，不读取 stdin |
| dialect flags | `-E` 在 Rust fallback 中视为 no-op 并记录测试，因 Rust regex 已接近扩展正则；`-P` 返回 unsupported exit `2`，不伪装 PCRE 支持 |
| shape flags（`-c` `-l` `-L` `-o` `-Z` `--files` `--type-list`） | 外部 `grep` 存在时 passthrough；缺失时返回 unsupported exit `2` |
| literal flag | `-F` 首版不实现；外部 `grep` 存在时 passthrough，缺失时返回 unsupported exit `2` |
| `rg` | 不在本次原生化范围 |

#### pattern 方言

Rust fallback 使用 Rust `regex` crate 语义，不承诺 GNU grep BRE/ERE/PCRE 完全兼容：

- `-e <pattern>` 支持多个 Rust regex pattern
- `-E`：Rust fallback 中接受但不改变方言，作为兼容 no-op
- `-P`：Rust fallback 中明确 unsupported，退出码 `2`
- 不支持 grep BRE 专属转义语义
- 不支持 PCRE 扩展
- 正则编译失败返回 exit `2`
- literal 搜索不在首版范围内；用户传 `-F` 且外部 `grep` 缺失时返回 unsupported exit `2`

#### fallback / unsupported 规则

- Windows 中 `rtk grep` 找不到 `grep.exe`：启用 Rust fallback
- Windows 中 `rtk rg` 继续调用 `rg.exe`，不因本计划改为 Rust fallback
- 外部 `grep` 存在时，`rtk grep` 的 shape flags 继续走现有 passthrough
- 外部 `grep` 不存在时，`rtk grep` 的 shape flags 不能继续 passthrough 到缺失引擎；必须返回明确 unsupported 错误，除非该 flag 已被 Rust fallback 显式实现
- 外部 `grep` 不存在且用户传 `-F` 时，返回明确 unsupported 错误和 exit `2`
- `--help` / `--version` 在外部 `grep` 缺失时也必须返回 RTK 自己的说明或明确 unsupported，不能因 `resolved_command("grep")` 失败而产生模糊错误
- `--help` 最低内容：说明这是 `rtk grep` 的 Windows Rust fallback、列出支持参数、说明 Rust regex 方言、列出与 GNU grep 不兼容处，并提示需要完整 grep 时使用 `rtk proxy grep ...`
- `--version` 最低内容：返回 RTK 版本和 fallback 名称；不得尝试调用缺失的 `grep.exe`

#### 退出码契约

Rust fallback 必须保留 grep 风格退出码，否则会改变脚本 / agent 的控制流语义：

| 场景 | 退出码 | 说明 |
|------|--------|------|
| 至少一个匹配 | `0` | 输出匹配结果 |
| 无匹配 | `1` | 不输出匹配结果，不能当作执行错误 |
| 文件不存在 / 读取失败 | `2` | 打印错误到 stderr |
| 正则语法错误 | `2` | 打印错误到 stderr |
| 不支持的 shape flag 且外部 `grep` 存在 | passthrough | 由底层 engine 决定退出码 |
| 不支持的 shape flag 且外部 `grep` 缺失 | `2` | 打印明确 unsupported 错误 |

#### 验收标准

- `rtk grep "fn main" src/main.rs`
- `rtk grep "TODO" src tests`
- stdin 搜索场景
- 多 pattern 搜索场景
- `-E` no-op、`-P` unsupported exit `2`
- `-F` 在外部 `grep` 缺失时 unsupported exit `2`
- `-r` / `-R` / `--recursive` 的支持或 unsupported 行为被测试锁定
- 若实现递归：`rtk grep -r "TODO"` 无显式 path 时搜索当前目录，不读取 stdin
- `rtk grep --help` / `rtk grep --version` 在无 `grep.exe` 时仍返回清晰输出
- shape flags 保持 passthrough / 不误处理
- 匹配 / 无匹配 / 文件不存在 / 正则错误的退出码符合上表

**复杂度**：中高

---

### 4.5 `rtk ps` — 新增独立原生子命令

**当前实现**：无独立子命令，实为 `Commands::Other -> powershell -Command ps`。

注意：`rules.rs` 中已经存在 `ps -> rtk ps` rewrite 规则，但顶层 `Commands::Ps` handler 缺失。当前这是一个已存在回归：hook rewrite 后不会进入可控 RTK handler，而会落入 `Commands::Other`。

#### 技术路线

Windows 首版固定使用 `sysinfo`：

- 原因：在 Windows 上可直接获得稳定进程枚举，且比维护 Win32 细节更简单
- 必须在 `Cargo.toml` 新增 `sysinfo` 依赖，并记录版本选择
- 必须记录并验证所选 `sysinfo` 版本在当前 Windows 支持范围内可用；若 API 在目标 Rust / Windows 版本上不可用，应先降级版本或单独评估替代方案
- 实施时记录 release binary 体积和冷启动时间变化，但**不**在本计划实施阶段切换到 Windows API
- 若后续确认 `sysinfo` 不满足性能目标，应单独立项评估 Windows API（`CreateToolhelp32Snapshot` 等）替代

#### 输出契约（首版最小集）

| 字段 | 首版要求 |
|------|---------|
| PID | 必须 |
| 进程名 | 必须 |
| 内存 | 不显示 |
| CPU | 不显示 |
| 线程数 | 非必须 |

首版 Windows 输出固定为两列文本表：

- 第一行为 header：`PID NAME`
- 后续每行一条进程记录，PID 在前、进程名在后，使用 ASCII 空格分隔
- 按 PID 升序排序，避免同一次测试中输出顺序随 `sysinfo` 枚举顺序漂移
- 进程名不额外 quoting；若包含空格，保持原名并由测试锁定该场景
- 该两列格式必须由测试锁定，避免未来改成逗号、制表符或 PowerShell 风格表格导致脚本解析漂移

#### 支持边界

| 项目 | 计划 |
|------|------|
| 支持 | 基础进程列表 |
| 不承诺 | 完整复刻 `Get-Process` / Unix `ps` 所有列和所有参数 |
| 风险点 | 权限不足进程、输出稳定性、排序规则 |

#### 参数处理策略

新增 `Commands::Ps` 后必须避免破坏现有 `ps -> rtk ps` rewrite。Windows 与 Unix/macOS 的策略分开定义：

| 输入形式 | 首版行为 | 说明 |
|----------|----------|------|
| `rtk ps`（Windows） | 原生执行 | 输出最小字段集：PID + 进程名 |
| `rtk ps --help` / `rtk ps -h`（Windows） | 手动打印 `Ps` help 并返回 `0` | 不依赖 Clap 对 `trailing_var_arg` 的 help 处理细节 |
| `rtk ps aux` / `rtk ps -ef`（Windows） | 暂不支持，返回明确错误 | Windows 首版不做 Unix `ps` 参数兼容 |
| `rtk ps <其他参数>`（Windows） | 暂不支持，返回明确错误 | 避免把未知参数误解释为过滤条件 |
| `rtk ps ...`（Unix/macOS） | 透传给外部 `ps` | 保持现有外部命令行为与退出码 |
| `Get-Process -Name foo` rewrite 后 | 不允许 rewrite | 需要 PowerShell 参数翻译层 |

过滤进程名不在首版范围内；未来若要支持，应单独设计 `rtk ps --name <pattern>`，不要复用 PowerShell 或 Unix `ps` 参数语义。

#### 必要代码改动

- 在 `Commands` 枚举中新增 `Ps`
- 在 `main.rs` 中新增 dispatch
- 在 `test_every_subcommand_is_classified` 使用的命令分类中把 `ps` 归为外部工具封装/透传类（现有测试里的 `PASSTHROUGH` 类），不要加入 `RTK_META_COMMANDS`；该分类只是测试里的“包装真实工具”标签，不得被解释为运行时允许 parse 失败后 fallback
- 在 `rules.rs` 中保留 `ps -> rtk ps`；`Get-Process` 只在 Phase B 白名单阶段扩展

#### fallback 规则

- Windows：始终走独立原生 `rtk ps`
- Unix/macOS：由 `Commands::Ps` 通过 `Command::new("ps").args(args).status()` 透传到外部 `ps`，并通过 `exit_code_from_status` 保留退出码
- 不允许因 Clap 解析失败掉回 `Commands::Other`

#### 跨平台策略

`ps -> rtk ps` rewrite 规则是全局规则，不是 Windows-only。新增 `Commands::Ps` 后的固定策略如下：

| 平台 | 行为 |
|------|------|
| Windows | 使用 `sysinfo` 原生输出最小字段集 |
| Unix/macOS | 保持现有外部 `ps` 行为，由 `Commands::Ps` 使用 `Command::new("ps").args(args).status()` 透传参数并保留退出码 |
| 任意平台 parse 失败 | 不得 fallback 到 `Commands::Other` |

也就是说：本计划只在 Windows 上改变 `ps` 的实现方式，不改变 Unix/macOS 上 `ps` 的用户体验。Windows 下 `ps aux` / `ps -ef` 等复杂 Unix 形式仍明确 unsupported，并在错误消息中提示使用 `rtk proxy ps aux` 获取原生命令输出。

#### Clap 参数结构

为避免 Clap 在未知参数上直接输出生硬 parse error，`Ps` 子命令首版应显式捕获剩余参数并自行判断：

```rust
#[command(disable_help_flag = true)]
Ps {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}
```

处理规则：

- Windows：`args.is_empty()` 执行原生列表；如果 `args` 中包含 `--help` / `-h`，手动打印 `Ps` help 并返回 `0`；其他参数进入 unsupported 分支并给出 `rtk proxy ps ...` 提示。
- Unix/macOS：`args` 原样透传给外部 `ps`，保留退出码；`args.is_empty()` 即执行外部 `ps`，不补默认参数。
- `Ps` 也使用 `disable_help_flag = true`，避免 `-h` 被 Clap 默认帮助占用；首版不支持 Unix `ps -h` 语义，但必须给出清晰 unsupported / proxy 提示。
- 手写 help 最低内容：命令用途、Windows 首版支持的无参形式、不支持 `ps aux` / `ps -ef` 的说明、`--help` 本身、以及 `rtk proxy ps ...` fallback 提示。

#### 验收标准

- `rtk ps`
- 输出字段稳定
- 低权限环境不崩溃
- Windows 上 `rtk ps --bad` 明确 unsupported，不 fallback 到外部 `ps`
- Unix/macOS 上 `rtk ps aux` 保持现有外部 `ps` 行为

**复杂度**：中

---

### 4.6 `rtk df` — 新增独立原生子命令

**当前实现**：`rules.rs` 中存在 `df -> rtk df` rewrite，但 `main.rs` 无 `Commands::Df`，没有真实 `df` 子命令。

这不是纯新增能力：`df -> rtk df` rewrite 已经存在，当前 Windows 上 `df -h` 被 rewrite 后会落入 `Commands::Other -> powershell -Command "df -h"` 并失败。本节实现必须同时修复该回归。

#### 目标重新定义

`rtk df` 的目标是提供 Windows/跨平台磁盘空间摘要，而不是完整复刻 Unix `df` 全部文件系统列。

#### 技术路线

- Windows 首版固定使用 `sysinfo` 的 disks / mounted volumes 能力
- 输出最小字段：卷/挂载点、总容量、已用、可用、使用率
- Windows 上显示盘符或卷挂载点
- Unix/macOS 不改实现，保持现有外部 `df` 行为
- 必须记录并验证所选 `sysinfo` 版本在当前 Windows 支持范围内可用
- 若后续确认 `sysinfo` 对启动时间/二进制体积影响不可接受，应单独立项评估 Windows API 替代，不在本计划实施阶段切换路线

#### 输出契约

- 按盘符 / 挂载点名称字典序排序
- `used = total - available`
- `use% = floor(used * 100 / total)`
- `-h` / `--human-readable` 使用 RTK 现有 compact size 风格：`978B`、`1.2K`、`345M`、`1.2G`，采用 GNU 风格单字母量级后缀，不输出 `KB` / `MB` / `GB` 后缀
- `total == 0` 但卷信息可枚举时保留该行并显示 `use% = ?`，同时汇总 warning
- 不可访问或完全缺失容量信息的卷跳过，并汇总 warning 数

#### 支持边界

| 项目 | 计划 |
|------|------|
| 支持 | `rtk df`、`rtk df -h` |
| 不支持 | `-i` inode 统计、`-T` 文件系统类型、复杂过滤参数 |
| unknown flags | 明确 unsupported，提示使用 `rtk proxy df ...` |

#### 必要代码改动

- 在 `Commands` 枚举中新增 `Df`
- 在 `main.rs` 中新增 dispatch
- 在 `test_every_subcommand_is_classified` 使用的命令分类中把 `df` 归为外部工具封装/透传类（现有测试里的 `PASSTHROUGH` 类），不要加入 `RTK_META_COMMANDS`；该分类不得改变运行时“不 fallback 到 `Commands::Other`”的要求
- 复用 `sysinfo` 依赖；若 `ps` 已加入 `sysinfo`，不新增额外依赖

#### Clap 参数结构

`df` 首版需要把 `-h` 留给 human-readable，因此不能依赖 Clap 默认短帮助：

```rust
#[command(disable_help_flag = true)]
Df {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}
```

处理规则：

- Windows：`args.is_empty()` → 执行基础列表；`args == ["-h"]` 或 `args == ["--human-readable"]` → human-readable 输出；`args == ["--help"]` → 手动打印 `Df` help 并返回 `0`；其他参数 → 明确 unsupported，并提示 `rtk proxy df ...`
- Unix/macOS：`args` 原样透传给外部 `df`，保留退出码
- 手写 help 最低内容：命令用途、支持的 `-h` / `--human-readable`、`--help` 本身、不支持参数列表、以及 `rtk proxy df ...` fallback 提示

#### 验收标准

- `rtk df`
- `rtk df -h`
- `rtk df --bad` 明确 unsupported，不 fallback 到外部 `df`
- Windows 上无 `df.exe` 也能运行

#### 跨平台策略

`df -> rtk df` rewrite 规则是全局规则，不是 Windows-only。新增 `Commands::Df` 后的固定策略如下：

| 平台 | 行为 |
|------|------|
| Windows | 使用 `sysinfo` 读取磁盘 / 卷容量 |
| Unix/macOS | 保持现有外部 `df` 行为，由 `Commands::Df` 使用 `Command::new("df").args(args).status()` 透传参数并保留退出码 |
| 任意平台 parse 失败 | 不得 fallback 到 `Commands::Other` |

**复杂度**：中

---

### 4.7 `rtk du` — 新增独立原生子命令

**当前实现**：`rules.rs` 中存在 `du -> rtk du` rewrite，但 `main.rs` 无 `Commands::Du`，没有真实 `du` 子命令。

这不是纯新增能力：`du -> rtk du` rewrite 已经存在，当前 Windows 上 `du ...` 被 rewrite 后会落入 `Commands::Other -> powershell -Command "du ..."` 并失败。本节实现必须同时修复该回归。

#### 目标重新定义

`rtk du` 的目标是提供目录/文件占用空间摘要，优先服务 agent 判断“大目录在哪里”，不是完整复刻 GNU `du` 所有磁盘块语义。

#### 技术路线

- Windows 首版固定使用 `walkdir` 递归遍历
- 首版统计 logical file size（metadata length），不承诺磁盘实际 allocation size
- 默认跳过子路径权限错误并汇总 warning 数；不能因单个文件不可读导致整个命令失败
- Unix/macOS 不改实现，保持现有外部 `du` 行为

#### 支持边界

| 输入 | 首版行为 | 说明 |
|------|----------|------|
| `rtk du <path>` | 支持 | 输出路径总大小 |
| `rtk du -s <path>` / `--summarize` | 支持 | 只输出总计 |
| `rtk du -h <path>` | 支持 | human-readable size |
| `rtk du -d <n> <path>` / `-d<n>` / `--max-depth <n>` / `--max-depth=<n>` | 支持 | 控制输出深度 |
| `rtk du -sh <path>` / `-hs <path>` | 支持 | 组合短 flags |
| 多路径 | 支持 | 分别输出 |
| unknown flags | 明确 unsupported | 提示使用 `rtk proxy du ...` |

#### 过滤与安全策略

- 不默认跳过 `NOISE_DIRS`，因为 `du` 的核心用途是看真实空间占用
- 遇到 symlink / junction / reparse point：默认不跟随，避免循环和跨卷遍历
- 使用 `WalkDir::follow_links(false)`，并在 Windows 测试中覆盖 junction / symlink / reparse point 不递归进入
- 对 symlink / junction / reparse point 使用 `symlink_metadata`，不递归进入，且不把目标内容大小计入总量
- 根路径不可访问：返回 exit `2`
- 子路径权限错误：记录 warning，继续统计可访问部分，最终返回 `0`

#### 必要代码改动

- 在 `Commands` 枚举中新增 `Du`
- 在 `main.rs` 中新增 dispatch
- 在 `test_every_subcommand_is_classified` 使用的命令分类中把 `du` 归为外部工具封装/透传类（现有测试里的 `PASSTHROUGH` 类），不要加入 `RTK_META_COMMANDS`；该分类不得改变运行时“不 fallback 到 `Commands::Other`”的要求
- 复用现有 `walkdir` 依赖或 `std::fs` 递归；首版优先 `walkdir`

#### Clap 参数结构

`du` 首版同样需要把 `-h` 留给 human-readable：

```rust
#[command(disable_help_flag = true)]
Du {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}
```

处理规则：

- Windows：支持 `-s`、`-h`、`-sh`、`-hs`、`-d 1`、`-d1`、`--max-depth 1`、`--max-depth=1`；`args == ["--help"]` → 手动打印 `Du` help 并返回 `0`；其他未知参数 → 明确 unsupported，并提示 `rtk proxy du ...`
- Unix/macOS：`args` 原样透传给外部 `du`，保留退出码
- `du` 参数解析应先做一轮显式 token scan：拆分短 flag cluster，识别 `-d<n>` 和 `-d <n>`，识别 `--max-depth=<n>` 和 `--max-depth <n>`；缺少深度值、非数字、负数、重复深度值均返回 unsupported exit `2`
- `-h` / human-readable 输出使用 RTK 现有 compact size 风格：`978B`、`1.2K`、`345M`、`1.2G`，采用 GNU 风格单字母量级后缀，不输出 `KB` / `MB` / `GB` 后缀
- 手写 help 最低内容：命令用途、支持的 `-s` / `-h` / `-d` / `--max-depth`、`--help` 本身、不跟随 symlink/junction 的说明、以及 `rtk proxy du ...` fallback 提示

#### 验收标准

- `rtk du .`
- `rtk du -sh target`
- `rtk du -d 1 .`
- `rtk du -d1 .`
- 多路径统计
- symlink / junction 不跟随
- 权限错误不中断整体统计
- 根路径不可访问返回 `2`
- Windows 上无 `du.exe` 也能运行

#### 跨平台策略

`du -> rtk du` rewrite 规则是全局规则，不是 Windows-only。新增 `Commands::Du` 后的固定策略如下：

| 平台 | 行为 |
|------|------|
| Windows | 使用 `walkdir` + 原生 metadata 遍历 |
| Unix/macOS | 保持现有外部 `du` 行为，由 `Commands::Du` 使用 `Command::new("du").args(args).status()` 透传参数并保留退出码 |
| 任意平台 parse 失败 | 不得 fallback 到 `Commands::Other` |

**复杂度**：中

---

### 4.8 `rtk find` — 已原生（无需改造）

`find_cmd.rs` 已使用 `ignore::WalkBuilder`，支持 `.gitignore`、最大深度、glob 匹配。**不纳入本次改造。**

### 4.9 `rtk diff` — 已原生（无需改造）

`diff_cmd.rs` 已通过纯 Rust 自定义实现文件比较。**不纳入本次改造。**

---

## 五、Phase B：受限 PowerShell rewrite 设计

### 5.1 总原则

本计划**不新增通用 PowerShell 参数翻译层**。

因此 rewrite 仅允许两类形式：

1. **前缀安全型**：参数可直接透传
2. **白名单正则型**：只匹配极少数安全形式

凡是需要将 PowerShell 参数翻译成 RTK 参数的形式，全部延后。

### 5.2 可考虑纳入白名单的形式（后续阶段）

| PowerShell 命令 | 条件 | 目标 |
|----------------|------|------|
| `dir` | 基础调用 / 单路径，无 PowerShell 专有参数 | `rtk ls` |
| `Get-Process` | 基础调用 | `rtk ps` |
| `Get-Content <file>` | 仅基础文件读取，无 `-Tail` / `-TotalCount` | `rtk read <file>` |

#### 白名单 pattern 契约

Phase B 若实施 rewrite，必须先用测试锁定下列白名单。未列入的形式默认 passthrough：

| 输入 | 是否 rewrite | 目标输出 | 备注 |
|------|--------------|----------|------|
| `dir` | 是 | `rtk ls` | 仅无参基础形式 |
| `dir <path>` | 是 | `rtk ls <path>` | `<path>` 不得以 `-` 开头 |
| `dir -Force` | 否 | passthrough | PowerShell 参数需翻译，不直接透传 |
| `dir -Recurse` | 否 | passthrough | 需要映射到 tree/find 语义，延后 |
| `Get-Process` | 是 | `rtk ps` | 仅无参基础形式 |
| `Get-Process <anything>` | 否 | passthrough | 例如 `-Name` / `-Id` 需要参数翻译 |
| `Get-Content <file>` | 是 | `rtk read <file>` | 仅单文件、无额外参数 |
| `Get-Content <file> -Tail 10` | 否 | passthrough | 需要翻译到 `--tail-lines`，延后 |
| `Get-Content <file> -Encoding utf8` | 否 | passthrough | 编码语义不直接等价 |

实现时不要只把这些字符串加入 `rewrite_prefixes`；必须让 `pattern` 限制输入形态，避免参数被原样透传到不兼容的 RTK 子命令。

##### 可测试 pattern 约束

Phase B 首版白名单固定只允许这些形态，并必须配套单元测试。下面是语义约束，不是可直接复制进代码的最终正则；实施时应先写 rewrite 单元测试，再固化具体 matcher / regex。

| 规则 | 允许形态 | 拒绝形态 |
|------|----------|----------|
| `dir` | `dir`；或 `dir <single-path>`，命令名大小写不敏感 | 任意 switch、多参数、管道、命令连接符、PowerShell 表达式 |
| `Get-Process` | 仅无参 `Get-Process`，命令名大小写不敏感 | 任何带参数形式 |
| `Get-Content` | `Get-Content <single-path>`，命令名大小写不敏感 | 多文件、任何 switch、管道、script block、命令连接符、PowerShell 表达式 |

`<single-path>` 首版固定限制为：

- 无换行
- PowerShell 命令名前缀大小写不敏感，使用 `(?i)` 或实现前统一 lowercase
- 不包含 PowerShell 管道符 `|`
- 不包含 `;`、`&&`、`||`
- 不包含 `$`、反引号、`(`、`)`、`{`、`}`
- 不允许 `$env:TEMP`、`$(...)`、反引号转义、script block 等 PowerShell 表达式
- quoted 与 unquoted path 都不得以 `-` 开头
- 允许普通相对路径、绝对路径、带引号路径
- 带空格路径必须保持原引号，不做拆分或重新 quoting

quoted path 与 unquoted path 使用同一安全字符集；引号只允许路径中包含空格，不允许引号内出现 `$`、反引号、括号或 PowerShell 表达式。

带引号路径必须有 rewrite 单元测试覆盖；如果测试失败，Phase B 不得启用该 rewrite 规则。

### 5.3 明确不纳入本计划 rewrite 白名单的命令

| 命令 | 原因 |
|------|------|
| `Measure-Object` | 对象聚合语义，不等价于 `wc` |
| `Compare-Object` | 对象比较语义，不等价于 `diff` |
| `where` / `Where-Object` | 管道过滤语义，不等价于 `find` |
| `type` | 当前受 `IGNORED_PREFIXES` 保护，且语义存在歧义 |
| `Get-ChildItem -Recurse/-Filter` | 需要参数翻译层，不可直接透传 |
| `Get-Content -Tail/-TotalCount` | 需要映射到 `--tail-lines` / `--max-lines` |
| `Select-String` 复杂参数 | PowerShell 参数语义不等价于 `grep` |

### 5.4 与当前 rewrite 引擎的契合方式

由于 `rewrite_segment()` 的本质是“替换前缀 + 保留 rest”，因此：

- 规则必须通过 `pattern` **限制可接受输入形态**
- 不能仅靠 `rewrite_prefixes` 就假定 PowerShell 参数可直接复用

### 5.5 冲突与保护规则

- `type` 当前在 `IGNORED_PREFIXES` 中，默认继续 passthrough
- `ps` 现有规则可扩展到 `Get-Process`，但前提是 `rtk ps` 已成为真正子命令
- 不新增 `where -> find` 映射
- 不新增 `Measure-Object -> wc` 或 `Compare-Object -> diff` 映射

---

## 六、实现策略

### 6.1 Windows 原生实现模式

对每个 shell 转发命令，使用同文件内 `#[cfg]` 分支：

```rust
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(not(target_os = "windows"))]
    {
        // Unix: 保持现有 shell 转发逻辑
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: Rust 原生实现
    }
}
```

对 `ps` / `df` / `du`，不是简单沿用既有 shell 转发文件，而是新增正式 `Commands` 变体：Windows 分支原生实现，Unix/macOS 分支直接 `Command::new("<tool>").args(args).status()` 透传并保留退出码。不要把它们加入 `RTK_META_COMMANDS`；它们属于外部工具封装/透传类命令。

`test_every_subcommand_is_classified` 中的 `PASSTHROUGH` 只是测试分类名，用来表示“该 RTK 子命令包装一个真实外部工具或工具语义”。它不应被用于运行时决策，也不意味着 `ps` / `df` / `du` 在 Clap parse 失败时可以落回 `Commands::Other`。

### 6.2 优先级原则

1. 先保证 Windows 可运行
2. 再保证输出契约稳定
3. 最后才考虑高兼容参数覆盖率

### 6.3 依赖策略

| 依赖 | 用途 | 策略 |
|------|------|------|
| `regex` | grep fallback | 已有，可直接用 |
| `sysinfo` | 进程枚举、磁盘/卷容量查询 | `ps` / `df` 首选方案；需新增到 `Cargo.toml` 并验证 binary size / startup 影响 |
| `walkdir` | 递归目录遍历 | 已有；用于 `du`，`find` 已原生无需改造 |
| `ignore` | gitignore-aware 遍历 / 可选目录过滤 | 已有；`find` 已使用，若 `du` 后续需要用户显式 ignore 语义，优先评估复用 |

原则：优先复用现有依赖，新增依赖只服务明确收敛后的输出契约。

---

## 七、实施顺序

### Phase A：Windows 原生化

| 阶段 | 命令 | 目标 | 原因 |
|------|------|------|------|
| **P0** | `rtk wc` | 完全原生 | 范围最小，最快建立成功样板 |
| **P0** | `rtk ps` | 新增独立子命令 | 当前缺口最大，且 rewrite 依赖它 |
| **P0** | `rtk df` | 新增独立子命令 | 与 `ps` 共享 `sysinfo` 依赖，系统查询型命令 |
| **P1** | `rtk tree` | Windows 原生目录树 | 行为边界清晰，先于 `ls` 实现结构化目录能力 |
| **P1** | `rtk ls` | Windows 原生目录列表 | 复杂度高，需在 `tree` / `wc` 样板之后做 |
| **P1** | `rtk du` | 新增独立子命令 | 与目录遍历相关，可复用 `walkdir`/tree 经验 |
| **P2** | `rtk grep` | Windows fallback | flag 表面最大，单独收尾 |
| **P3** | 文档与测试补完 | 固化支持边界 | 防止后续语义漂移 |

### Phase B：受限 rewrite

| 阶段 | 内容 | 前置条件 |
|------|------|---------|
| **R0** | `dir -> rtk ls` 基础形式 | `rtk ls` 已原生化 |
| **R0** | `Get-Process -> rtk ps` 基础形式 | `rtk ps` 已原生化 |
| **R1** | `Get-Content <file> -> rtk read <file>` 基础形式 | 仅允许无需参数翻译的形式 |
| **R2** | 其他 PowerShell 映射 | 需单独立项参数翻译层 |

---

## 八、验收标准与测试矩阵

### 8.1 命令实现测试

| 命令 | 必测场景 |
|------|---------|
| `rtk wc` | Windows 无 `wc.exe`：单文件 / 多文件 / stdin / `-l -w -c -m` / `--help` / `--version` / 二进制文件 `-c` / invalid UTF-8 `-w` 不崩溃且按 ASCII whitespace 降级 / invalid UTF-8 `-m` 返回 `2` / `-cm` 遇 invalid UTF-8 整体返回 `2` |
| `rtk ls` | Windows 无 `ls.exe`：空目录 / 普通目录 / 隐藏文件 / 多路径 / symlink or junction / 结构化 formatter 输出与既有 `compact_ls` 契约一致 / `-R` unsupported / unknown flag unsupported |
| `rtk tree` | Windows 无 `tree.exe`：默认 `NOISE_DIRS` / `*.egg-info` glob basename 过滤 / `-a` 禁用默认 `NOISE_DIRS` 注入 / 显式 `-I <pattern>` 在 `-a` 下仍生效 / `--ignore=<pattern>` / `node_modules|target` OR 语义 / `-L <n>` / `├──` `└──` 输出缩进稳定 / unknown flag unsupported / 不触发 `tree command not found` bail |
| `rtk grep` | Windows 无 `grep.exe` 但可有 `rg.exe`：单文件 / 多文件 / `-e foo -e bar path` / Rust regex 方言 / `-E` no-op / `-P` unsupported exit `2` / `-F` unsupported exit `2` / `-r` `-R` `--recursive` 支持或明确 unsupported / 若支持递归则无 path 搜索 CWD / regex 编译错误 exit `2` / 多 bare pattern unsupported / stdin / `--help` / `--version` / shape flags 在 `grep.exe` 存在时 passthrough / `grep.exe` 缺失时 unsupported exit `2` |
| `rtk ps` | Windows 无外部 Unix `ps`：基础列表 / `PID NAME` header / PID 升序 / 两列空格分隔输出稳定 / `-h` 和 `--help` 手动返回 `0` / 低权限环境 / `ps aux` unsupported / `ps -ef` unsupported / `--bad` 不 fallback；Unix/macOS：空 args 执行外部 `ps`，`ps aux` 保持现有外部行为与退出码 |
| `rtk df` | Windows 无 `df.exe`：基础列表 / `-h` compact size 格式（如 `1.2G`） / `--help` 手动返回 `0` / `--bad` unsupported / `total == 0` 显示 `use% = ?` / 排序与使用率计算符合契约 / rewrite 后不落入 `Commands::Other`；Unix/macOS：`df -h` 保持现有外部行为与退出码 |
| `rtk du` | Windows 无 `du.exe`：单路径 / 多路径 / `-s` / `-h` compact size 格式（如 `1.2G`） / `-sh` / `-d 1` / `-d1` / `--max-depth 1` / `--max-depth=1` / 非法深度值 exit `2` / `--help` 手动返回 `0` / symlink/junction/reparse point 不跟随 / `follow_links(false)` / 子路径权限错误不中断 / 根路径不可访问返回 `2` / rewrite 后不落入 `Commands::Other`；Unix/macOS：`du -sh` 保持现有外部行为与退出码 |

### 8.2 rewrite 测试

| 输入 | 预期 |
|------|------|
| `dir` | 仅在纳入白名单后按规则 rewrite |
| `Get-Process` | 仅在 `rtk ps` 原生化后按白名单 rewrite |
| `Get-Content foo.txt` | 仅基础形式允许 rewrite |
| `type foo.txt` | 保持 passthrough |
| `Measure-Object` | 不 rewrite |
| `Compare-Object` | 不 rewrite |
| `where` / `Where-Object` | 不 rewrite |
| `dir -Force` | 不 rewrite |
| `dir foo -Recurse` | 不 rewrite |
| `Get-Process -Name foo` | 不 rewrite |
| `Get-Content foo.txt -Tail 10` | 不 rewrite |
| `Get-Content "path with spaces.txt"` | 仅在 quoting 测试通过后允许 rewrite |
| `get-process` / `GET-PROCESS` | 大小写不敏感，基础形式允许 rewrite |
| `dir $env:TEMP` | 不 rewrite |
| `dir $(pwd)` | 不 rewrite |
| ``dir `"foo`"`` | 不 rewrite |
| `dir "$env:TEMP"` | 不 rewrite |
| `dir "-foo"` | 不 rewrite |
| `Get-Content "$(pwd)"` | 不 rewrite |
| `Get-Content "-bar.txt"` | 不 rewrite |
| `Get-Content $env:TEMP\a.txt` | 不 rewrite |
| `Get-Content $(Get-Location)` | 不 rewrite |

### 8.3 回归测试要求

- `cargo fmt --all`
- `cargo clippy --all-targets`
- `cargo test --all`
- `cargo check`（可作为开发中快速检查，但不能替代完整门禁）
- 相关单元测试 / 集成测试
- 不破坏 Unix 平台现有行为
- 不破坏当前 `find` / `diff` / `read` 的原生能力
- `Cargo.toml` 新增 `sysinfo` 后，记录依赖版本、release binary size 变化、启动时间是否仍满足项目目标
- `ps` / `df` / `du` 作为正式 `Commands` 变体进入 dispatch，并在子命令分类测试中归为外部工具封装/透传类；验证 Windows 上 `rtk ps --bad`、`rtk df --bad`、`rtk du --bad` 不 fallback 到 `Commands::Other` 或外部命令
- 增加 Windows 无外部 Unix 工具矩阵：至少覆盖无 `ls.exe` / `wc.exe` / `tree.exe` / `grep.exe` / `df.exe` / `du.exe` 的环境；`rtk grep` 还需覆盖“有 `rg.exe` 但无 `grep.exe`”仍走 `rtk grep` fallback 的场景
- Windows 上 `rtk ps --help` / `rtk ps -h` / `rtk df --help` / `rtk du --help` 返回 `0` 且不依赖 Clap 默认 `-h`
- Unix/macOS 上 `rtk ps aux`、`rtk df -h`、`rtk du -sh` 继续保持现有外部命令行为与退出码

### 8.3.1 每阶段质量门禁

每个 Phase A 命令完成后至少运行：

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

若完整门禁因已知历史失败无法通过，必须记录：

- 失败测试名称
- 是否与本次变更相关
- 本次命令相关测试是否全部通过
- 后续修复归属

### 8.4 完成定义（Definition of Done）

某个命令视为完成，必须同时满足：

1. Windows 上无 Unix 二进制依赖也能运行
2. 输出满足本计划定义的 RTK 紧凑契约
3. 已写明支持边界与非支持边界
4. 已有正向测试 + 至少一个负向 / 回归测试

---

## 九、风险评估

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| 计划继续扩张成“完整 PowerShell 兼容” | 高 | 高 | 明确分 Phase A / B，拒绝通用参数翻译 |
| `ls` 目标滑向复刻 GNU `ls` | 中 | 高 | 以 RTK 输出契约为目标，而非逐字段兼容 |
| `grep` flag 表面失控 | 高 | 高 | 先定义支持子集，保留 shape flags passthrough |
| `ps` 输出字段争议过大 | 中 | 中 | 首版只收敛到 PID + 名称最小集 |
| `du` 扫描超大目录或权限复杂目录导致耗时/告警过多 | 中 | 中 | 默认摘要优先、支持深度限制、warning 汇总、根路径错误与子路径错误分级处理 |
| rewrite 误改写 PowerShell 命令 | 中 | 高 | 只允许白名单正则，不做模糊 alias 覆盖 |

---

## 十、不在本次范围内

- `rtk rg` 的完全原生替代（Windows 上通常有 `rg.exe`）
- `rtk sort` 原生化
- `type` alias 的自动接管
- `Measure-Object` 的等价替代
- `Compare-Object` 的等价替代
- `where` / `Where-Object` 的等价替代
- PowerShell 参数翻译层
- PowerShell 对象管道兼容

不在范围内并不代表不能做，而是：

> **这些项需要单独立项，不能挤进当前“Windows 原生 shell 子命令补齐”计划里。**
