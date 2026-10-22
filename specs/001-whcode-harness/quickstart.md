# Quickstart: WHCode 验证运行指南

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Date**: 2026-10-20

本文件是**验证/运行指南**：按里程碑给出可执行的最小验证路径与预期结果。命令为接口草案（契约见 [contracts/](./contracts/)），实现在 `tasks.md` 阶段产出。不含实现代码。

## 1. 前置条件

- Rust 工具链：版本以 M0 锁定值为准（`rust-toolchain.toml` 固定），各原生目标分别可原生构建
- DuckDB 原生依赖：官方预编译动态库（构建前执行 `python ci/fetch-duckdb.py`；BSD 平台按 M0 记录改用 pkg/ports 或源码构建）
- 终端：支持 resize 的本地终端（TUI 验证）；BSD 平台另需 SSH 客户端
- 数据集夹具：`tests/fixtures/` 生成 10 万事件与 1 万任务数据集
- 运行时验证（M8）另需：受限 JS 引擎 payload / Python 解释器 / Windows PortableGit / BSD `/bin/sh`

## 2. M0 —— 平台可行性与基线（建议 MVP）

```bash
# 0) 构建前置：拉取并校验 DuckDB 官方预编译动态库（未拉取时构建显式失败）
python ci/fetch-duckdb.py
# 预期：vendor/duckdb/1.5.6/ 就位，载荷 SHA256 与登记值一致（见 docs/adr/ADR-DB-BUILD.md）

# 1) 各原生目标构建与依赖记录（每平台各执行一次）
cargo build --locked --release
python ci/platform-matrix/record_deps.py     # Windows
sh ci/platform-matrix/record-deps.sh         # FreeBSD / NetBSD
# 预期：构建成功；产出 artifacts/deps-<platform>.txt，含 Rust/DuckDB/编译器/OS 四类版本（V-001）

# 1b) 交叉检查（类型检查，不链接）
python ci/platform-matrix/cross-check.py
# 预期：代码级兼容检查完成；交叉检查不构成运行验证，不得替代各平台 V-001/V-002 证据（ADR-010、G-01）

# 2) 数据库冒烟：创建 / 事务写读 / 关闭重启 / 迁移
whcode doctor --json
whcode session new --title smoke && whcode status --json
# 预期：数据库创建成功、事务写读通过、重启后状态一致、迁移可重复执行（V-002）

# 3) TUI 最小闭环
whcode   # 启动 TUI
# 预期：启动、键盘输入、终端 resize、退出、异常恢复全部通过（V-003）

# 4) SSH 原型：FreeBSD/NetBSD 服务端 + 内置客户端（FR-607/FR-611）
cargo test -p whcode-ssh                     # 内置客户端原型测试（认证/受限命令/超时/输出上限/登记）
whcode serve --ssh --port 2222               # 服务端原型（FreeBSD/NetBSD）
ssh -p 2222 -i key user@host                 # 公钥认证、窗口变化、断连
# 预期：客户端侧与 FreeBSD/NetBSD 服务端逐项有记录；无法完成的平台记录明确阻塞原因（V-004）

# 5) 事件写者原型：幂等键、批次提交、关键事件确认、写入错误传播
cargo test -p whcode-store --test writer_prototype
# 预期：重复幂等键去重；批次阈值触发；关键事件 flush 确认；写入失败返回可重试错误（V-005）

# 6) 基线性能报告（固定硬件/数据集/采样方法）
cargo run -p whcode-bin --release -- bench baseline --out artifacts/baseline-$(uname -s).json
# 预期：产出候选指标实测值；冻结或经 ADR 修订 NFR-201—210（V-006）
```

**当前证据状态（2026-10-21）**：

- Windows x86_64（msvc）：构建、依赖记录（`artifacts/deps-windows.txt`）、数据库冒烟四步、存储 14 例与 TUI/SSH 客户端逻辑层测试已通过。
- FreeBSD/NetBSD：脚本与 CI 骨架就绪，本机无平台环境，V-001/V-002 证据阻塞。
- OpenBSD/DragonFly BSD：已移出原生安装目标，改由内置 SSH 客户端控制（FR-611、ADR-011）；客户端连通性证据尚未取得。
- V-003：仅逻辑层测试通过，交互式终端证据未取得，需人工冒烟。
- V-004：服务端验收未取得（属 M7）；SSH 客户端原型 9 例测试通过，四平台连通性证据未取得。
- 性能预算：未冻结（T027—T031 未完成）。

**M0 退出判据**：V-001—V-007 全部通过；D-01/D-02/D-03/D-04/D-06/D-12 形成明确决策；ADR-001—010 批准。不得以“CI 编译通过”单独判定五平台可交付。

## 3. M1 —— 存储与事件

```bash
cargo test -p whcode-store            # schema/迁移/事务/幂等
whcode events export --session S-001 --after 9900 --format jsonl > /tmp/tail.jsonl
whcode db stats --json                # 数据库/WAL/已用块/可用块
whcode db purge --before 2026-01-01 --dry-run
whcode db compact --export /tmp/backup.duckdb   # 复制—校验—切换
```

预期：断线按游标补拉后关键事件完整（V-103）；已确认事务经进程终止+重启后状态一致（V-104）；重复幂等键不重复创建（V-106）；清理有预览/确认/审计与活跃会话保护，compact 副本校验成功后才切换（V-107）。

## 4. M2 —— 应用服务

```bash
cargo test -p whcode-app                     # 命令/权限/错误语义
whcode session list --json
whcode task list --session S-001 --json
# 大数据集：10 万事件会话切换、1 万任务概览
cargo run --release -p whcode-bin -- bench session-switch --session S-BIG --repeat 20
```

预期：会话切换仅加载元数据/概览/有界尾部，分页游标无重复无遗漏（V-101、SC-001）；概览查询报告 p50/p95/p99 并满足冻结预算（V-102）。

## 5. M3 —— LLM 与上下文

```bash
# 使用桩 Provider（固定响应速率与 chunk 大小）
whcode run --session S-001 --model stub --trace-level redacted
whcode events list --session S-001 --after 0 --format jsonl | head
```

预期：疑似秘密字段脱敏且留有标记（FR-203/208）；链路按 request_id/session_id/task_id/tool_call_id 可串联（FR-204）；上下文注入返回 `items + next_cursor + consumed_budget + truncated_reason`（§4.1）；断连重连按游标补齐（V-103）。

## 6. M4 —— SDD 门禁

```bash
whcode spec status --json
whcode spec validate --json
whcode spec analyze --json          # 期望：检出遗漏需求/孤儿任务/无效引用/循环依赖
whcode spec converge --json         # 期望：fail/not_tested 生成差异报告
whcode spec force-advance --reason "..."   # 期望：审计含 actor/reason/门禁/产物版本
```

夹具：`tests/fixtures/sdd/`（违规 Constitution 产物、未解决澄清项、循环依赖、连续解析失败样例）。

## 7. M5/M6/M7 —— 三端与 SSH

```bash
whcode serve --web --bind 127.0.0.1:8787      # 默认仅回环；对外须显式启用认证
whcode serve --ssh --port 2222                # 受限 WHCode 入口，默认无系统 Shell
cargo test --test contract_three_fronts       # 同命令三端一致（V-109）
```

预期：三端最终状态/权限/错误码一致（V-109）；默认安全值生效（V-110）；FreeBSD/NetBSD 服务端验收 + 四平台客户端连通性证据（V-004、FR-611、ADR-011，不以单一平台代替）。

## 8. M8 —— 运行时

```bash
cargo build --locked --no-default-features    # 全运行时关闭
whcode runtime list --json                    # 未启用者不注册工具
cargo test -p whcode-runtime --test timeout_cleanup   # 进程树清理 + stderr 尾部
cargo test --test crash_injection -- --include-ignored  # 三时点中断 → unknown
```

预期：V-301—V-307 逐项通过；能力不足返回结构化错误并给出安装建议（V-306）；不确定副作用标记 `unknown`（V-307）。

## 9. M9 —— 稳定性与发布

```bash
cargo run --release -p whcode-bin -- bench stability --hours 72 --out artifacts/stability.json
cargo run --release -p whcode-bin -- bench report --in artifacts/stability.json
```

预期：记录 RSS/CPU/线程/文件大小/WAL/写者队列深度/最老队列项年龄/提交与查询延迟/事件缺失计数/失败任务数（§11.5）；达标或形成修订预算 ADR；发布清单（§12.2）逐项齐备。

## 10. 验证记录要求

- 每次验证留存**原始产物**（日志、JSON 报告、时间序列），不得只留结论
- 测量必须记录 OS/硬件/编译类型/工具链/DuckDB 版本/数据库参数/数据规模/采样窗口/脚本（NFR 测量纪律）
- 不同平台结果分别报告，不混为一组平均值；分位数与平均值并列
- 失败与阻塞须记录原因与当前实际行为，不得以隐含默认值掩盖未实现项（§15 PR 清单）
