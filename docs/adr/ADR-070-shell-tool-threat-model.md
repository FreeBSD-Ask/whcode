# ADR-070：shell 工具（模型可执行命令）与执行威胁模型

- **状态**：已批准（2026-11-14，proposal-v3 P5.1／D12 批准立项——**先威胁模型后代码**）
- **来源**：`research/synthesis/proposal-v3.md` W5（P5.1）；`research/dsh-comparison/domain-Q-execution-environment.md`
  （DSH 证据根 `local/dsh-src` @ `fcc0ad4`）；ADR-064（四态审批 broker，P1.1 前置已满足）；
  FR-702/703/704/706/757；`whcode-runtime` 既有受控执行骨干
- **责任人**：工具系统维护者、安全与可观察性维护者、装配根维护者
- **退出条件**：见"已知限制与失效条件"节；P5.2 沙箱落地时本模型的残余风险清单逐条复审

## 背景

模型当前不能执行命令：9 个内置工具全部是文件/状态类（`whcode-tools::builtins::default_registry`）。
DSH 域 Q 判定"执行环境是产品主线"（DSH 领先项）；WHCode 的差异化优势是"可选、默认关闭、
能力分级"（FR-702/704），这一优势**保持不变**。既有基建已覆盖执行治理的主干：
`worker::execute_with_cancel`（并发闸门＋最小环境＋凭据剥离＋进程组＋窗口抑制＋尾部截断＋
超时杀树＋取消区分）、`GitBashProvider`（payload 校验＋FR-706 检查点四要素＋专用闸门）、
审批 broker 的 `shell` → Ask 种子（`approval.rs::with_builtin_destructive_ask`）。

本 ADR 先立威胁模型，再定工具形态——顺序是 D12 的批准前提，不是文风偏好。

## 威胁模型（先于代码）

**资产**：用户工作区文件、父进程环境凭据、宿主资源（CPU/内存/磁盘）、审批决策的完整性、
审计链的可信度。

**攻击者**：①被诱导的模型——提示注入可经检索内容、用户粘贴、MCP 服务器输出进入上下文，
诱导模型生成恶意命令（**模型诱导执行**，proposal 指定的必答项）；②本地恶意环境——PATH
劫持、冒充 Git Bash 的恶意 payload、凭据文件。

**信任边界**：审批界面是人机边界；WHCode v1 **不做 OS 沙箱**（P5.2 未实施）；网络不受限；
工作区 cwd 是锚点不是监狱。

| # | 威胁 | 对策（既有/新增） | 残余风险（显式登记） |
|---|---|---|---|
| T1 | 模型诱导执行破坏性命令 | per-tool `Ask` 种子（人看到**完整命令文本**再决定）；四态审批（ADR-064）；每次调用落 `ToolCallStarted/Finished` 事件 | 用户审批疲劳放行（T2） |
| T2 | 审批疲劳／持久允许被滥用 | 持久允许是显式落库规则（`approval_rules` 表，可查可删）；**白名单只约束"免审批通道"**（决策 4）：复合脚本永不免审批 | 用户持久允许后模型可重复执行同类命令——属用户显式决定 |
| T3 | 凭据经环境外泄 | `apply_minimal_env`（最小继承集）＋`strip_credentials`（按名剥离、只记名不记值） | 脚本可读磁盘上的凭据文件（如 `~/.ssh`）——无文件沙箱，登记至 P5.2 |
| T4 | PATH 劫持（恶意同名可执行） | bash 经 `locate_bash` 显式候选定位（`git_path` 覆盖优先），非裸 PATH 查找；payload 来源/哈希/平台校验（FR-703）＋`--version` 探测确认 | 脚本内部的二级命令按（已收敛的）PATH 解析——残余，登记至 P5.2 |
| T5 | 资源耗尽（fork 炸弹／死循环／巨输出） | 超时（默认 30 s、上限 600 s 常量）＋超时终止进程树＋全局与 gitbash 专用并发闸门＋输出尾部截断＋请求体积上限 | 超时窗口内的资源消耗；磁盘无限写（无配额） |
| T6 | 工作区逃逸写 | cwd 固定工作区根（`execute_in`，`workdir_policy = "workspace"`）；shell 副作用**不进前像恢复**（ADR-068 决策 7 口径：外部进程副作用不回滚），审批卡与工具描述明示 | 脚本可用绝对路径写任意位置——无沙箱，登记至 P5.2 |
| T7 | 命令拼接注入 | 脚本文本经 argv 直传（`bash -c <script>`），不经二次 shell 拼接；`WHCODE_*` 环境命名空间不继承（域 Q 借鉴 1） | — |
| T8 | 控制台闪窗／进程残留 | `configure_child_window`（ADR-069）＋进程组／`taskkill /T`（零残留门禁 Check 109/201） | — |
| T9 | 审计链失真／凭据入事件 | 命令、退出码、截断标记全量入事件（可复原"执行过什么"）；tracing 默认 `redacted`；worker 剥离凭据＋CodeQL 敏感词纪律（断言/日志静态文案） | 事件中的脚本文本由模型生成，可能含用户粘贴的敏感内容——用户责任边界，文档明示 |
| T10 | 冒充 bash 的恶意二进制 | `--version` 探测＋payload 哈希校验＋环境指纹入检查点 | — |

**结论**：在"无 OS 沙箱"前提下，审批＋资源界＋argv 直传＋留痕构成 v1 的可辩护最小防线；
全部残余风险指向同一收敛点——P5.2 沙箱评估（Windows ACL 试点）。威胁模型不充分即停
（proposal 风险 top3 ①），本节即"停"的判据承载。

## 决策

1. **工具形态**：`shell` 工具（`whcode-tools`），参数 `script`（必填，bash 脚本文本）与
   `timeout_ms`（可选，默认 [`DEFAULT_TOOL_TIMEOUT_MS`]，钳制 [`MAX_TOOL_TIMEOUT_MS`]）；
   输出＝退出码＋stdout 尾部＋stderr 尾部＋`timed_out`/`truncated` 显式标记（截断不静默，
   R-05）。`SideEffectScope` 新增 **`ProcessSpawn`**（穷尽匹配同步 `rule_for` 与 catalogue
   渲染；`rule_for` 归 `default_workspace_write` 口径，交互装配的 per-tool `Ask` 种子已是
   事实标准）。
2. **执行骨干＝注入，不加 crate 依赖**：`whcode-tools` 定义 `ShellExecutor` trait（`ShellRequest`
   ＋`ShellOutcome`）；`whcode-bin` 装配根提供实现——扩展 `GitBashProvider::execute_in(&Script,
   workdir)`（新增 `workdir_policy = "workspace"`；既有 `"temp"` 策略与 `execute()` 口径**不动**），
   复用 FR-706 检查点四要素与既有闸门。`whcode-tools` 不依赖 `whcode-runtime`（分层不变）。
3. **默认关闭（FR-704）**：默认构建不注册 `shell` 工具。启用＝**显式 opt-in 配置文件**
   `<workspace>/.zcode/shell.json`（与 `.zcode/mcp.json` 同模式：文件存在才启用，无自动发现）
   ＋ bash 可用探测，双条件缺一不注册。能力等级沿用 `ShellOnly`，不得表述为完整 shell 生态
   （D-08）。
4. **白名单＝免审批通道的约束面，不是执行前置**（威胁模型 T1/T2 的直接产物）：bash 图灵完备，
   任意脚本的"命令白名单"**不可判定**——任何首词匹配都能被 `;`／`&&`／`$()`／子壳绕过。
   因此：配置 `allowed_commands`（首词模式列表，默认空）；**仅当**脚本经分词后是"单命令"
   （不含 `;`、`|`、`&`、`$`、反引号、换行等控制结构）且首词命中白名单时，才可被"持久允许"
   规则免审批；其余一切脚本**每次都必须 Ask**。白名单不拒绝任何请求（放行通道始终是审批），
   也不提供任何"阻止执行"的承诺——该承诺属于 P5.2 沙箱。
5. **事件与审计**：`ToolCallStarted.args` 承载完整 `script`（受既有事件上限约束）；
   `ToolCallFinished.output_value` 承载摘要（退出码＋尾部长度＋标记）；FR-706 检查点四要素
   随事件留痕（脚本哈希可复核"执行过什么"）。
6. **超时与取消**：deadline 桥接沿 T267/FR-757 口径——实际超时＝descriptor 超时与 deadline
   余量的较小者；`timed_out` 映射 `ErrorCode::ToolTimeout`（"停止等待并标记"可判定）。用户
   取消（CancelToken）**不打断**运行中的子进程（worker 的 cancel 机制已备，turn 侧接线属
   P5.4/jobs 批次）——登记为非目标。
7. **非目标**：OS 沙箱（P5.2）、PTY/终端（P5.4）、后台作业（P5.5）、复合脚本免审批、
   前像恢复覆盖 shell 副作用（ADR-068 决策 7 明文排除）。

## 形式审查（宪法原则 I）

解决的真实矛盾：模型无法构建/测试/运行 git 等开发命令——这是编码 harness 的核心能力缺口
（DSH 域 Q 判定"产品主线"），不是功能囤积。威胁模型先行的原因是该能力把"模型输出"变成
"任意进程执行"，风险等级与其余工具不同轴。

## 正负和解（宪法原则 II）

- **正面**：核心执行能力补齐；三层治理（审批/白名单约束免审批/资源界）全部复用已验证基建；
  默认关闭保住资源铁律与 FR-704 差异化优势。
- **负面**：高危面扩大（残余风险清单 10 项）；启用需显式配置（体验成本）；新增 trait 注入与
  provider 扩展的实现复杂度；复合脚本永不免审批（能力让步）。
- **承担者**：残余风险由启用该工具的用户知情承担（审批卡明示"shell 副作用不回滚"）；
  实现成本由本批次承担。
- **和解理由**：能力缺口真实且治理基建已备；负面均有界、显式、可复审。

## 已知限制与失效条件

- **已知限制**：无 OS 沙箱（文件/网络不受限）；脚本内二级命令按收敛 PATH 解析；
  用户取消不打断运行中子进程；复合脚本永不免审批；Windows 上依赖 Git Bash 存在。
- **失效条件与复审触发（绑定命令＋判定规则）**：
  - `python .github/scripts/check-code-scanning.py` 出现 shell 工具相关 open 告警 →
    **停止交付**，修复后重推（涉安全条目脱敏审查，proposal 执行序 4）。
  - 任何一次 `cargo test -p whcode-tools` 白名单/审批用例红 → 复审决策 4 的匹配算法。
  - P5.2 沙箱（Windows ACL）落地时 → 逐条复审本表残余风险（T3/T4/T5/T6），能收敛的移出清单。
  - `cargo build` 默认特性下 `default_registry()` 出现 `shell` → FR-704 违规，立即修
    （门禁：`default_registry_registers_nine_tools_without_conflict` 保持九工具）。

## 0 信任校验

- DSH 行为以 `local/dsh-src`（`fcc0ad4`）＋域 Q 记录的文件行号为据，不凭比较书结论转述。
- worker 治理行为（凭据剥离/最小环境/超时/进程组）以既有测试与 Check 109/201 为准，本 ADR
  不复述其实现。
- 白名单不可判定性论证基于 bash 语义本身（控制结构可绕过首词匹配），属可复验的逻辑事实；
  决策 4 的范围因此收窄到"免审批通道"，并在工具描述与审批卡如实表述。
