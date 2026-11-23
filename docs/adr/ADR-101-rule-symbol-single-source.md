# ADR-101：规则符号单一事实源与四支柱治理重构

- **状态**：已批准（2026-11-23，用户“全部批准，不计成本”）
- **关联**：ADR-086（权威层级）；ADR-085（融洽性审计）；ADR-046（证据态）；
  `docs/analysis/software-engineering-logic-sufficiency.md` §4.2
- **上游依据**：2026-11-23 全仓审计发现六类问题共用一个根因——同一事实存在多个手写副本，
  缺乏机器强制的同步；其中 R 编号三方语义分叉使“pre 零反例”与“Γ 实际一致”脱钩

## 任务认知卡

- **方向**：把 Γ 从散文集合＋手工计数重构为可执行规范内核＋受校验文档投影。
- **目标**：建立符号注册表、计数单一事实源、原子提交门禁、状态注释清零、判据蕴含升级
  五支柱，使结构片段的漂移由机器阻断。
- **用户意图**：复述——不计成本地执行上一轮审计报告的全部五支柱建议。
- **非目标／边界**：不宣称 Γ 全域一致；语义片段 Σ_m 的不可判定性仍由用户预言机与抽样承担；
  不修改用户未提交工作树内容（`turn.rs`、`event.rs`、`ADR-100`）。
- **现有证据**：本轮实跑 pre 零反例但 mid 五条 soft 反例；台账 59→60、75→81 数字漂移；
  R1—R8 在三处指不同命题。
- **手段（后置）**：`symbols.toml`＋`coherence.py` 直接谓词＋`check_counts.py --json`＋
  `check-docs-style.py` 模式检查＋`traceability.py` 蕴含判据。

## 问题

六类问题（R 编号分叉、台账数字陈旧、工作树脏、注释滞后、计数快照过时、覆盖率注释偏差）
各自修补会边际成本递增且无法阻止下一次漂移。需要在根因层建立单一事实源。

## 决策

分五支柱，按依赖顺序落地：

1. **符号注册表**：新建 `coherence/symbols.toml`，把推理规则拆为 inference（R1—R8）与
   coherence（CR1—CR7）两个不重叠命名空间；`axiom-system.md` §3 改用 CR 前缀；
   `coherence.py` 增 `symbol_unambiguous` 谓词与 `COH-M1-07` 硬条目，歧义即阻断。
2. **计数单一事实源**：`check_counts.py` 增 `--json`，台账 evidence 改机器引用或历史快照标注。
3. **原子提交门禁**：`coherence.py` 增孤立 ADR 检测（已有 FR／任务／Check 引用为零即反例），
   `end` 阶段工作树脏由记录升为提示；FR 幻影引用（代码中引用 spec 不存在的 FR）登记反例。
4. **状态注释清零**：移除 crate 头部“骨架／未实现事项／M* 起步／待交付”状态声称，
   文风门禁加源码模式检查；运行时 `not_supported` 语义保留。
5. **判据蕴含升级**：Check 增 `polarity=`／`implies_fr=` 字段；`traceability.py` 检查
   正面 FR 有正面极 Check 蕴含；存量负极性覆盖按台账批次回填。

## 形式审查记录

```text
形式：治理重构，涉及规范文件与门禁脚本，不改产品功能代码
矛盾：规则体系能报结构反例，却无法保证自身符号与计数不漂移；不重构则“全绿”持续误导
正面：漂移由机器阻断；“待办是否充分”可计算；R 编号歧义消除
负面：新增注册表与谓词成为维护对象；三处文档符号改名有引用断裂风险；存量回填成本高
过去依据：2026-11-17 两轮审计建立 45 条目录＋46 条可执行公理；本轮实测漂移扩大
现在依据：pre total=47、counterexample=0（支柱一后）；mid 五条 soft 反例仍在台账
将来失效条件：注册表与文档再次不同步，或 check_counts 输出格式变化即失效
0 信任校验：每个谓词配负向自检；每个计数给复现命令；未取得项显式登记
自由影响：不改用户默认值与退出权；不放宽任何硬边界
安全和解：本 ADR 与新增脚本不含凭据与会话标识
复审触发：symbols.toml 的 guard 正则失配、COH-M1-07 转 counterexample、或账本计数再次漂移即复审
```

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 六类漂移根除；pre 零反例重新与 Γ 一致挂钩；充分性判据可算 |
| 负面 | 新增注册表与 3 个谓词成为维护对象；符号改名与存量回填成本 |
| 承担者 | 规则修订者维护注册表；批次执行者承担回填；用户承担裁决等待 |
| 和解理由 | 台账数字已漂移两次且无机制阻止第三次；一次迁移＋机器强制低于无限补丁 |

## 三重时间

- **过去来源**：ADR-085 融洽性审计、ADR-086 权威层级、2026-11-17 公理充分性审计。
- **现在证据**：2026-11-23 实跑 pre／mid／台账与源码逐域审阅。
- **已知限制与失效条件**：①只覆盖结构片段 Σ_s，语义片段仍不可判定；②符号注册表依赖
  文本锚点，规则改写会触发误报，按修锚点处理不放宽匹配；③存量回填未完成时 mid 仍有反例。

## 复审触发（绑定命令）

1. `python .github/scripts/coherence.py --phase pre` 的 COH-M1-07 转 counterexample ⇒ 复审符号域。
2. `python .github/scripts/coherence.py --selftest` 的符号负向探针不再命中 ⇒ 复审谓词。
3. `python .github/scripts/check_counts.py` 输出与账本声明再次不一致 ⇒ 复审支柱二。
4. 本 ADR 一年未被任何批次引用，或用户判定未改变任何门禁决策 ⇒ 降级为分析文档。

## 影响（2026-11-23 落地后回填）

- 新增：`coherence/symbols.toml`；4 条公理（COH-M1-07／M1-08、COH-A6-03、COH-A2-06）；
  5 个直接谓词（`symbol_unambiguous`／`orphan_adr`／`phantom_fr`／`implies_fr` 及 selftest 探针）；
  台账条目 COH-CE-008（closed）、009／010（open）、006／007（closed）。
- 改动：`axiom-system.md` §3 与 T5 改用 CR 前缀；`axioms.toml` 头部命名空间声明；
  `check_counts.py --json`（counts/v1）；`check-docs-style.py` 增过期状态声称模式；
  7 个 crate 头注释清理；`spec.md` FR-406 极性字段；`checklist.md` Check 172 蕴含字段。
- 附带修复：ADR-100 的 `steer` 字段在 7 处跨模块 `TurnRequest` 构造点补 `None`
  （原为编译阻断，非 ADR-101 本体；`turn.rs` 是你的未提交工作，仅补构造点未改其逻辑）。
- 不做：不改产品功能逻辑；不替用户完成 ADR-100 的 `drain_one` 循环接线（该项仍为 dead_code 警告）。

**落地实测**：`coherence.py --selftest` OK；`--phase pre` total=50／counterexample=0；
`--phase mid` counterexample=3（均登记）；`traceability.py` 零断链（FR=186／任务=426／Check=498）；
`check_counts.py` 一致；文风/EOL/ADR/workflow/action 门禁全绿；`cargo fmt --all --check` 通过。

**2026-11-23 续批（用户“批准继续”）**：
- ADR-100 主循环接线完成（每步 drain_one → user 消息 + log-only SteerAccepted 事件），
  `drain_one` 的 dead_code 消除，`cargo clippy -p whcode-app --all-targets -D warnings` 通过。
- FR-808 正式立入 spec（T426／Check 498），LLM 模块 4 处历史幻影 FR 引用清除；COH-CE-009／010 销号。
- 同步并行会话 dependabot 合入造成的 action 清单漂移（v1.8.0／SHA 30cde56b…），恢复 `check-actions.py` 绿。
- **环境阻塞**：`cargo test`／`cargo clippy --workspace` 因本机缺 llvm-mingw 链接器失败
  （清华镜像 404、缓存失效、PATH 无 clang）。用户禁止安装 GNU 工具链且不再尝试工具链安装，
  按 ADR-037 登记为阻塞，不以本地 check 冒充测试通过。
