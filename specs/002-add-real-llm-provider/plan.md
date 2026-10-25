# Implementation Plan: 真实 LLM Provider 接入（add-real-llm-provider）

- **Branch**：`002-add-real-llm-provider` | **Date**：2026-10-25（补登记） |
  **Spec**：[spec.md](./spec.md)
- **Input**：`specs/002-add-real-llm-provider/spec.md`、ADR-031、ADR-032、
  主账本 T162—T167
- **前置事实**：设计决策（ADR-031）与部分实现（提交 `3f9564c`）先于本计划入库；
  本计划为 G2 的补登记，**不追认门禁按序完成**

## Summary

在 `whcode-llm` 内以可选 `http` feature 提供真实供应商传输：同步 `ureq` 客户端 +
`rustls` TLS，剖面声明接入点，五种线协议适配到统一的增量／终止／用量语义，
三格式流解码全程有界，HTTP 与传输错误归一为既有错误码。网络依赖只在装配根
（`whcode-bin`）启用；没有调用点，因此“默认不联网”当前结构性成立。
四端 `llm.run`／`llm.cancel` 接线未交付，登记保持 `Reserved`。

## Technical Context

**Language/Version**：Rust 1.99.1（`rust-toolchain.toml`；MSRV 声明 1.99）

**Primary Dependencies**（以 `Cargo.lock` 为准）：

| 依赖 | 版本 | 特性与角色 |
|---|---|---|
| `ureq` | 3.5.0 | `default-features = false`，仅 `rustls`；同步阻塞客户端，可选依赖 |
| `rustls` | 0.23.46 | `std`／`tls12`／`ring`／`logging`；不启用默认特性（避免第二加密提供者） |
| `ring` | 0.17.14 | `rustls` 的加密提供者（含 C 与汇编，第三方内部实现） |
| `webpki-roots` | 1.0.11 | `ureq` 传递依赖的内置 Mozilla 根集 |
| `rustls-native-certs` | 0.8.5 | 相邻变更 T188（ADR-040）用于 `system` 档位；不在本变更交付范围 |

**Storage**：无（本变更不触碰 DuckDB schema 与写入路径）

**Testing**：`cargo test`（单元 + 契约）；纯函数夹具覆盖协议与帧解码；
本地回环桩服务器用于后续客户端任务；不访问公网；CI 为测试结论唯一入口（ADR-037）

**Target Platform**：Windows 11 amd64（`x86_64-pc-windows-msvc`，ADR-013）；
BSD 延后，代码级兼容保留，不作验收结论

**Project Type**：多 crate workspace 的领域服务 crate（`whcode-llm`）+ 装配根（`whcode-bin`）

**Performance Goals**：流式期间 CPU ≤ 8% 单核（NFR-206）；解码与错误正文有界；
不新增每会话常驻线程；发布体积在接入调用点后必须重测（当前 0 字节增量不是承诺）

**Constraints**：默认不联网；凭据只来自环境变量或仓库外文件；追踪默认 `redacted`；
零 `unsafe`（NFR-215）；不使用 GNU 工具链；不引入异步运行时（先过 ADR-015 门槛）

**Scale/Scope**：1 个同步客户端、5 种线协议、3 种帧格式；本次已入库代码
约 1.5 千行（提交 `3f9564c`：`profile` 367、`protocols` 718、`frames` 302、`errors` 159）

## Constitution Check

| # | 门禁 | 判据来源 | 结论 |
|---|---|---|---|
| C-01 | 默认持久化后端仅 DuckDB | 任务书封面、ADR-001 | PASS——本变更不涉及存储 |
| C-02 | 架构基线不可绕过；变更必须走 ADR | §2 | PASS——ADR-031（网络与凭据）、ADR-032（四端协调）；**时序倒置已如实记录** |
| C-03 | 前端不得直接触库 | ADR-003／005 | PASS——前端零改动；`test_layering` 覆盖 |
| C-04 | 外部副作用不得承诺 exactly-once | §0.3-8、ADR-007 | PASS——不自动重试流式生成，避免重复计费与 chunk 序号破坏（ADR-031 决策 8） |
| C-05 | 不受信任脚本不得进主进程 | ADR-008 | 不适用——本变更不执行模型输出作为指令 |
| C-06 | 未验证能力不得表述为已交付 | §0.1、NFR-2xx | PASS——未交付项逐条标注，四端保持 `Reserved` |
| C-07 | 需求属性标记保持 | §0.1／§0.2 | PASS——复用既有 FR 编号，不新造编号 |
| C-08 | SDD 门禁由程序实施 | G-05、FR-308 | 不适用——本变更不改门禁状态机 |

**违规**：无新增违例。**已知流程违例**：G2 产物晚于实现（门禁倒置），已在 ADR-031
“决策时序”与 `spec.md` §9 登记；补救为补齐 G1／G2 产物，不放宽完成判据。

## Project Structure

### 本变更涉及的文档与代码

```text
specs/002-add-real-llm-provider/
├── spec.md          # 本变更需求与交付状态（补登记）
├── plan.md          # 本文件（技术路线、模块映射、测试策略、顺序与回滚）
└── tasks.md         # 变更内任务分解（引用主账本 T162—T167，不设第二账本）
```

```text
crates/whcode-llm/
├── Cargo.toml                   # ureq／rustls 可选依赖 + http feature（默认关闭）
└── src/
    ├── http/
    │   ├── mod.rs               # 模块声明；客户端与 llm run 装配属未交付
    │   ├── profile.rs           # 剖面、协议枚举、凭据、端点与时限（已交付）
    │   ├── frames.rs            # SSE／NDJSON／JSON 数组流有界解码（已交付）
    │   ├── protocols.rs         # 五协议适配：请求构造、帧语义、用量归一（已交付）
    │   ├── errors.rs            # HTTP 状态与传输错误归一（已交付）
    │   └── tls.rs               # 出站信任装配（相邻变更 T188／ADR-040，不属本变更）
    ├── provider.rs              # Provider trait、CancelToken、传输快照钩子（已交付）
    └── transport_events.rs      # 传输进度事件与快照（T132 基线，已交付）
crates/whcode-bin/Cargo.toml     # 唯一启用 http feature 的装配根（无请求调用点）
```

**Structure Decision**：真实传输留在 `whcode-llm`（领域服务），不新建 crate；
装配根只声明依赖与后续注入点，保持“网络依赖只进装配根”的单向边界（ADR-031 决策 1）。

## 技术路线

1. **依赖门控**：`ureq` 与 `rustls` 设为 `optional`，由 `http` feature 聚合；
   `whcode-llm` 默认 `default = []`，唯一启用者为 `whcode-bin`。
   关闭 feature 后 `cargo tree -p whcode-bin -i ureq` 必须不匹配任何包（可判伪）。
2. **同步客户端**：一次请求占一个线程，复用现有线程模型，不引入 tokio；
   若将来需要异步，先按 ADR-015 产出并发设计档案（不变量、线性化点、内存序、
   进展保证、失败模式、回退）。
3. **剖面驱动**：`ProviderProfile` 描述协议、端点、路径／查询、模型、鉴权、额外头体、
   超时三元组与 `max_tokens`；托管变体（Azure、Vertex）经剖面覆盖，不写厂商分支。
4. **端点策略**：`https://` 强制；明文 `http://` 仅回环；非法端点返回可操作错误。
   已知宽松项（userinfo、IDN、端口格式）归 T162 加固。
5. **凭据策略**：`CredentialRef::Env`／`CredentialRef::File` 两种来源；`Secret` 的
   `Debug`／`Serialize` 恒为 `[REDACTED]`；错误只含变量名或文件路径。
   鉴权头／查询参数的**实际注入**尚未实现（无消费者），归 T162／T163。
6. **请求构造**：`WireProtocol::request_path`／`request_body` 合并 `extra_body`，
   同键以剖面为准；协议必需头（如 `anthropic-version`）由适配器提供。
7. **解码**：`FrameDecoder`（SSE／NDJSON／JSON 数组）逐帧产出 `Frame`；
   单帧 1 MiB、单响应 64 MiB 上限；畸形 JSON 与超限返回 `parse_error`；
   未知事件映射 `Unknown` 并要求计数，不得静默。
8. **归一**：协议帧 → `DecodeOutcome` → chunk 语义（增量、终止原因、供应商用量）；
   错误 → `normalize_status`／`normalize_transport` → 既有 `ErrorCode` 与 `details`；
   供应商正文只用于提取消息，不作为控制指令。
9. **传输可观察**：`TransportTracker` 按连接聚合字节、RTT（≤64 采样）、重连次数与
   断连原因码，产出 `ProgressChanged` 事件；`Provider::transport_snapshot` 默认 `None`，
   真实 Provider 接入后返回快照（FR-616）。
10. **不做自动重试流式生成**：仅幂等探测类请求允许最多 2 次指数退避（1 s、2 s），
    且只对连接失败与未产生增量的 5xx 生效；当前实现未交付（T163）。

## 模块映射

| 模块 | 职责 | 对应需求 | 门控／交付状态 |
|---|---|---|---|
| `http/profile.rs` | 剖面、协议枚举、凭据、端点与时限 | FR-203、FR-616、NFR-213 | 无 feature 门控；已交付 |
| `http/frames.rs` | 三格式有界流解码 | FR-206、FR-208、NFR-215 | 无 feature 门控；已交付 |
| `http/protocols.rs` | 五协议请求构造、帧语义、用量归一 | FR-206、FR-207、FR-208 | 无 feature 门控；已交付（协议层） |
| `http/errors.rs` | 状态／传输错误归一与有界正文 | FR-201、FR-616 | 无 feature 门控；已交付 |
| `http/mod.rs` | 模块声明与边界说明 | — | 无 feature 门控；客户端未交付 |
| `provider.rs` | `Provider` trait、`CancelToken`、快照钩子 | FR-201、FR-202、FR-207 | 基线已交付；真实实现未交付 |
| `transport_events.rs` | 传输事件与只读快照 | FR-616、FR-208 | 基线已交付（T132）；数据源未交付 |
| `whcode-bin/Cargo.toml` | 唯一启用 `http` feature | ADR-031 决策 1 | 已启用；无调用点 |
| `http/tls.rs` | 出站信任装配 | FR-626、ADR-040 | `http` feature 内；**相邻变更 T188，不属本变更** |

## 测试策略

### 已有测试（本变更范围内）

| 层 | 位置 | 覆盖 | 数量 |
|---|---|---|---|
| 单元 | `http/profile.rs` | 端点校验、`Secret` 不泄漏、五协议解析 | 3 |
| 单元 | `http/frames.rs` | SSE 事件名与多行 data、NDJSON 空行、JSON 数组增量、超限报错 | 4 |
| 单元 | `http/protocols.rs` | 五协议请求体与帧解码、错误事件、`extra_body` 合并与类型拒绝、未知帧不 panic | 10 |
| 单元 | `http/errors.rs` | 状态映射、`details` 不含秘密面、超时与 IO 区分、`Retry-After` 只解析秒 | 4 |

`cargo test -p whcode-llm` 的基线记录为 39 passed／0 failed（ADR-031，含非 `http` 模块）；
当前源码静态计数为 50 个 `#[test]`（含 `http/tls.rs` 11 个，属相邻变更）。

### 必须补的测试（随剩余任务）

1. **凭据文件分支**：不可读、为空、只有空白三种失败路径（T162）。
2. **端点负向用例**：userinfo、IDN／punycode、异常端口（T162）。
3. **脱敏端到端**：捕获日志与事件，断言凭据值不出现（T167）；当前只有模型层断言。
4. **默认不联网契约**：用 `cargo metadata`／`cargo tree` 断言非装配根成员未启用
   `http` feature，且 feature 关闭时依赖图中无 `ureq`；当前只有人工核验（缺口）。
5. **客户端失败路径**：本地回环桩服务器上的连接拒绝、读取超时、取消、错误状态与
   错误正文上限（T163／T166）；不访问公网。
6. **计量入账**：供应商用量与本地估算在事件中可区分（T164）。

### 测试纪律

- 不访问公网；本地 TLS 夹具用 `rcgen` 自签证书（与 T188 一致）；
- 不得把凭据写入夹具或日志；测试使用假值并断言脱敏；
- 结论以 CI（`.github/workflows/ci.yml`）日志与 artifact 为准；本地运行只作复现（ADR-037）；
- 不得用 `#[ignore]` 藏起失败用例。

## 交付顺序（主账本 T162 → T167）

| 顺序 | 主账本 | 剩余交付内容 | 完成判据 |
|---|---|---|---|
| 1 | T162 | 端点加固（userinfo／IDN／端口）；`File` 凭据测试；鉴权头／查询参数注入函数与单测 | 上述负向用例通过；鉴权注入有“头值不含密钥原文于错误/日志”的测试 |
| 2 | T163 | `ureq::Agent` 构造（时限、信任根注入点）、请求发送与响应读取、探测类有界重试 | 本地回环桩服务器上的成功与失败路径测试；`cargo tree` 门控不变 |
| 3 | T164 | `DecodeOutcome` → `LlmChunk` 流、终止原因、供应商用量写入事件与计量 | 桩服务器流式夹具断言 chunk 序号连续、用量标记正确 |
| 4 | T165 | 将 `TransportTracker` 接入客户端：连接、断开、重连、字节、RTT、原因码 | 事件载荷可从 CUI JSON 导出；RTT 采样有界 |
| 5 | T166 | 时限执行、取消在读循环生效、重试策略（仅幂等探测） | 超时／取消／重试各自有正反用例；取消生效时机写入错误说明 |
| 6 | T167 | 装配根凭据文件读取、Provider 注册与 `llm run` 命令面、四端登记翻转与矩阵同步 | 四端同 PR 翻转 `Reserved`→`Implemented` 并补真实锚点；ADR-032 修订记录更新 |

**并行约束**：T167 的登记翻转必须与前端接线同一 PR，提前翻转即“绿灯下的事实漂移”
（ADR-032 2026-10-25 修订记录）。T163 与 T166 共享客户端代码，不建议并行。

## 回滚与依赖风险

### 回滚路径

1. **关闭 feature**：从 `whcode-bin/Cargo.toml` 移除 `features = ["http"]`，
   `cargo tree` 中 `ureq`／`rustls`／`ring` 消失；代码保留但不参与编译（已实测可判）。
2. **前端不可见**：四端登记未翻转，回滚不影响任何用户入口；CUI 仍返回 55、Web 仍返回 501。
3. **无数据迁移**：不触碰 DuckDB schema 与事件表，回滚无数据风险。
4. **相邻变更隔离**：`http/tls.rs` 属 T188／ADR-040，可按该变更自己的回滚路径摘除，
   不影响本变更的协议层与错误归一。

### 依赖风险

| 风险 | 影响 | 处置 |
|---|---|---|
| `ring` 含 C 与汇编，且有安全公告历史 | 第三方安全面与许可证审计面扩大 | 版本锁定；关注 RUSTSEC 公告；自有代码零 `unsafe` 承诺不覆盖其内部 |
| `rustls` 默认不读系统证书库 | 企业 MITM 代理默认失败 | 相邻变更 T188／ADR-040 提供 `builtin`／`system` 两档与 `ca-extra.pem` |
| 同步客户端在慢响应下占用线程 | Web 单线程服务可能被阻塞 | T163 接入时限制并发入口；Web 端接线需单独评估（属 T167 前端任务） |
| 接入调用点后发布体积未知 | 发布包膨胀无据 | 接入后重测并记录；不得沿用“0 字节”结论 |
| 版本注记漂移（ADR 记 `webpki-roots 1.0.10`，锁文件 1.0.11） | 文档与事实不一致 | 本计划采信锁文件；ADR 修订由主代理决定 |
| 端点校验宽松 | 可能接受带 userinfo 的 URL | 归 T162 加固并补负向用例 |

## 形式审查、正负和解与三重时间

本计划的正面／负面／承担者／和解理由与三重时间已在 `spec.md` §7、§8 记录，
此处不复制；设计层补充两点：

- **形式**：`http` feature + 剖面 + 协议适配器三者共同构成“可关闭的网络面”；
  去掉任一环（feature 门控、剖面权威、适配器统一语义）都会让默认离线或供应商差异
  重新变成口头约定。
- **失效条件**：出现异步、HTTP/2、QUIC、mTLS 或企业代理需求时，本计划的技术路线
  （同步客户端、单请求一线程、rustls 内置根）需要重写而非扩展。
