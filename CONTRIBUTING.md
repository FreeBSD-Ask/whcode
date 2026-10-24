# 贡献指南

感谢关注 WHCode。本项目是规格驱动（spec-driven）工程：需求先入基线，实现可追溯，
验收有判据。提交 PR 前请完整阅读本页；提交即表示同意以
[BSD-2-Clause](./LICENSE) 授权本项目，版权主体为 FreeBSD 中文社区（FreeBSD Chinese Community）。

## 1. 环境前置

- Rust：版本由 `rust-toolchain.toml` 锁定（1.99.1，rustup 自动配给）。
  Windows 验收使用 `x86_64-pc-windows-msvc`，**不使用任何 GNU 工具链**（D-01）；
- Python 3：仅用于 `.github/scripts/fetch-duckdb.py`（拉取并校验预编译 DuckDB）与 CI 脚本；
- DuckDB：官方预编译动态库，任何构建前先执行 `python .github/scripts/fetch-duckdb.py`（SHA256 校验）；
  BSD 上改用系统库（环境变量契约见 [docs/guides/bsd-interface.md](./docs/guides/bsd-interface.md)）。

## 2. 开发流程

1. **需求**：所有行为变更先更新 [specs/001-whcode-harness/spec.md](./specs/001-whcode-harness/spec.md)
   （新 FR 或场景修订），并在 [tasks.md](./specs/001-whcode-harness/tasks.md) 登记任务、
   在 [checklist.md](./specs/001-whcode-harness/checklist.md) 登记验收判据——三方缺一即退回；
2. **实现**：按目录分类放置文件（[repository-layout.md](./docs/guides/repository-layout.md)，
   `test_layout` 契约测试强制）；遵守 crate 单向依赖与平台分支收敛（`test_layering` 强制）；
3. **测试**：每个 PR 必须更新正向与失败路径测试——测试不是可选项（任务书 §15）；
   涉及外部副作用必须走“意图—执行—登记”并在测试中覆盖 `unknown` 恢复路径（ADR-007）；
4. **验证**：PR 必须全绿：
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --locked --workspace`
5. **证据**：涉及 `[待验证]` 事项的 PR 必须附实测证据（日志/JSON 产物），不得以推断关闭
   （research.md TBD-01—TBD-10）；性能相关改动必须附测量条件与分位数（§5.2 纪律）。

## 3. PR 清单

提 PR 时逐项勾选 [docs/guides/pr-checklist.md](./docs/guides/pr-checklist.md) 的七项清单；
GitHub 模板已内置（`.github/PULL_REQUEST_TEMPLATE.md`）。清单缺项或“未实现事项未明确标记”
的 PR 不予合并。

## 4. 提交信息

使用 Conventional Commits（`feat:` / `fix:` / `test:` / `docs:` / `ci:` / `refactor:`），
正文说明“为什么”，验证结果在正文末尾列明。示例见 `git log`。

## 5. 文本与本地化约定

- 中文正文与注释使用中文引号“”，不使用英文直引号；禁止出现“”‘’；
- 用户可见文案一律经 i18n 文本层（FR-615），不得硬编码字面量；zh-CN 与 en 双语同步维护；
- 新增源文件建议添加 SPDX 头：`// SPDX-License-Identifier: BSD-2-Clause`。

## 5.1 编码与换行（强制）

- 所有文本文件：**UTF-8 无 BOM、换行符 LF**（Windows 开发同样适用）；
- 由 `.gitattributes` + `.editorconfig` + `rustfmt.toml` 三重强制；
  提交前可用 `python local/normalize_eol.py` 自查；
- 新增二进制类型须在 `.gitattributes` 登记为 `binary`，避免误规范化。

## 6. 平台事项

- FreeBSD/NetBSD 原生构建与 OpenBSD/DragonFly 客户端接入的接口契约见
  [bsd-interface.md](./docs/guides/bsd-interface.md)；
- 平台相关分支只允许出现在 `whcode-platform`（NFR-101，契约测试强制）；
- 不得以交叉编译冒充平台运行验证（G-01、ADR-010）；平台结论以 CI 门禁与留证为准。

## 7. 行为预期

保持技术讨论对事不对人；证据优先于观点；不确定的事说“不确定”。
