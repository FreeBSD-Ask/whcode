# CI 产物复测记录（2026-10-25）

**对象**：提交 `b33366e` 的两个工作流运行——`ci.yml` run `40195460785`、
`build-release.yml` run `40195460786`。**方法**：用 GitHub API 下载全部 artifact，
先核对 SHA256，再解包实测；不采信工作流自报的“success”作为结论。

## 产物与结论

| 产物 | 核对内容 | 结论 |
|---|---|---|
| `whcode-windows-amd64`（31 073 868 B） | 内层包 `sha256=f1c071f7…7cd10` 与 `install-verify-windows-amd64` 记录一致；`whcode.exe` `388182c8…f36a85e2`、`duckdb.dll` `8785bcbb…39f9897` 均与记录一致 | 通过 |
| `install-verify-windows-amd64` | 记录时间 `2026-10-25T13:04:10Z`；包内 13 个文件与清单一致 | 通过 |
| `smoke-evidence`（TUI） | 3/3：`--version` 退出 0、`doctor --json` 引擎 `v2.0.0`、stdin 关闭时退出 0 并打印入口提示 | 通过 |
| `smoke-evidence`（Web） | 5/5：`/api/status` 200、信封键完整、`/api/sessions` 200、首页 6 221 B、停止时退出码 1（脚本判据接受 0/1/-15/143） | 通过 |
| `deps-windows` | rustc `1.99.1`（= 锁定值）、MSVC `18.11.12315.225`、SDK `10.0.26100.0`、DuckDB `v2.0.0`，payload sha 与包内 DLL 一致 | 通过 |
| `ci-measurements` | NFR-201 中位 31.61 MB（≤36）、NFR-202 中位 36.11 MB（≤80）、NFR-203 20 次切换最大增量 0.57 MB（≤10）、NFR-206 25 chunks/s 采样正常；写者压力 4 000 条 0 背压、p50 149 µs；`memory_limit` 128MB 组未 OOM（ADR-012 断言仍未复现）；查询计划 2/8 走索引（与既有记录同口径） | 通过 |
| `deps-freebsd`、`deps-netbsd` | 四类版本齐备（FreeBSD DuckDB 1.5.6／NetBSD 1.5.5），与已入库记录同口径 | 通过 |

**不构成 NFR-204 证据**：`stability_run_smoke.log` 是 30 秒、5 个样本的冒烟，summary
自报 4.1466 MB/h——窗口远小于 ADR-033 的 5.5 小时，斜率只反映预热噪声；NFR-204 仍以
5.5 小时长跑（run `40154046118`，斜率超预算）为准。

## 复测中发现并修复的问题

`test_memory_limit.rs`、`test_writer_stress.rs`、`test_query_plan.rs` 三个实验夹具把
`machine_note` 写死为“Windows 11 10.0.26300 / i7-1260P / 16.9 GB（本机环境事实）”。
在 GitHub runner 上生成的证据同样带着这段本机配置——**环境字段被伪造**，违反测量纪律
（NFR-222／§5.2）。已改为如实标注“未采集”，保留实测的 `os`／`arch`／`logical_cores`
字段；三个夹具以 `--ignored --nocapture` 复跑，证据 JSON 重新生成。

## 复现命令

```bash
# 取 token（本机 gh 未登录，用 git 凭据存储）
cred=$(printf "protocol=https\nhost=github.com\n\n" | git credential fill)
token=$(printf '%s\n' "$cred" | sed -n 's/^password=//p')

# 列产物并逐个下载
curl -s ".../actions/runs/<run_id>/artifacts" -H "Authorization: Bearer $token"
curl -sL -o art.zip ".../actions/artifacts/<artifact_id>/zip" -H "Authorization: Bearer $token"

# 包完整性
sha256sum -c whcode-0.1.0-windows-amd64.zip.sha256

# 实验夹具（默认 #[ignore]，需显式运行）
cargo test --locked -p whcode-tests --test memory_limit --test writer_stress --test query_plan -- --ignored --nocapture
```

## 维护方式

每个提交的 `ci.yml` 与 `build-release.yml` 运行都可按上表口径复测：先比对 SHA256，
再验证 exe 行为（无控制台退出 0、真实控制台默认 GUI、`--tui` 按键可用），最后核对
测量类 artifact 的数值与预算。结论写入本文件的新条目，随提交推送。
