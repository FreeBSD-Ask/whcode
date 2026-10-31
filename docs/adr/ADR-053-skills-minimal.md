# ADR-053：技能最小集（P1-6 v1——SKILL.md 格式、加载器与 turn 注入）

- **状态**：已批准（2026-10-31，建议书 proposal.md D3 排序 + 用户"继续"指令）
- **来源**：`research/zcode-comparison/` 域 H（ZCode 技能系统证据：SKILL.md 格式/加载/
  注入；**skills-lock 无消费者**为反例）+ `research/better-harness/`（BH 治理：SKILL.md
  =路由器 + 一跳 references + doc-link-graph 可达性测试 + 创建门槛）+
  `research/synthesis/README.md` §3 交叉发现 3（"BH 治理为骨、ZCode 格式为皮"）
- **责任人**：whcode-app 维护者
- **退出条件**：技能系统连续两个批次零使用（模型从未调用匹配技能）或维护成本
  显著超出价值（须另立 ADR 裁剪）

## 1. 事实与证据

1. 本仓无技能系统（Grep 全仓无 `.zcode/skills` 引用）——agent 上下文注入完全依赖
   AGENTS.md 静态规则，无可复用程序性知识的结构化承载。
2. ZCode 域 H：SKILL.md 格式成熟（frontmatter name/description + 目录发现 +
   `${ZCODE_SKILL_DIR}` 替换）；但 **skills-lock.json 无代码消费者**、两份锁文件内容
   不一致——"有格式无治理"的反面教材。
3. BH 域 H/S：SKILL.md = 路由器（frontmatter 触发 + 短 body + 一跳 references）；
   **doc-link-graph 测试强制每个 reference 从 SKILL.md 可达**；skill-review 执行契约
   审计；Loop Discovery 覆盖阶梯"先试已有再创建"——治理完备。
4. 本仓 turn 循环的 `LlmRequest.messages` 已支持 system/user/assistant/tool 四角色
   （FR-720），技能注入的技术底座已备。

## 2. 决策

1. **格式（ZCode 皮）**：`.zcode/skills/<name>/SKILL.md`，frontmatter `name` +
   `description`（YAML），body 为 Markdown 正文；不递归扫描（与 ZCode 一致）。
2. **治理（BH 骨）**：
   - SKILL.md 引用的所有相对路径文件必须存在且从 SKILL.md 可达——`test_layout.rs`
     或独立 `test_skills_reachability.rs` 门禁强制；
   - 无锁文件（ZCode skills-lock 无消费者的教训——不做无消费者的机制）；
   - 技能创建走 Loop Discovery 覆盖阶梯（先证既有覆盖不足，再新增）。
3. **加载器**：`whcode-app/src/skills.rs` `SkillRegistry::scan(workspace)`——扫描
   `.zcode/skills/*/SKILL.md`，解析 frontmatter（name + description），返回结构化
   列表；坏文件跳过并记 warning（不 panic，不静默）。
4. **注入**：turn 循环的 `LlmRequest.messages` 在 user 消息之后追加一条
   `role:system` 消息，内容为匹配技能的 `name + description` 清单（技能发现层，
   不注入技能正文——正文由模型按需 `read_file` 读取，防止上下文膨胀）。
5. **显式排除**（v1）：技能正文注入（上下文膨胀风险）、技能参数化调用
   （`${ZCODE_SKILL_DIR}` 替换）、技能热重载、用户级/全局技能目录——均延后。

## 3. 正负和解

- **正面**：模型可发现程序性知识（技能描述注入），按需读取正文；治理机制防蔓延；
  零依赖、零线程。
- **负面**：技能描述注入增加 system 消息长度（有界：描述截 200 字符 × 技能数 ≤10
  = ≤2 KiB）；技能正文需模型主动 read_file（多一步，但保留上下文预算）。
- **承担者**：whcode-app 维护者（扫描器与注入）、技能作者（SKILL.md 质量）。
- **和解理由**：v1 最小闭环——发现 + 按需读取，不做正文注入（膨胀风险）。

## 4. 三重时间

- **过去来源**：ZCode 域 H / BH 域 H/S 一手源码；synthesis 交叉发现 3。
- **现在证据**：本仓无技能系统（Grep 实测否定性结论）。
- **将来失效条件**：见头部退出条件。

## 5. 复审触发

- `cargo test -p whcode-app skills` 随批提交；
- 真实会话统计：技能发现事件占比——连续两个批次零匹配 → 触发裁剪评估。

## 6. 实施摘要

- FR-735、T232—T235、Check 328—331 登记（计数 FR 112→113、任务 231→235、
  Check 327→331）；实现落 `whcode-app/src/skills.rs`。

## 7. 修订记录

- **2026-10-31（首版，已批准）**：编号 053（052 已被子代理占用）。
