# ADR-078：LSP 四查询客户端（P5.3，封闭集；模型工具面属 P5.3b）

- **状态**：已批准（2026-11-15，proposal-v3 P5.3；用户指令“继续功能开发”）
- **来源**：`research/synthesis/proposal-v3.md` W5（P5.3“LSP 四查询（封闭集：
  definition/references/hover/diagnostics）”）；`whcode-mcp` 的 JSON-RPC 客户端先例
  （ADR-054）；ADR-071（有界出站通道＋线程 join 无常驻泄漏的并发先例）
- **责任人**：工具系统维护者、语言服务维护者、装配根维护者
- **退出条件**：见“已知限制与失效条件”——服务器兼容性矩阵出现系统性失配，或 LSP 3.19
  转正带来封闭集外字段需求时复审

## 1. 问题与边界

模型当前只有文件/状态类工具（九个内置）＋ shell（ADR-070），无法回答“这个符号在哪定义、
谁引用它、悬停说明是什么、这里有什么编译诊断”。这些是编码 harness 的核心检索面
（DSH 域 Q 判定“产品主线”），且语言服务器已在用户机器上（编辑器生态标配），**复用现成的
LSP 服务器**比自造索引便宜得多、准确得多。

边界：

- **封闭集**：只做四个查询（definition／references／hover／diagnostics）＋ didOpen 同步；
  不做 completion／rename／formatting／code action 等（proposal 明确封闭集，防面扩大）；
- **不内置服务器**：不捆绑任何 language server（与运行时 payload 同口径，D-07；离线优先），
  用户经 `.zcode/lsp.json` 显式 opt-in（与 `.zcode/mcp.json`／`shell.json` 同模式）；
- **本批（P5.3a）＝库**：`whcode-lsp` crate 的配置／传输／客户端＋四查询 API，以 stub 服务器
  测试；**模型工具面（`lsp_*` 四工具、注册与对等登记）属 P5.3b**，不在本批声称交付。

## 2. 决策

1. **新 crate `whcode-lsp`**（第 16 个成员，领域服务层）：依赖 `whcode-domain`（错误）、
   `whcode-runtime`（子进程环境收敛与凭据剥离的**单一实现**，沿 `whcode-mcp::session` 的
   先例）＋ serde/serde_json；不依赖 `whcode-tools`（工具面属 P5.3b，届时按 ADR-070 的
   注入模式接线，不反向依赖）。
2. **传输＝LSP 3.18 基础协议**（一手规范，2026-11-15 取读）：头与体以 `\r\n` 分隔，头块以
   `\r\n\r\n` 结束，必填头 `Content-Length`（字节），可选 `Content-Type`（仅支持 UTF-8）；
   体为 JSON-RPC 2.0。消息上限 [`MAX_MESSAGE_BYTES`]（1 MiB）——超限**显式报错**，不截断
   （截断会产生伪 JSON）。
3. **有界性**（NFR-215／R-05）：服务器数 ≤4、单 server 参数 ≤16 条、扩展名 ≤16 条；
   位置结果 ≤100 条；hover 文本 ≤2000 字符；诊断 ≤100 条、单条 message ≤500 字符；
   请求 deadline 默认 5 s、上限 30 s。所有结果上限**钳制并显式标记**（不静默丢弃后冒充完整）。
4. **生命周期与线程**：客户端按“查询期”存在（构造→initialize→查询→Drop），**非会话常驻**；
   读取走单 reader 线程＋有界通道（容量 64），Drop＝kill 子进程→join reader（复用 ADR-071
   的“全线程 join 无常驻泄漏”口径，测试断言）。每查询新建客户端（冷启动延迟由 LSP 服务器
   承担，登记为已知限制）。
5. **四查询的封闭映射**（宽松解析＋忽略未知字段，`serde` 默认行为；只取封闭字段）：
   - `textDocument/definition`：`Location | Location[] | LocationLink[] | null` →
     `Vec<LspLocation>`（link 取 `targetUri`＋`targetSelectionRange`）；
   - `textDocument/references`（`context.includeDeclaration` 显式传参）：`Location[] | null`；
   - `textDocument/hover`：`null → None`；`contents` 接受 `string | {value} | [(string|{value})…]`
     三形态并折叠为纯文本（bounded）；
   - 诊断：先 `textDocument/diagnostic` **拉取**（`DocumentDiagnosticReport`，`kind=="full"`
     取 `items`）；期间收到的 `textDocument/publishDiagnostics` **推送**缓存入有界 map
     （≤64 个 uri）；拉取不被支持（方法级错误）时回退到缓存，缓存也没有则显式报错——
     不返回伪“零诊断”（正确的空 vs 错误的空，§1.4）。
6. **查询前必 didOpen**：以磁盘内容发送 `textDocument/didOpen`（多数服务器要求）；
   文件不可读或超过 [`MAX_OPEN_BYTES`]（2 MiB）显式报错。
7. **权限与副作用**：本批为库层，无工具注册（P5.3b）；`didOpen` 只读磁盘不写盘；子进程
   环境最小白名单（`PATH`／`SYSTEMROOT`／`TEMP`／`TMP`／`HOME`）＋**无凭据传递**——命令行
   与参数由用户配置文件给出，不经模型（本批无模型入口）。
8. **非目标**：completion／rename／formatting／workspace symbol、多根 workspace、
   增量同步（`textDocumentSync` 只用 didOpen 全量）、模型工具注册（P5.3b）、GUI 面。

## 3. 正负和解

**正面**：四个高价值检索面一次补齐；复用现成服务器，索引质量与编辑器一致；封闭集＋
宽松解析让兼容面可测（stub 服务器逐形态覆盖）。

**负面与承担者**：

- 冷启动：每查询新建客户端，大项目首查可能数秒——由用户承担，P5.3b 后可评估客户端复用
  （需要会话生命周期决策，属 D13 议题的近亲）；
- 服务器兼容性：宽松解析覆盖规范三形态，但真实服务器仍有方言——由兼容矩阵与失效条件承担；
- `didOpen` 全量发送大文件（≤2 MiB）产生内存与传输开销——有界并显式报错；
- 新增第 16 个 crate 的门禁面（layering／layout／AGENTS／module-design 同步）——本批承担。

**和解理由**：语言智能是“找回与理解代码”的核心能力，封闭集＋复用服务器是成本最低的
正确路径；负面均有界、显式、可复审。

## 4. 三重时间

- **过去**：proposal P5.3 明确封闭集；`whcode-mcp` 已验证 JSON-RPC 客户端与有界通道模式；
- **现在**：LSP 3.18 为 Current（3.17 Previous，3.19 Upcoming，官方页 2026-11-15 实取）；
  本批实现与 stub 测试（帧解析、三形态 hover、link/数组/空 result、推送缓存回退）；
- **将来失效条件**（绑定命令＋判定）：LSP 3.19 转 Current 且引入封闭集外必答字段 → 复审；
  `cargo test -p whcode-lsp` 任一红 → 停止交付；真实服务器兼容性抽样（rust-analyzer／
  typescript-language-server）出现系统性失配 → 扩充兼容矩阵或收紧解析。

## 5. 回退方案

纯新增 crate（无迁移、无依赖变更、无 schema 变更）：删除 `crates/whcode-lsp/` 与门禁白名单
条目即可；P5.3b 未开始，无下游耦合。

## 6. 已知限制与失效条件

- **封闭集外不支持**：补全、重命名、格式化等显式拒绝（`Unsupported`），不冒充“暂不支持”；
- **每查询冷启动**：无客户端复用；大项目首查延迟显著；
- **单文件同步**：只 didOpen 查询目标文件，未打开依赖文件，跳转结果可能不全（服务器侧限制）；
- **拉取诊断非强制**：服务器不支持时回退推送缓存；两者皆无则显式报错；
- **未接真实服务器**：本批以 stub 服务器测试协议形态；rust-analyzer 等真实服务器的兼容性
  未取证，**不得声称“已验证与 rust-analyzer 兼容”**；
- **失效条件与复审触发（绑定命令＋判定）**：
  - `cargo test -p whcode-lsp` 任一红 → 停止交付；
  - `python .github/scripts/check-code-scanning.py` 出现 LSP 相关 open 告警 → 修复后重推；
  - `.zcode/lsp.json` 配置非法 → 显式错误（坏 JSON／超限），不得静默当“未配置”；
  - LSP 3.19 转 Current → 复审本 ADR（与 `standards-baseline.md` LSP 行联动）。

## 7. 文献依据（标准研读记录，§1.6 口径）

- LSP 规范 3.18（Microsoft/language-server-protocol 官方站，2026-11-15 取读，页面标注
  Current）：基础协议帧（`\r\n` 分隔、`Content-Length` 必填、UTF-8 唯一支持编码）；
  四查询方法名与能力门槛（`definitionProvider`／`referencesProvider`／`hoverProvider`／
  `diagnosticProvider`）。已登记 `standards-baseline.md`（部分覆盖：封闭集）；
- `lsp-types` crate 文档（metaModel 派生类型，证据层级：官方规范的机器派生）——
  `Hover{contents, range?}`、`Diagnostic{range, severity?, code?, source?, message, …}`
  字段核对，用于自持最小结构体而非引依赖；
- ISO/IEC 25010:2023（已登记）：functional appropriateness——四查询覆盖“定位／引用／解释／
  诊断”四个真实任务面，不做功能囤积。

## 8. 形式审查

- **真实矛盾**：模型无法回答“符号在哪定义／谁在用”，只能靠文本搜索近似——误报率高且无
  语义；自造索引则重复造轮子且离线不可得；
- **正负和解**：见 §3，冷启动与兼容面显式，触发条件可判定；
- **0 信任校验**：帧规则与能力门槛取自官方规范页（非二手转述）；三形态解析由 stub 测试
  实证；`full` 上限与钳制由测试断言；
- **知难行易**：方向＝P5.3 四查询；目标＝库层四查询可用且有界；判据＝stub 服务器全绿＋
  门禁全绿；非目标＝模型工具面（P5.3b）与真实服务器兼容声明；手段（自持最小结构体＋宽松
  解析＋有界通道）在确认“不引 lsp-types 依赖、不内置服务器”之后确定。

## 修订记录（2026-11-17：`.zcode` 更名 `.whcode`，用户指令）

**触发**：用户指令「全部改为 whcode 前缀」——产品名为 WHCode，工作区配置目录沿用 ZCode
的 `.zcode` 前缀造成品牌歧义；ZCode 配置互操作的价值由用户裁量终止。

**决策增量**：本 ADR 记录的配置路径 `.zcode/…` 统一更名 `.whcode/…`。只改名字：schema、
上限、显式 opt-in、fail-closed 语义与测试判据全部不变。既有工作区的 `.zcode/*` 配置
需手动迁移至 `.whcode/*`。历史正文、`research/zcode-comparison/` 与 `local/zcode-src`
证据锚点保留原名不改写。
