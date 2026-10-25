# Tasks: 真实 LLM Provider 接入（add-real-llm-provider）

- **变更标识**：`add-real-llm-provider`（正式目录 `specs/002-add-real-llm-provider/`）
- **账本约定**：本文件**不设完成勾选框，也不复制主账本状态**。任务完成状态、依赖与验收
  仍以 `specs/001-whcode-harness/tasks.md` 的 T162—T167 与
  `specs/001-whcode-harness/checklist.md` 为唯一账本；本文件只做**变更内分解**与
  **证据映射**，避免第二账本与“绿灯下的事实漂移”
- **执行前提**：ADR-031 已补录（G2 产物，但时序晚于实现）；本 spec／plan／tasks 为补登记
- **纪律**：密钥不得进入日志、事件、错误 `details`、测试夹具与提交内容；不伪造通过；
  未取得证据标“未交付／未取得”；临时脚本与运行日志放 `local/`（不入库）
- **命令约定**：除特别说明外均在仓库根执行；文本 UTF-8 无 BOM、LF；不使用 GNU 工具链
- **依赖原则**：仅 `whcode-llm` 的可选 `http` feature 承载网络依赖，唯一启用者为
  `whcode-bin`；不引入异步运行时与第二加密提供者；自有 crate 零 `unsafe`

## 1. 任务分解

| 分解编号 | 主账本 | 本变更范围 | 交付状态 | 证据锚点 | 剩余工作与完成判据 |
|---|---|---|---|---|---|
| S-01 | T162 | 剖面模型、协议枚举、凭据边界、端点与时限 | **部分交付** | `crates/whcode-llm/src/http/profile.rs`（367 行）；单测 `endpoint_https_required_and_loopback_http_allowed`、`secret_never_leaks_via_debug_or_serialize`、`protocol_parse_lists_all_five` | 剩余：鉴权头／查询参数**注入函数**（`AuthScheme` 目前只有数据模型，无消费者）；端点加固（userinfo、IDN／punycode、端口格式）；`CredentialRef::File` 的不可读／为空单测。判据：三项各有正反用例，且错误与日志不含密钥原文 |
| S-02 | T163 | 阻塞式客户端与协议适配器 | **部分交付** | `http/mod.rs`（模块声明）、`http/frames.rs`（302 行）、`http/protocols.rs`（718 行）、`http/errors.rs`（159 行）；`http` 模块单测 21 例 | 剩余：`ureq::Agent` 构造（连接／读取／总时限、信任根注入点）、请求发送与流式读取、探测类有界重试（ADR-031 决策 8）。判据：本地回环桩服务器上的成功与五类失败路径测试；不访问公网 |
| S-03 | T164 | 响应归一化（流式／非流式 → 领域事件与用量） | **部分交付** | `http/protocols.rs` 的 `DecodeOutcome::{Delta,Finish,DeltaAndFinish,Unknown,Error}` 与 `usage` 归一（`cost_is_estimate = false`）；`metering.rs` 本地估算 | 剩余：`DecodeOutcome` → `LlmChunk` 流、chunk 序号连续性校验、终止原因与用量写入事件与计量。判据：桩服务器夹具断言序号连续、供应商计量与估算可区分 |
| S-04 | T165 | 传输进度事件与脱敏 | **部分交付** | `crates/whcode-llm/src/transport_events.rs`（T132 基线，2 例单测：字节／RTT／重连／原因码、RTT 有界 64）；`Provider::transport_snapshot` 钩子 | 剩余：把 `TransportTracker` 接入真实连接（连接／断开／字节／RTT／原因码数据源）。判据：事件可经 CUI JSON 导出，RTT 采样有界，断连原因码非空 |
| S-05 | T166 | 超时、重试与取消 | **部分交付（仅模型与错误归一）** | `profile.rs` 的三元组时限与 `timeouts()`；`errors.rs::normalize_transport`（超时／IO 区分）；`provider.rs` 的 `CancelToken`（桩语义，测试 `cancel_takes_effect_before_next_chunk`） | 剩余：时限执行、读循环中的取消生效、仅幂等探测的有界重试。判据：三者的正反用例；取消生效时机写入错误说明（§8.4 口径） |
| S-06 | T167 | 装配根接线与登记翻转 | **部分交付（仅 feature 声明）** | `crates/whcode-bin/Cargo.toml` 启用 `whcode-llm` 的 `http` feature；无 HTTP 请求调用点（`grep -rn "ureq::" crates` 仅命中 `http/tls.rs` 的 Agent 构造，属相邻变更 T188） | 剩余：装配根凭据文件读取、Provider 注册表、`llm run` 命令面、四端登记翻转与矩阵同步。判据：四端同 PR 翻转 `Reserved`→`Implemented` 并补真实锚点，同步 ADR-032 修订记录与对等门禁断言 |
| S-07 | —（本次补登记） | 变更 spec／plan／tasks 入库 | **已完成（工作树，待主代理提交）** | `specs/002-add-real-llm-provider/{spec,plan,tasks}.md`；目录命名通过 `specs_follow_change_id_naming` | 剩余：由主代理提交入库；不得代改主账本 |
| S-08 | —（主代理决定） | 主账本措辞同步 | **未执行（越权边界）** | 建议见 §3 | 主代理决定是否采纳，并同步 `tasks.md` / `checklist.md` 的阻塞原因措辞 |
| S-09 | —（随 T167） | 契约文档：剖面 schema 与 `llm run` 命令面 | **未交付** | 无 `contracts/` 文件；剖面字段现由 `profile.rs` 的 serde 结构与单测承担 | 与 T167 同批补 `contracts/`（剖面字段、凭据来源、超时默认值、错误码映射），并标注“交付前不生效” |
| S-10 | —（随全量门禁） | 全量 `fmt`／`clippy`／`test` 与 CI 取证 | **未执行（本次范围外）** | 本次只执行目录分类门禁与 EOL 门禁，结果见 §2 | 由主代理在合入批次执行；不得以本地结果冒充 CI（ADR-037） |

## 2. 本次执行的门禁与结果

| 门禁 | 命令 | 结果 | 说明 |
|---|---|---|---|
| 目录分类 | `cargo test -p whcode-tests --test layout` | **5 passed / 1 failed** | `specs/002-add-real-llm-provider` 的命名用例 `specs_follow_change_id_naming` 通过；失败项仅为 `top_level_entries_follow_layout`，原因是仓库根存在并行会话的运行残留：首次运行为空目录 `tmp/`，复跑时为 `whcode.duckdb` 与 `whcode.duckdb.wal`（`.gitignore` 已排除，但测试按磁盘枚举）。两者均非本变更创建。本变更**未修改白名单**：为运行残留加白名单会放宽既有分类规则，且不属本变更范围；清理由产生残留的会话或主代理负责 |
| 编码与换行 | `python .github/scripts/check-eol.py` | **通过**（跟踪文件 517，跳过二进制 69；CRLF 0，BOM 0） | 该脚本只审计 git 跟踪文件，新文件尚未暂存 |
| 新文件自查 | 逐文件解码 | **通过** | `spec.md`、`plan.md`（以及本文件）均为 UTF-8 无 BOM、LF |

**未执行**：`cargo fmt`／`clippy`／全量 `test`、`traceability.py`、`check_counts.py`、
`check-workflows.py`。原因：本批次只新增 `specs/002/` 文档，不触碰 `specs/001/` 与代码；
`traceability.py` 与 `check_counts.py` 只解析 `specs/001/`，不受本变更影响。
主账本同步与全量门禁由主代理在合入时执行。

## 3. 给主账本的措辞建议（待主代理决定）

1. **Phase 15 阻塞原因**：由“**_未完成（阻塞：变更 spec 未入库；ADR-031 已补录）_**”
   改为“**_未完成（阻塞：客户端装配与四端接线未交付；变更 spec 已入库，见
   `specs/002-add-real-llm-provider/`）_**”。六条任务行同此口径。
2. **Phase 15 说明段**：由“该变更的正式 spec 仍未入库；…标题与交付状态以该变更收口时的
   正式 spec 为准”改为“正式 spec 已于 2026-10-25 补登记入库
   （`specs/002-add-real-llm-provider/`，状态：代码早于 spec，门禁倒置由 ADR-031 记录）；
   任务标题与交付状态**仍以主账本为准**，变更内分解见该目录 `tasks.md`”。
3. **ADR-031 缺口 5**：由“变更 spec 未入库”改为“变更 spec 已补登记
   （`specs/002-add-real-llm-provider/`）；ADR-031 的需求映射已可在该 spec 的 FR 表复核”。
4. **ADR-031 版本注记**：`webpki-roots 1.0.10` 与 `Cargo.lock` 的 `1.0.11` 不一致，
   建议在 ADR 修订记录中改正（本变更未修改 ADR）。
5. **`checklist.md`**：T162—T167 的检查点仍由主账本收口时登记；本变更不代行。

## 4. 收口判据（供主账本引用）

本变更达到“可宣称交付”需同时满足：

1. T162—T167 在主账本勾选，且每项有可复现命令或测试锚点；
2. 四端 `llm.run`／`llm.cancel` 同 PR 翻转登记并补真实锚点（ADR-032 复审触发③）；
3. 凭据不落日志／事件的**端到端**断言存在（不只是模型层）；
4. 默认不联网有可执行契约（非仅 `cargo tree` 人工核验）；
5. 接入调用点后的发布体积重新实测并记录；
6. 全量 `fmt`／`clippy`／`test` 与 CI 证据由 GitHub Actions 日志与 artifact 承担（ADR-037）。

未满足任一项时，本变更状态为“未完成”，四端不得宣称 `llm run` 可用。
