# 域 H：技能与运行时自改

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 技能来源 | Provider 注册表：本地文件、打包、远程皆可实现 `SkillProvider`；host 层与 per-scope 层合并，最近层重名全胜（`docs/subsystems/skills.md`） | 只扫描工作区 `.zcode/skills/<name>/SKILL.md`，不递归（`crates/whcode-app/src/skills.rs:23`—`34`） | **DSH 领先** |
| 发现协议 | `list()` 异步发现＋`complete` 标志（不完整观察可贡献候选但不缓存）；无效候选 fail fast（`docs/subsystems/skills.md`） | 同步扫描目录，frontmatter 解析失败即产生告警（`skills.rs:31`—`66`） | **DSH 领先** |
| 注入 | `skill` 工具＋目录目录常驻；正文按需 `get()` 加载（`docs/subsystems/skills.md`） | 技能描述以 system 消息注入，截 200 字符/技能、上限 10 技能（`crates/whcode-app/src/turn.rs:379`—`393`） | **同构**：目录＋按需读 |
| 治理 | 无锁文件消费问题（ZCode 的教训不在 DSH 复现）；候选校验失败即拒（`docs/subsystems/skills.md`） | 可达性门禁：SKILL.md 引用的相对路径文件必须存在（`skills.rs:95`—`97`，ADR-053 §2.2） | **WHCode 领先**：相对路径完整性在加载期校验 |
| 运行时自改 | `tool-cordis` 两个**只读**检视工具＋`cordis-host-runner`：agent 可定义带版本的 Cordis 包并在宿主/浏览器两端运行（`packages/extensions/README.md`） | 无（检索 `mount`/`plugin`/`dlopen` 于 `crates/` 无实现） | **DSH 独有** |
| 自改权限 | 检视只读；持久插件安装走 Plugin Manager（人操作）（`extensions/README.md`） | 不适用 | **DSH 领先**：边界明确 |
| preset | `preset/cordis.yml` 可给单会话不同能力组合，服务行可标 `isolate` realm（`docs/architecture.md`） | 无会话级组合；crate 集合编译期固定 | **DSH 领先** |

## 2. DSH 实现解剖

### 2.1 技能是三层角色分离

Service Definition（`ctx.skills`）、Provider（本地/打包/远程）、Consumer（`skill` 工具）分离
（`docs/subsystems/skills.md`）。注册同步，远程发现放在被 await 的 `list()` 里；provider 名称**按层唯一**而
非进程唯一，agent preset 挂的插件进 preset 层（`docs/subsystems/skills.md`）。

### 2.2 发现可以“不完整”

`SkillProviderObservation{candidates, complete}` 允许 provider 报告“我有可用候选但发现没做完”；
这类结果可用但**不可缓存**（`docs/subsystems/skills.md`）。发现期 provider 代际变化会重试一次，
再变则返回最新候选且不缓存（`docs/subsystems/skills.md`）。`skills/change` 失效事件不带 diff，
消费者自己重拉快照（`docs/subsystems/skills.md`）。

### 2.3 extensions：只读检视＋沙箱化运行

`tool-cordis` 只暴露两个只读检视工具；真正的动态插件由 `cordis-host-runner` 在**沙箱化的
宿主半区**运行，浏览器半区由 `cordis-client-runner` 求值（`packages/extensions/README.md`—
`15`）。即“模型能看、能跑临时代码，但持久安装必须经 Plugin Manager（人）”。

## 3. WHCode 现状核对

- 已实现：技能扫描（`scan_skills`，`skills.rs:31`）、frontmatter 解析（name+description，
  描述截 200 字符，`:66`—`92`）、相对路径可达性校验（`:95`—`97`）、turn 注入（系统消息，
  每技能 200 字符、上限 10 个，`turn.rs:379`—`393`）。
- 已实现：可达性失败即测试失败（ADR-053 §2.2，BH 治理为骨）。
- 缺失：`skill` 工具（模型不能主动调技能，只能靠目录）、远程/打包 provider、技能变更失效事件、
  运行时检视与动态插件、会话级 preset。检索 `SkillProvider`、`tool_skill`、`preset` 于 `crates/`
  无实现命中。
- 差异：WHCode 的技能**没有独立工具入口**，目录注入后由模型用 `read_file` 自取正文；
  DSH 有 `skill` 工具封装“列表＋加载”。

## 4. 差异判定与可借鉴点

1. **`complete` 语义**：可采纳（作为概念）。WHCode 扫描是同步全量的，不存在“发现没做完”；
   但将来若支持远程目录，必须区分“完整快照”与“部分可用”，且只缓存前者。
2. **`skill` 工具入口**：可采纳。当前“目录注入＋模型自己 read_file”多占一轮工具调用且无
   失败反馈；一个 `skill` 工具（name → 正文）更可控。代价：多一个工具与登记面。
3. **按层重名解析**：不采纳。WHCode 无 per-scope 注册概念，全局唯一名（`BTreeMap` 重名拒绝）
   更简单可证；若引入多来源，建议“来源前缀”而非分层覆盖。
4. **extensions 动态插件**：不采纳。与静态分层、零 unsafe、无运行期加载的硬边界冲突；
   DSH 自己也把“模型安装持久插件”排除在外（走 Plugin Manager 人审），证明这条边界是共识。
5. **preset 会话级组合**：不采纳（当前）。WHCode 的组合面是编译期 crate 与 feature；若将来
   需要会话级裁剪（如只读会话），应先立 ADR 定义“组合不能新增能力，只能裁剪”。

## 5. 待验证项

1. DSH `skill` 工具的失败语义（技能加载失败返回什么）：只读了 registry 层。取证：
   读 `packages/skill/tool-skill/src/index.ts`。
2. WHCode 技能目录注入是否含正文截断告警（超过 200 字符是否提示模型）：`turn.rs:379` 只见截断，
   未见提示。取证：读注入代码段。
3. DSH 技能是否可携带可执行脚本并被授权执行：`docs/subsystems/skills.md` 未提执行；需确认技能只是“指令＋附件”。
   取证：读 `packages/skill/skill-filesystem/src/index.ts` 与 `SkillDefinition` 字段。

## 6. 证据路径清单

- `docs/subsystems/skills.md`（三角色分离）
- `docs/subsystems/skills.md`（host/scope 分层）
- `docs/subsystems/skills.md`（不完整观察）
- `packages/extensions/README.md`（extensions 组定位）
- `packages/extensions/README.md`（只读检视＋沙箱 runner）
- `crates/whcode-app/src/skills.rs:31`（扫描）
- `crates/whcode-app/src/skills.rs:95`（可达性门禁）
- `crates/whcode-app/src/turn.rs:379`（注入）
