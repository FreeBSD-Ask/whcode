# ADR-075：PR 基准回归门禁（三指标多轮中位＋相对回归）

- **状态**：已批准（2026-11-15，proposal-v3 P7.2；用户指令“继续功能开发”）
- **来源**：`research/synthesis/proposal-v3.md` W7 P7.2（修订 ADR-025；6—10 单元）；ADR-025（性能预算冻结）；ADR-037（CI 唯一验收）；V-006（测量纪律）
- **责任人**：性能与存储负责人；PR 门禁由 platform-verification 承载
- **退出条件**：相对回归口径出现系统性误判（同一提交多次跑互有红绿）时，回到 dispatch-only 并重开 ADR

## 1. 问题与边界

P7.2 要求“turn 首 token／会话切换／历史页”三个指标进 PR 门禁、多轮取中位。边界与约束：

- 托管 runner 的绝对性能与本机冻结值**不可比**：2026-10-25 已有“托管 runner 上 NFR-207/209
  未达冻结预算而红”的先例，红的是环境不是代码；2026-11-15 再次复现（run
  `43639525007`：NFR-207 p95 7.5 ms 达标、NFR-209 p95 38.6 ms 超 30 ms 冻结值，同一夹具
  本机 28.9 ms）——这不是回归，而是共享盘的环境差异；
- ADR-025 的绝对冻结值（本机 i7-1260P 夹具）继续作为**发布门槛**，只在 `perf-budget`
  （workflow_dispatch 且 `inputs.perf`）通道判定——本决策**不修改**冻结值与判定方式；
- 三个指标共用既有测量纪律（V-006：同夹具、分位数输出、独占 CPU、零样本不充当达标）；
  新增的 turn 首 token 夹具同样 `#[ignore]`、同一串行锁。

## 2. 决策

1. **PR 门禁判定相对回归，不判绝对值**：只与**同一 runner 类别、同一构建剖面（debug）**
   取得的基线比较。基线随仓库版本化（`docs/matrices/perf-baseline-pr.json`），由
   `perf-budget` 通道在托管 runner 上产出（`--write-baseline` → artifact → 人工复核后提交）；
2. **判定公式（两项取宽者，满足任一即通过）**：`current <= baseline × (1 + tolerance)`
   或 `current <= baseline + slack_ms`。`tolerance = 0.30`（托管 runner 同提交的 p95 抖动
   经验值约 ±30%），`slack = 10 ms`（噪声底：基线很小时比率无意义，8 ms 抖 2 ms 已 25%）。
   取**宽**是语义正确的写法——“或”关系的上限是二者的较大值；取严会把小基线的绝对抖动
   误判成回归；
3. **多轮中位**：默认 3 轮，每轮三个夹具各跑一次（`--test-threads=1` 独占），逐指标取
   跨轮中位（ISO/IEC/IEEE 15939 的“指标→测量→分析→判定”链条；中位对尾部离群不敏感）；
4. **三个指标与夹具**（全部 `--ignored`，由门禁脚本以 `--ignored --nocapture` 显式驱动）：

| 指标 | 夹具 | 判据量 | 边界说明 |
|---|---|---|---|
| `turn_first_token_p95_ms` | `p72_turn_first_token_latency`（新增） | p95 | 桩 Provider 立即产 8 chunk：测 harness 就绪延迟，**不含网络与模型思考**；真实 Provider 接入后须真实流另测 |
| `session_switch_p95_ms` | `nfr207_switch_latency_within_frozen_budget` | p95 | 10 万事件切换（复用 ADR-025 夹具） |
| `history_page_p95_ms` | `nfr209_history_single_page_query_p95_within_frozen_budget` | p95 | 尾页单页查询（复用 ADR-025 夹具） |

5. **基线未取得 ≠ 通过**：runner 类别或剖面不匹配时脚本显式输出 `SKIP`（含两边的
   `runner_class` 原文），退出码 0 但**不声称通过**——信号链完备的“正确的空”；基线文件
   缺失同理。只有基线匹配且三指标全部在限内才打印 PASS；
6. **CI 接线**：
   - `platform-verification.yml` 的 windows lane：`github.event_name == 'pull_request'` 时
     运行门禁（`--json artifacts/pr-bench-report.json`，artifact 随 run 上传）；
   - `perf-budget`：测量完成后以 `--write-baseline` 产出基线 artifact；
   - `push main` 不跑门禁（避免每推必测的配额消耗，测量责任归 dispatch 通道）；
7. **自证伪**：`--self-test` 含五条纯函数探针——抖动放行、成倍回归拦截、绝对兜底生效、
   n=0 拒绝、跨轮中位——判定逻辑可被证伪，不靠口头保证。

## 3. 正负和解

**正面**：PR 可以拦“成倍数”的性能回归（T115 那类两阶段分页级别的劣化会在 PR 红掉），
且不需要维护每 PR 的绝对预算；三个指标覆盖“交互响应／切换成本／翻页成本”。

**负面与承担者**：

- PR 增加约 5—6 分钟测量时间（3 轮 × 3 夹具；本机实测每轮约 110 s，其中会话切换
  46 s、历史页 57 s 主要是 10 万事件数据集生成）：由全体 PR 作者承担；若成为瓶颈，
  可下调为 2 轮并附抖动证据，不得直接去掉中位；
- 相对口径对“整块均匀变慢”（例如某依赖升级让一切慢 25%）宽容——由 ADR-025 的
  dispatch 通道与发布门槛兜底；承担者＝性能负责人（基线更新时复核绝对值）；
- 基线漂移需要人工提交（runner 重镜像后 runner_class 变化会触发 SKIP）：由平台负责人
  在 runner 环境变更时重抓基线（`perf-budget` → artifact → 提交），登记为复审触发。

**和解理由**：绝对预算拦“发布是否超标”，相对门禁拦“这次改动是否劣化”——两者口径
不同、互不替代；把两者放进同一判定会同时引入两种误判源（环境噪声 × 绝对值）。

## 3.1 已取得的托管 runner 基线（2026-11-15，run `43640990876`）

`docs/matrices/perf-baseline-pr.json`（`runner_class = Windows-2025Server-10.0.26100-SP0`，
`profile = debug`，3 轮中位）：turn 首 token **85.5 ms**、会话切换 **8.1 ms**、历史页 **30.8 ms**。
产物由 `perf-budget` 通道的基线步生成并以 artifact 交付，经复核后提交；该 job 当轮的绝对
预算步为红（NFR-209 环境差异，见 ADR-025 修订记录），基线步以 `if: always()` 独立产出。

## 4. 三重时间

- **过去**：ADR-025 冻结了绝对预算但明确“托管 runner 不可比”；2026-10-25 的 CI 红是该
  约束的第一次显形；
- **现在**：本机尖峰（`p72` 夹具 debug p95 25.1 ms；`--self-test` 五探针通过）与本轮
  dispatch 产出的 runner 基线；
- **将来失效条件与复审触发**（绑定命令＋判定）：同一提交的两次门禁结果互有红绿
  （系统性误判）→ 回退 dispatch-only；runner 重镜像 / profile 变更 → SKIP 增多，
  需重抓基线并复核；新增真实 Provider 后 `p72` 须以真实流另测（桩口径只承诺 harness）。

## 5. 回退方案

删除 PR 步骤（保留 perf-budget 的基线导出亦可）；`.github/scripts/benchmarks/pr-bench-gate.py`
与夹具无害保留。回退后 PR 性能回归由 dispatch 通道的人审承担。

## 6. 已知限制与失效条件（复审触发，绑定命令＋判定）

- **桩口径**：`p72` 只测 harness 路径；判定该指标代表“端到端首 token”即属伪造；
  接入真实 Provider 时触发重测（ADR-075 复审条件）；
- **基线不匹配**：`runner_class`／`profile` 变化 → 门禁 SKIP（显式、不声称通过）；
  连续两次 dispatch 产基线后仍 SKIP → 复审 runner 类别口径；
- **tolerance/slack 修订**：任何参数变更须同 PR 附“同提交双跑”的抖动证据（两次运行
  的三指标中位差），不得拍脑袋放宽；
- **测量独占**：门禁与 `perf-budget` 一样要求独占 CPU/磁盘；并行改动（未来 CI 拆分）
  不得与其它重负载步骤同跑。

## 7. 文献依据（标准研读记录，§1.6 口径）

- ISO/IEC/IEEE 15939:2017（已在基线登记，判定=已覆盖）：测量过程链
  （establish indicators → collect/analyze → interpret & decide）——本决策的
  “指标→测量→分析→判定”即按该链落位，门禁参数（tolerance/slack）属其“decision criteria”；
- ISO/IEC 25010:2023（已登记，部分覆盖）：性能效率的 time behaviour 子特性——三个
  指标都是时间特性的操作化；25010 不规定测量方法，方法落在夹具与脚本（与现行口径一致）；
- 仓库内部一手依据：V-006 测量纪律（spec §15）、ADR-025 冻结表与 2026-10-25 CI 红
  （`artifacts/progress-review-2026-10-23.md` 与 checklist 复核记录）、
  `docs/guides/performance-and-concurrency.md`（测量纪律与 ADR-015 边界）。

## 8. 形式审查

- **真实矛盾**：性能预算“只在发布时判”会让回归在合并后才被发现；而“PR 判绝对值”
  会把环境噪声当回归。P7.2 的真实矛盾在两者之间，相对回归是可判定的中间态；
- **正负和解**：见 §3，代价（PR 时长、人工基线、宽容均匀变慢）与承担者显式；
- **0 信任校验**：判定逻辑有 `--self-test` 证伪探针；基线/报告 JSON 记录原始分位数行，
  缺指标与 n=0 一律 FAIL；SKIP 的输出包含双方 `runner_class` 原文，可人工复核；
- **知难行易**：方向＝P7.2（proposal 明文）；目标＝三指标进 PR 门禁、多轮中位、可判定；
  手段＝相对回归＋版本化基线，是在“托管 runner 绝对值不可比”约束下的选择，选型晚于认知。
