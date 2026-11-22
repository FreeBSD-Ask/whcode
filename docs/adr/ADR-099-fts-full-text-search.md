# ADR-099：会话全文检索（DuckDB fts 扩展，随包分发）

- **状态**：已批准（2026-11-23，用户明确批准 FTS，并指示“发行包体积本身不构成任何限制”）
- **关联**：ADR-076（关键词检索第一步，FTS 显式延后）；FR-746；NFR-215（有界）；
  `research/dsh-comparison` 域 L §4.4（FTS 扩展的允许项）
- **上游依据**：ZCode／DSH 均提供全文检索；WHCode 此前因“扩展需联网安装”而延后

## 任务认知卡

- **方向**：在关键词子串检索之外，提供按相关度排序的全文检索。
- **目标**：fts 扩展随包分发、离线可用；检索按 `match_bm25` 排序；扩展缺失时**回退**到既有
  `contains` 子串检索，不硬失败。
- **用户意图**：用户批准 FTS，并明确发行包体积不设限——不必为 44.8 MB 扩展做取舍。
- **非目标／边界**：不替换既有 `search_messages`（保留为默认与回退路径）；不做跨会话聚合
  排序之外的语义检索；不改 `messages` 表结构；不引入联网 INSTALL（离线优先）。
- **现有证据（2026-11-23 实测）**：`INSTALL fts` 成功、`LOAD '<绝对路径>'` 成功、
  `PRAGMA create_fts_index('t','id','body')` 成功；扩展文件 46 990 358 字节（约 44.8 MB），
  元数据 `core` 类、带 sha256；`/vendor` 被 gitignore，扩展应由 `fetch-duckdb.py` 获取。
- **手段（后置）**：fetch 脚本增扩展下载与校验 → store 层加载与索引 → 检索接口 →
  回退路径 → 测试。

## 问题

`search_messages` 是 `contains(lower(...))` 子串匹配：无词边界、无相关度、无排序依据
（只按时间倒序）。用户按“聊过什么”找回会话时，无法让最相关的消息排前；也无法匹配
“词干/多词”等常见检索需求。

## 决策

1. **扩展分发**：`fetch-duckdb.py` 增加 `fts.duckdb_extension` 下载（按平台：
   `windows_amd64`／`freebsd_amd64`／`windows_arm64`），校验 sha256 与字节数，落到
   `vendor/duckdb/<ver>/<target>/fts.duckdb_extension`。**不入库**（`/vendor` 已忽略），
   与 duckdb.dll 同一分发路径。
2. **加载**：store 层新增 `fts_available(conn) -> bool`——先尝试 `LOAD '<vendor 路径>'`，
   失败再试 `LOAD fts`（用户已安装时），两者皆失败返回 false。结果在进程内按连接缓存
   （避免每次检索重复尝试）。
3. **索引**：新增 `fts_index_meta(session_id, indexed_max_ts)` 表（迁移 0012），记录每会话
   已索引到的 `max(messages.ts)`。检索前若 `max(ts) > indexed_max_ts` 或索引不存在，
   执行 `PRAGMA create_fts_index('messages','id','content_ref', overwrite=1)` 并更新 meta。
4. **检索**：新增 `search_messages_ranked(conn, needle, limit)`——扩展可用时用
   `fts_main_messages.match_bm25` 排序（按分数降序，同分按时间倒序），返回同一
   `ContentHit` 结构；扩展不可用或索引失败时**回退** `search_messages`（并在返回值
   标注 `ranked: false`，不静默改变语义）。
5. **有界性**：沿用 `MAX_SEARCH_NEEDLE_CHARS`／`MAX_SEARCH_LIMIT`／`SNIPPET_CHARS`；
   索引重建按会话范围（`create_fts_index` 是表级——本 ADR 接受全表重建，见限制）。

## 理由与约束

- 为何接受全表重建：DuckDB 的 `create_fts_index` 是表级操作，无会话级增量接口；WHCode 的
  单文件库规模以会话数据为界，重建成本随消息量线性增长。以 `indexed_max_ts` 判断“是否需要
  重建”，避免每次检索都重建（无新消息时零成本）。
- 为何保留 `contains` 路径：扩展缺失（未分发、平台不支持、加载失败）时必须可用；这是
  “正确的降级”而非“静默失败”——返回 `ranked: false` 让调用方知道排序语义未生效。
- 为何不联网 INSTALL：违反本地优先与离线可用（AGENTS.md §1.1 安全优先序）。
- 承担者：store 维护者；发布流程需为三平台准备扩展（体积约 45 MB/平台，用户已明确不设限）。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 相关度排序、词边界匹配；离线可用；扩展缺失时自动回退，不阻断检索 |
| 负面 | 发布包增约 45 MB/平台；索引重建为表级、随消息量线性；`fts_index_meta` 增一表与迁移 |
| 承担者 | 发布流程承担体积与三平台扩展准备；store 维护者承担索引时效逻辑 |
| 和解理由 | 用户明确接受体积；以“按需重建＋回退”控制运行期成本，不以 FTS 替代既有检索 |

## 三重时间

- **过去来源**：ADR-076 的“FTS 显式延后”与失效条件；DSH／ZCode 的全文检索能力。
- **现在证据**：2026-11-23 实测 INSTALL／LOAD（绝对路径）／create_fts_index 均成功；
  扩展 44.8 MB、带 sha256；`/vendor` 已忽略，扩展可走同一分发路径。
- **已知限制与失效条件**：①`create_fts_index` 为表级全量重建，大库（>100 万消息）时
  单次重建可能达数秒——按 ADR-076 复审触发（消息 >100 万且检索 p95 >200 ms）再评估增量；
  ②中文分词依赖 DuckDB fts 的 tokenizer（未做中文分词定制），中文检索质量**未验证**，
  须实测标注；③扩展版本必须与 DuckDB 2.0.0 精确匹配，升级 DuckDB 须同步换扩展；
  ④FreeBSD／ARM64 的扩展可得性未在本机验证，须 CI 取证。
- **复审触发**：①`cargo test -p whcode-store search::` 失败→回退；②DuckDB 版本升级→
  重取扩展并复测；③中文检索实测不达可用→改为“英文/标识符优先”并如实标注；④索引重建
  耗时超过检索预算→改为增量或按会话分表。

## 影响

- 正面：相关度排序检索；离线；可回退。
- 负面：包体积 +45 MB/平台；表级重建；新增迁移与 meta 表。
- 不做什么：不替换 `contains` 默认路径、不联网 INSTALL、不改 messages 表结构、不做语义检索。
