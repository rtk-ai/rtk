---
title: RTK pytest 结果真实性修复合并过程
status: source_ready
owner: Camus
template_id: report.merge_process.v1
template_source: team-ai-pack/templates/merge-process-template.md
related_prd: not_applicable
related_technical_plan: not_applicable
related_test_plan: not_applicable
related_test_set: not_applicable
last_updated: 2026-08-10
---

# 合并过程：RTK pytest 结果真实性修复

> 本改动只修复 pytest 压缩输出的结果判定，并复用捕获层的截断事实；不增加通用超时、不恢复 Safe Runner child deadline、不触碰业务 runtime。

## 产品汇报（与 Multica 正文对齐）

### 需求对象与用户价值

- 问题：pytest 已经收集或执行测试，但摘要未被识别时，RTK 会把“无法判断”误报成“没有测试”，让用户误以为测试没有运行。
- 用户价值：用户继续得到真实退出码、可识别的通过/失败/跳过/收集数量、子测试数量和失败位置；摘要不完整时会明确看到“无法解析”，不会得到编造结论。
- 如果不改：长时间运行、静默输出、摘要格式变化或捕获截断都会继续制造错误 no-tests 结论，掩盖真实失败。

### 目标边界

- In scope：pytest 单一解析主路、直接 pytest 执行的退出码与 stdout 截断事实传入、摘要识别、计数、失败位置和原始日志路径 fixtures。
- Out of scope：Safe Runner、通用超时、10 MiB 捕获上限数值、其他 test/lint wrapper 的行为、业务 runtime、生产发布。
- 历史相关卡 / 依赖：PR #1877 的本地异常证据；当前代码基于 `origin/develop` 的 RTK pytest wrapper。

### 当前进度判断

- 已完成：红灯复现、根因确认、最小解析修复、24 条 pytest 夹具、捕获截断观测 fixture、直接二进制退出码/失败位置/无法解析日志验证、两轮独立复核问题修正。
- 进行中：本地提交、远程 PR、PR CI 和主线读回。
- 未完成 / 缺口 / 阻塞：本地全量测试仍有 9 个未触达的 curl/SQLite 基线环境失败；远程 CI 尚未消费本提交。

### RCA / 解决判断

- 现象：原解析器五类计数全为零时直接输出 `No tests collected`；摘要缺失、格式未识别和捕获截断都会进入这条分支。
- 根因或判断：摘要缺失不等于无测试；原逻辑没有保留“是否见到摘要”、`collected` 数量或真实退出码的判定上下文。
- 处理或方案：直接 pytest 改走带真实退出码的既有 runner 入口；解析器单一路径记录 collected、摘要和显式 no-tests 信号，只有显式 no-tests 或 `collected 0 + pytest exit 5` 才输出 no-tests，其余降级为无法解析并保留退出码与失败位置。
- 验证或待决策点：24 条解析夹具、捕获层 10 MiB 边界 fixture、49 条 Python 命令测试、Clippy 和真实二进制 pass/fail/no-tests/exit 4 已通过；最终独立复核、PR exact-head CI 和目标分支读回仍待完成。

### 完整改动逻辑

- 改动前：`pytest` 进入捕获 runner，过滤器只从摘要计数推断结果；所有零计数都被压成 no-tests。
- 触发条件：直接执行 `rtk pytest ...` 时，既有 runner 把真实子进程退出码传给 pytest 解析器；管道输入继续复用兼容入口。
- 唯一主路径：捕获层提供 stdout 与“是否截断”事实 → 清理 ANSI → 读取 collected → 识别唯一终态摘要 → 校验摘要与退出码 → 解析 passed/failed/skipped/xfail/xpass/error/subtests → 输出压缩结果。
- 阻塞 / 失败路径：pytest exit 5 且 collected 不为正才输出 no-tests；无摘要、多摘要、截断、exit 0/1 与摘要冲突、或 exit 2/3/4 异常退出时统一输出“无法解析”、真实退出码、已知 collected、失败位置和原始日志路径。
- 生产者与消费者：pytest 子进程产生 stdout/stderr 与退出码；既有 stream 捕获层额外暴露 stdout 是否超过既有 10 MiB 上限；runner 仅为 pytest 传入该事实并用 stdout+stderr 计算输出预算；`main` 继续消费真实返回码，`pipe_cmd` 继续消费无退出码兼容入口。没有新增旁路 writer 或业务状态文件。
- 兼容与迁移：保留原有 xfail/xpass、失败摘要和管道入口；ANSI、quiet-mode、单等号和子测试摘要进入同一解析主路。
- 回滚：回退本 PR 的 squash commit，恢复原 pytest 过滤器；不需要迁移数据或切换运行时配置。

### 交付物、现阶段结论与下一步

- 交付物 / 证据索引：`src/cmds/python/pytest_cmd.rs`；`outputs/rtk-pytest-truth/pytest-regression-final3.log`；`python-consumers-final2.log`；`clippy-final3.log`；`all-tests-final.log`。
- 现阶段结论：source 层和本地直接消费者层证明 pytest 不再把摘要缺失误报为 no-tests；尚未证明 PR CI、main 或线上消费。
- 下一步：提交并推送分支，创建 PR，等待 required CI，按 exact-head 合并并回读 main；runtime/online regression 不适用。

## Discovery Gate

- task classification: C - 已有 wrapper 的局部 bug 修复
- research_status: not_required
- research_depth: baseline
- formal_research_reason: not_applicable
- internal_sources: `src/cmds/python/pytest_cmd.rs`; `src/core/runner.rs`; `src/core/stream.rs`; `src/cmds/system/pipe_cmd.rs`; `CLAUDE.md`
- external_sources: not_applicable
- source_intake_artifact: not_applicable
- research_conclusion: 复用现有 pytest runner 和 tee 路径，只收紧解析判定
- research_gap: none
- 调研结论: 复用现有主路；不新增通用 timeout、旁路或状态文件
- authority_plan: not_applicable
- test_plan: not_applicable
- test_set: not_applicable
- acceptance_registry: not_applicable
- mechanism_key: not_applicable
- metadata_note: 运行时退出码由既有 runner 产生；本 PR 只改变 pytest 输出层的可信判定和摘要保留。
- related surfaces: source / pytest direct caller / pipe caller / unit fixtures / local binary smoke / PR CI / main
- contracts / fixtures / smoke evidence: pytest parser fixtures and direct binary smoke recorded under `outputs/rtk-pytest-truth/`
- surface-freeze-exempt: not_applicable；只修改一个已有解析器文件和一个合并过程记录
- uncertainty / decision boundary: 10 MiB 捕获上限仍是既有合同；本 PR 只把捕获层已有的 stdout 截断事实暴露给 pytest 单一消费者，不改变上限、截断策略或其他 wrapper 行为。
- 预估命中率依据: not_applicable
- 误报代价: not_applicable
- 退出条件: not_applicable

## Change Summary

- User / project value: 让 RTK pytest 输出只报告已被底层摘要或退出码证明的结论。
- Actual changed behavior: 摘要缺失或不可识别时不再输出 `No tests collected`；直接 pytest 解析获得真实退出码，并保留 collected、子测试和失败位置。
- In scope: `src/cmds/python/pytest_cmd.rs`、`src/core/stream.rs` 的 stdout 截断观测字段、`src/core/runner.rs` 的 pytest 专用 capture-aware 入口与本合并过程记录。
- Out of scope: Safe Runner、超时、捕获上限数值、其他 wrapper 行为、hooks、业务 runtime、生产部署。
- Touched source / callers / tests / config / docs: pytest source and fixtures；stream 只暴露已有截断事实，runner 新入口仅由 pytest 消费，既有调用方默认行为不变。
- Runtime or external impact: none
- external_write: not_applicable
- external_endstate: pending_post_merge

## Validation Evidence

- targeted local / unit: pytest parser 24 passed; stream 10 MiB 截断边界 fixture passed; Python command consumer suite 49 passed; `cargo fmt --all -- --check`; `cargo clippy --all-targets` passed。
- selected stable test cases: normal pass, failure, explicit no-tests, collected-but-summary-missing, quiet/ANSI/non-standard summary, silent interval then pass, 10 MiB 真实捕获截断、伪终态摘要、exit 2/3/4、stderr-only 参数错误、xfail/xpass 和 mixed subtest counts。
- change-specific tests: `outputs/rtk-pytest-truth/pytest-regression-final3.log`
- smoke / E2E when required: real binary pass exit 0; real failure exit 1 with `/private/tmp/...py:2`; empty directory real pytest exit 5。
- PR / CI: pending push and exact-head CI
- skipped checks and reason: full local suite is not clean because 9 unrelated curl tee and tracking SQLite tests fail in this environment; see `all-tests-serial-isolated.log`。
- 旧调用方/旧数据兼容：pipe caller retains the one-argument compatibility function; no data migration。
- legacy fixture 或真实 oracle: parser fixture and real pytest subprocess smoke
- 行为是否变化: describe changed behavior
- online regression: not applicable before merge; no runtime or external consumption surface
- external owner write regression: not_applicable; no external owner write target

## Review Evidence

- required by current risk: yes
- fresh-context review（仅新高风险或当前合同要求，否则 not_applicable）: read-only explorer and code-reviewer review requested; no write access
- Codex review artifact / decision: pending final readback
- Fable review artifact / decision: not_applicable
- accepted findings and applied changes: 首轮发现精确伪摘要、伪 no-tests 横幅和 mixed subtests；第二轮发现 exit 2/3/4、真实捕获截断、stderr-only 空输出和冲突分支丢失败位置。全部收口到单一可信度判断和单一无法解析输出，均有 fixture 或真实二进制 smoke。
- rejected / deferred findings and reason: none；stdout truncation provenance 已按复核意见以只读字段实施，runner 保持唯一日志 writer
- reviewer boundary: review evidence is not CI, runtime, online acceptance, or external readback proof

## Merge Readiness

- source revision: local HEAD (PR head/CI/main pending; read back with `git rev-parse HEAD`)
- target branch: `develop`
- PR URL: pending
- review decision: pending
- required CI: pending
- mergeability: pending
- merge strategy: exact-head squash
- merge owner: repository maintainer
- rollback / revert path: revert the exact squash commit
- ready to merge: no

## Merge Result And Handoff

- merge commit: pending
- post-merge main readback: pending
- promoted runtime required: no
- next required document: not_applicable; no runtime/external acceptance surface
- Promote / runtime owner: not_applicable
- Online acceptance owner: not_applicable
- Final closeout remains blocked until: PR exact-head CI and main readback complete

## RCA 与风险

- 现象：已收集或已执行的 pytest 被压缩器误报为 no-tests。
- 根因或判断：缺少摘要存在性、collected 计数和退出码参与的可信判定；这是展示层误判，不是 pytest 子进程没有执行。
- 处理或方案：单一 exit-aware parser；未知结果保守降级，真实退出码由既有 runner 返回。
- 验证：红灯夹具先证明旧行为；24 条 pytest fixtures、捕获边界 fixture、49 条 Python consumer、真实二进制 smoke 和 Clippy 通过；全量测试的 9 个非相关失败已单独记录。
- 剩余风险：远程 PR CI/目标分支尚未验证；完整 suite 的 9 项环境基线失败仍由各自 owner 处理。
- 待决策点：无新增产品决策；只待外发授权、CI 与合并读回。
