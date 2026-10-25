# ADR-031：真实 LLM Provider 的网络接入与凭据边界

- **状态**：已补录（2026-10-25，用户指令“补遗 ADR-031 未入库”）；决策内容以提交 `3f9564c`
  的实现事实与既有 FR／NFR 为依据，**属事后登记**（门禁倒置见“决策时序”）
- **来源**：`add-real-llm-provider` 变更（未入库 spec），任务位 T162—T167；
  D-16（`docs/matrices/decisions.md`）；`CHANGELOG.md` 2026-10-25 已知未完成第 4 项
- **责任人**：LLM 变更维护者、安全与可观察性负责人
- **退出条件**：需要异步／HTTP2／QUIC、需要企业代理或自定义 CA、需要 mTLS、`ureq` 停止维护
  或 `rustls`／`ring` 出现需处置的安全公告时，修订本 ADR 并给出迁移与回退证据

## 背景

项目已有 LLM Provider 抽象（`whcode-llm`：流式 `Provider`、计量、追踪、传输事件），
但真实供应商接入需要网络栈。网络依赖进入装配根会同时影响四件事：**默认是否联网**、
**依赖与发布体积**、**凭据来源与泄漏面**、**超时与重试边界**。四者都必须先有决策，
否则“本地优先、默认回环、凭据不落日志”的既有和解会被悄悄改写。

## 决策

1. **网络依赖只进装配根**：`ureq 3.5.0`（`default-features = false`，仅 `rustls`）作为
   `whcode-llm` 的可选依赖，由 `http` feature 承载；`whcode-llm` 默认不启用，唯一启用者为
   `whcode-bin`。`whcode-domain` 与四个前端 crate 保持零网络依赖（`test_layering` 与
   `cargo tree` 可复核）。
2. **不使用 OpenSSL、不要求系统 TLS 库**：TLS 由 `rustls 0.23.46` 提供，信任根为
   `webpki-roots 1.0.10`（内置 Mozilla 根集）。`ring 0.17.14` 是第三方依赖内部实现
   （含 C 与汇编），**不构成**项目自有 Rust 的 `unsafe` 例外（NFR-215 只约束自有 crate）；
   Windows 构建仍只用 msvc，不含 GNU 工具链（D-01）。
3. **阻塞式客户端，不引入异步运行时**：`ureq` 为同步 API；本项目现有线程模型（CUI 单次
   请求、Web 单线程服务器、TUI 事件循环、SSH 执行线程）直接复用。若将来引入异步运行时，
   必须先补 ADR-015 的并发设计档案（不变量、线性化点、内存序、进展保证、失败模式、回退）。
4. **默认不联网**：无剖面即无请求。`llm run` 只有在用户显式选择剖面（配置或参数）时才可
   发起网络调用；启动、`doctor`、`serve`、TUI／Web 空闲路径不得发起任何网络请求。
   当前版本**尚无任何调用点**（`grep -rn ureq crates` 只命中清单与注释），因此“默认不联网”
   在当前版本是**结构性成立**而非仅靠开关约定。
5. **端点方案**：`https://` 强制；明文 `http://` 仅允许回环（`127.0.0.1`、`localhost`、
   `[::1]`），与 FR-606／NFR-213 的默认回环口径一致。端点校验在
   `ProviderProfile::validate_endpoint`，非法端点返回可操作错误而非静默回退。
6. **凭据来源与泄漏面**：只允许环境变量（`CredentialRef::Env`）或**仓库外**显式文件
   （`CredentialRef::File`）。密钥值封装在 `Secret`，其 `Debug`／`Serialize` 恒为
   `[REDACTED]`；错误消息只含变量名或文件路径，不含值；密钥不写入数据库、事件、日志与
   测试夹具（FR-203、NFR-213）。文件路径本身不是秘密，允许出现在可操作错误中。
7. **时限与输出上限**：默认连接 10 s、读取 120 s（两次读取之间）、总时限 300 s、
   `max_tokens = 1024`；全部可按剖面覆盖。流帧解码有界：单帧 1 MiB、单次响应累计 64 MiB，
   超限返回 `parse_error`，不静默截断（NFR-215）。
8. **重试**（决策已定，实现未交付）：**不自动重试流式生成请求**（可能已产生计费与增量，
  自动重放会重复计费且破坏 chunk 序号）；仅对幂等的探测类请求（模型／能力查询）允许
   有界重试：最多 2 次、指数退避（1 s、2 s）、只对连接失败与 5xx 且**未产生任何增量**的
   响应生效；每次尝试写入 FR-616 的传输事件（连接、断连、原因码）。实现归属 T163。
9. **协议枚举属工程决策，不新造 FR**：五种线协议（OpenAI 兼容、Anthropic、Gemini、Ollama、
   Cohere）与剖面字段由本 ADR 记录；协议适配器新增只增实现与夹具，不改领域 `Provider`
   接口。此前代码注释引用的 FR-209—214 在 `spec.md` 中不存在（幻影编号），已改为既有
   编号并在本节登记原因。

## 决策时序（门禁倒置，如实记录）

依赖与模块先于本 ADR 入库（提交 `3f9564c`），违反“G2 设计先行、禁止先实现再补设计”的
既有纪律。本 ADR 是对该既成事实的**补录**，不放宽门禁：T162—T167 仍未完成、变更 spec
（`.trae/specs/add-real-llm-provider/`）未入库，`llm.run` 在四端登记表仍为 `Reserved`。
补录同时修正了两处信号链缺口：

1. `http/protocols.rs` 与 `http/errors.rs` 已入库但**未在 `mod.rs` 声明**——14 个测试从未
   参与编译（“文件存在”不等于“测试在跑”）。已补 `pub mod errors; pub mod protocols;`
   并修正 `protocols.rs` 的 `String::push` 字符字面量类型错误（`"&"` → `'&'`）。
2. 代码注释引用不存在的 FR-209—214 与 `161.x／162.x` 小节号；已改为 FR-201／FR-203／
   FR-206／FR-207／FR-208／FR-616、NFR-213／NFR-215 与 ADR-031。

## 证据

依赖链（`cargo tree -p whcode-bin -i ureq`，2026-10-25）：

```text
ureq v3.5.0
└── whcode-llm v0.1.0
    ├── whcode-app v0.1.0
    │   ├── whcode-bin v0.1.0
    │   ├── whcode-cui／gui／ssh／tui／web v0.1.0
    └── whcode-bin v0.1.0
```

`rustls 0.23.46 → ring 0.17.14、rustls-pki-types 1.15.1、rustls-webpki 0.103.16；
ureq-proto 0.6.5；webpki-roots 1.0.10`（`artifacts/deps-windows.txt`）。临时移除
`features = ["http"]` 后 `cargo tree -p whcode-bin -i ureq` 输出
“package ID specification `ureq` did not match any packages”，证明 feature 门控有效。

**发布体积**（2026-10-25，Windows 11 amd64，`cargo build -p whcode-bin --release` 三次）：

| 构建 | `whcode.exe` 字节数 |
|---|---|
| 带 `http` feature（现状） | 21 124 096 |
| 临时移除 feature | 21 124 096 |
| 还原 feature 后重建 | 21 124 096 |

增量为 **0 字节**：当前无调用点时链接器不保留未被引用的 `ureq`／`rustls` 目标代码。
三次构建的 SHA256 各不相同（构建元数据非确定），因此本结论只以字节数为准，不宣称
逐字节可复现。**接入调用点后必须重测**，届时体积增量不得无据宣称。

单元测试（`cargo test -p whcode-llm`）：39 passed／0 failed，其中 `http::protocols` 与
`http::errors` 的 14 个用例为本次接线后首次实际运行。

## 正负和解

- **正面**：网络能力进入项目但保持“只有装配根能联网”；TLS 不依赖系统库与 OpenSSL；
  凭据只有两种来源且默认脱敏；超时三元组显式；流解码有界；默认离线可被结构性验证。
- **负面**：发布二进制在接入调用点后会增大（本轮实测 0 字节，接入后须重测）；引入
  `ring`（含 C／汇编）扩大了第三方依赖的安全面与许可证审计面；rustls 不读系统证书库，
  企业 MITM 代理的自定义 CA 默认不受信任；同步客户端在慢响应下占用调用线程；重试尚未实现，
  当前对瞬时故障只能由用户重试。
- **承担者**：LLM 变更维护者（T162—T167 实现）、安全复核人（依赖安全公告）、
  企业网络用户（自定义 CA／代理需另立方案）。
- **和解理由**：以“依赖只进装配根 + 无调用点即零网络”换取默认离线的可验证性；以同步客户端
  避免为单请求引入异步运行时（NFR-216 的设计门槛）；以 rustls 避免 OpenSSL 的系统依赖
  与许可证复杂度，接受“系统证书库不生效”的运维代价。

## 三重时间

- **过去来源**：`spec.md` FR-201（统一事件类型）、FR-203（追踪级别与秘密不落日志）、
  FR-206（流式合并不丢关键转换）、FR-207（计量）、FR-208（完整性标记）、FR-616（传输进度）、
  NFR-213（默认回环与凭据纪律）、NFR-215（零 unsafe 与有界缓冲）、D-05（追踪默认
  `redacted`）、D-16（Provider 接入决策位）。
- **现在证据**：`crates/whcode-llm/src/http/{profile,frames,protocols,errors}.rs` 与
  `crates/whcode-bin/Cargo.toml`；上文的依赖链、体积实测与测试输出；`artifacts/deps-windows.txt`。
- **将来失效条件**：需要 HTTP/2 多路复用、QUIC、异步并发或取消更强语义；需要代理、
  自定义 CA、mTLS；`ureq`／`rustls`／`ring` 维护状态变化或安全公告；发布体积增量超过
  可接受阈值（由复审时给出实测数值再定阈值）。

## 0 信任校验

- **查证（一手）**：依赖版本来自 `Cargo.lock`／`artifacts/deps-windows.txt`；调用点缺失由
  `grep -rn ureq crates` 直接核验；端点与凭据语义来自源码与对应单测。
- **交叉验证**：`cargo tree`（依赖存在）／移除 feature 后 `cargo tree` 报不存在（门控生效）／
  三次 release 构建（体积）／`cargo test -p whcode-llm`（行为）——四个独立来源一致。
- **判定**：依赖与凭据边界为确证；“体积增量为 0”为**当前状态**的确证，不推广为接入后的结论；
  企业代理与自定义 CA 为**未支持**，标 `[待验证]`，不得默认可用。

## 已知未完成与缺口

1. **客户端与 `llm run` 装配未交付**（T162—T163 未完成）：当前无任何网络调用点，
   四端 `llm.run` 仍登记为 `Reserved`、CUI 返回退出码 55。
2. **重试未实现**：决策见第 8 条，实现归 T163；在此之前瞬时故障只能由用户重试。
3. **端点校验待加固**：`validate_endpoint` 未拒绝 URL 中的 userinfo（`https://user:pass@host`）、
   未做 IDN／punycode 归一、对端口格式校验宽松；归 T162 一并补齐并加负向用例。
4. **代理与自定义 CA 未支持**：`ureq` 的代理配置与自定义根证书未接线；企业环境需另立方案。
5. **变更 spec 未入库**：`add-real-llm-provider` 的需求文档缺失，T162—T167 的完成判据只能
   由任务行承担；补齐 spec 后须复核本 ADR 的需求映射。

## 复审触发

- 任一 crate 在 `http` feature 关闭路径出现网络依赖（`cargo tree` 可判）；
- 发布体积增量超阈值或依赖许可证变化；
- `ureq`／`rustls`／`ring`／`webpki-roots` 安全公告；
- 需要异步、代理、自定义 CA、mTLS、HTTP/2；
- T162—T167 任一项状态变化（完成或重新排期）。

## 修订记录（2026-10-25，ADR-040）

- **触发的既有条款**：“已知未完成与缺口”第 4 项（“代理与自定义 CA 未支持”）与头部退出
  条件“需要企业代理或自定义 CA”已被用户指令（内置安全证书、可编辑、可启用关闭）触发，
  因此按既有纪律走“修订而非沿用”。
- **现状改写（仅此两条语义，其余原文不动）**：决策 2 的“`rustls` 不读系统证书库”改为
  **默认 `builtin`（可编辑的内置 Mozilla 根证书副本），可切换 `system`（OS 证书库）**；
  企业／自定义 CA 由“未支持”转为**受控支持**——`ca-extra.pem` 在两种档位都追加生效，
  且任何路径都不得关闭证书链与主机名校验。同一语义在“正负和解·负面”中“`rustls` 不读系统
  证书库”一句亦由本节取代，原文其余条目（含自定义 CA 的运维代价）继续有效。
- **仍未支持**：企业代理（`ureq` 代理配置未接线）、mTLS；退出条件中的这两项继续有效。
- **实现归属**：出站信任装配落在 `whcode-llm` 的 `http` feature 内（`http::tls`），
  证书库与编辑面见 ADR-040；本条不改变“网络依赖只进装配根”的既有边界，
  也不改变“默认不联网”（无剖面即无请求）。
- **证据状态**：ADR-040 的依赖表与工作区证书库为设计决策；真实 Provider 调用仍归
  T163／T167，四端 `llm.run` 仍为 `Reserved`，不得据本条宣称已交付。
