# ADR-096：serve loop 测试线程有序停止——A2 SIGSEGV 根因修复

- **状态**：已批准（2026-11-22，用户"继续"；A2 长期 monitoring 项的根因定位与修复）
- **关联**：A2（`docs/guides/long-line-plan.md`）；ADR-037（CI 证据）；FR-793；NFR-211
- **证据**：`artifacts` 之外的 CI artifact `a2-diag-freebsd`（run `45028018422`），
  本地留存 `local/a2-diag-45028018422/`（core-bt.txt／parallel-run.txt／truss-parallel.log）

## 任务认知卡

- **方向**：定位并修复 A2——`whcode-bin` 测试二进制在 FreeBSD 上"全部用例通过后退出路径 SIGSEGV"。
- **目标**：根因可复现、可判定；修复后 FreeBSD `platform-freebsd` job 绿，且不引入线程泄漏或死锁。
- **用户意图**：用户要求继续推进；A2 是长期 monitoring 项，本次复现提供了 core backtrace，具备定位条件。
- **非目标／边界**：不改生产 `serve_loop` 的无限循环语义（生产传 `None`）；不改测试用例的断言；
  不关闭或跳过任何测试；不以"重跑通过"代替根因修复。
- **现有证据（决定性）**：run `45028018422` 的 `core-bt.txt` 显示崩溃线程名为
  `serve-loop-test`，栈顶在 `libduckdb.so` 的
  `__hash_table<...duckdb::LiteralInfo...>::find` +242，`stop reason = signal SIGSEGV`；
  同进程另有多个 `serve-loop-test` 线程停在 `__sys_clock_nanosleep`；`parallel-run.txt`
  尾部为 `process killed, signal = 11`；`whcode-bin` 测试结果为 `156 passed; 0 failed`
  后 `error: test failed`。
- **手段（后置）**：定位线程归属 → 确认生命周期缺陷 → 加可选停止信号 → 测试 Drop 有序停服 → 复验。

## 根因

两个测试基建函数（`spawn_loop_server`／`spawn_loop_server_with`）各自 `spawn` 一个名为
`serve-loop-test` 的线程运行 `serve_loop`，但**丢弃了 `JoinHandle`**；`serve_loop` 是无限
循环（仅错误时返回）。测试结束、进程退出时：

1. 主线程走 `exit` 流程，C++ 静态对象（DuckDB 的 `LiteralInfo` 等全局注册表）开始析构；
2. `serve-loop-test` 线程仍在循环中，可能在 `conn` 上执行查询，访问正在析构的静态表；
3. 跨线程竞争 → `SIGSEGV`，进程以 signal 11 结束。

这解释了两个此前无法解释的现象：**全部用例已通过**（断言无失败）、**崩溃在退出路径**
（不在任何测试内），以及**单线程复跑不复现**（并行压力下线程存活时间窗更大）。
此前把 truss 尾迹当作唯一线索，只看到系统调用而无故障点；core backtrace 直接给出了
崩溃线程名与栈顶，才使根因可判定。

## 决策

1. `serve_loop` 增末位参数 `shutdown: Option<&AtomicBool>`；循环每轮开头检查，
   置位即 `return Ok(())`。**生产调用传 `None`**，无限循环语义不变（NFR-211 零新增线程）。
2. `TestServer` 增 `shutdown: Arc<AtomicBool>` 与 `handle: Option<JoinHandle<()>>`；
   实现 `Drop`：句柄在则置位并 `join`（`IDLE_PARK` 25 ms，最迟一个停靠周期退出）。
3. `TlsTestServer` 改为内嵌 `TestServer` 并实现 `Deref<Target = TestServer>`，
   复用同一 `Drop`；既有 `server.addr`／`server.conn` 使用点零改动。
4. 句柄已移交时 `TestServer::drop` 不停服（`handle.take()` 为 `None` 即跳过），
   避免二次停服或提前停服。

## 理由与约束

- 为何改生产函数签名：`serve_loop` 是私有函数，调用点仅 3 处（1 生产 + 2 测试）；
  可选参数不改变生产行为，且比在测试侧"轮询等待线程自然退出"可靠。
- 为何用 `Drop` 而非显式 `stop()`：测试点 20+ 处，显式调用易漏；`Drop` 与作用域绑定，
  漏不掉。
- 承担者：bin 测试基建维护者；生产无感。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | A2 根因可判定并修复；测试线程有序退出；生产语义零变化；测试点零改动 |
| 负面 | `serve_loop` 多一个参数；测试结束时多一次 join（≤25 ms）；`TlsTestServer` 多一层内嵌 |
| 承担者 | bin 维护者承担参数与 Drop 逻辑；测试套件承担每用例 ≤25 ms 的退出等待 |
| 和解理由 | 以极小侵入消除跨线程析构竞争；若不修，FreeBSD 硬门禁会持续间歇判红 |

## 三重时间

- **过去来源**：A2 自 run `41290577560` 起的多次复现；`long-line-plan.md` 的取证管线
  （`--no-fail-fast`、单线程复跑、`truss -f`、VM 失败标记保全）。
- **现在证据**：core backtrace 的崩溃线程名与栈顶；`156 passed` 后 signal 11；
  `parallel-run.txt` 的 `process killed, signal = 11`。
- **已知限制与失效条件**：①本机（Windows）A2 本不复现，本地只能证明"无回归、无死锁"，
  **不能证明 FreeBSD 已修复**——修复结论须由后续 FreeBSD CI 判定；②若其他测试也存在
  "spawn 线程后丢弃句柄"的模式，可能仍有个别崩溃点，需按同一方法排查；③`Drop` 中的
  `join` 若遇线程内死锁会阻塞测试，需靠 `IDLE_PARK` 上界与超时保护。
- **复审触发**：①修复 push 后 `platform-freebsd` 仍红且 core 栈顶为 `serve-loop-test` →
  说明还有别的存活线程或析构路径，重开定位；②`serve_loop` 增加其他调用点 → 检查是否
  传入 `None`；③`cargo test -p whcode-bin serve::` 出现 hang → 回退本 ADR。

## 影响

- 正面：A2 根因明确、修复可验证；测试基建不再泄漏无限循环线程。
- 负面：`serve_loop` 签名多一参；测试退出多一次 join。
- 不做什么：不改生产循环语义、不跳过测试、不改 CI 门禁判据。
