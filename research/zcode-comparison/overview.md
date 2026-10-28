# 总览：ZCode × WHCode 定位与技术栈

> 本文件是 `research/zcode-comparison/` 的总览篇。分域深度报告见 `domain-{A..P}-*.md`。

## 1. 定位与技术栈总览

| 维度 | ZCode | WHCode |
|---|---|---|
| 定位 | AI 编程工作台：桌面/浏览器/终端三界面 + Agent CLI，多智能体协作 | Rust 原生、本地优先 LLM 编码 Harness：事件可追踪、规格驱动门禁 |
| 语言/运行时 | TypeScript，Node ≥24，pnpm workspace | Rust 1.99（零 unsafe 铁律），Cargo workspace 14 crate |
| 桌面 GUI | Electron（main/preload/renderer/host 四进程） | iced + tiny-skia 原生自绘（ADR-026 禁止浏览器套壳，只读 v1） |
| Web | Hono + @hono/node-ws，React 复用 @zcode/ui | 自研单线程 HTTP+WS（无 axum），静态 include_str 嵌入，无构建链 |
| TUI | React 19 + @mbears/opentui | 原生终端绘制，RowBackend 整行输出 |
| 数据库 | SQLite 多文件（tasks-index.sqlite + CLI 会话库 + credentials.json） | DuckDB 单文件，14 表 schema v7，迁移带 FNV-1a 校验和 |
| LLM 执行 | Vercel AI SDK（3 协议） | 自研 WireProtocol（5 协议），同步迭代器流式 |
| 资源策略 | 无可见资源铁律（Electron 多进程） | 资源占用第一优先：RSS/线程/句柄铁律、空闲零重绘、驻留卸载 |

## 2. 独有能力清单

**ZCode 独有**（WHCode 无对应物，详见各域报告）：
1. Agent turn 循环与多轮工具编排（域 A）
2. 内建工具全家桶（Read/Write/Edit/Bash/Glob/Grep/WebFetch/WebSearch/Todo/Cron/AskUserQuestion/Skill/Agent…）+ 输入校验三层 + diff/patch + 后台任务（域 B）
3. 工具输入级权限规则 + 交互 broker（allow/deny/**escalate/modify** 输入改写）（域 C）
4. 会话分叉（原子 fork）+ 对话剪枝（session.revert）+ **工作区文件前像回滚**（域 D）
5. 子代理（进程内子 runtime + 事件镜像 + watchdog/自动后台化）（域 F）
6. 会话内计划模式 + 只读强制（域 G）
7. 技能（SKILL.md + frontmatter + 注入 + SEA 分发；**锁文件无消费者**）（域 H）
8. MCP 客户端（SDK + 三 transport + OAuth + 进程树/Job Object + 连接池）（域 I）
9. Hooks（7 事件 + matcher + trust 审批流）与插件体系（域 J）
10. 流式生成重试（10 次指数退避 + jitter + 空补全重试 + stream-idle 重试）与流空闲超时（域 K）
11. 远程开发三后端（SSH/Docker/WSL）与远端 daemon 部署（域 O）
12. 会话分享（share_code + public_readonly + integrity）、手机远控（域 O）
13. Bot Channel + 闲时任务表（automations/off_peak_tasks）
14. 模型轨迹/白板/Todo UI、Claude 原生会话导入、model-option-map 请求体补丁编译
15. 架构治理的文件级约束（maxFileLines 800 / forbidCycles 自动检查）（域 P）
16. 凭据 AES-256-GCM 静态加密（enc:v1:）（域 N）

**WHCode 独有**（ZCode 无对应物）：
1. 规格驱动门禁（GateId 四类 + StageTransition + force_advance reason 必填审计）（域 G）
2. 四端对等能力登记表（56 条 SurfaceEntry + Support 四态 + 对等契约门禁）（域 C/M）
3. 事件溯源全链路（幂等键、批量写入 4-7k 条/秒、四级持久化、IntegrityFlags）（域 L）
4. 审计表 11 类动作不随清理删除 + AuditSink 失败阻断（域 C/N）
5. 任务状态机检查点同事务原子提交 + recover_uncertain（域 D/L）
6. 资源铁律体系（RSS/线程/句柄/CPU、空闲零重绘、驻留卸载、有界缓冲、每会话不加线程）（域 M）
7. 零 unsafe + clippy 零告警 + 禁止 mem::forget（Rust 特有）
8. 追踪四级脱敏 + TLS 出站信任库（域 K/N）
9. SSH 前端（白名单 + TOFU + 默认拒 Shell）（域 M）
10. CUI JSON 信封 + 稳定退出码 + dry-run 一致性校验（域 M）
11. Web 单线程服务器零新增线程 + 1MiB 上限 + 慢客户端断连（域 M）
12. FR↔任务↔Check 三方追溯 + CI action SHA 铁律（域 P）
13. **Origin/scheme+host 校验**（ZCode 无，域 N）
14. 恒定时间 Bearer 比较（仅 ZCode R3 server-core 有，Legacy server 无）（域 N）

## 3. 待验证项与证据缺口

1. ZCode `skills-lock.json` **无代码消费者确认成立**（两份锁文件内容不同，3 vs 4 个技能；
   均无 `.ts` 读取）——锁定机制可能未接线，采纳前回上游核实（域 H）。
2. ZCode 子代理**无硬并发上限确认**（仅 workflow roster 展示层截断 `maxSubagents=64`）——
   补齐时须自行补硬上限，勿照抄（域 F）。
3. ZCode 各功能结论来自 2026-10-20 commit 快照；上游此后可能已变更，实施前重读对应文件。
4. WHCode 补齐任何一项的验收口径：测试结论以 GitHub Actions 为准（ADR-037），
   本机无远端仓库时登记缺口。
5. 本目录**入库**（research/）；ZCode 源码克隆 `local/zcode-src` 不入库，以路径+commit 指向。
6. 个别域报告标注 `[待验证]` 的点（如 ZCode `prePlanMode` 从未在真实运行态设置、
   `zcode-server.cjs` 与 `server-cli.js` 打包衔接）在实施前须回上游核实。

## 4. 证据层级与交叉验证说明

- ZCode 侧 16 个子代理分域直读源码，每域给出 `文件:行号` 级证据；WHCode 侧由
  `crates/**`、`specs/`、`docs/adr/` 直读交叉佐证。
- 凡“”的否定性结论，均以本仓库 Grep/Glob 实测确认（如域 I 全仓无 MCP 引用、
  域 D 无 session fork/revert 字段），非推测。
- 凡 ZCode 存疑点（无消费者、无硬上限、无 Origin 校验、Legacy 非恒定时间等）均以
  Grep 全库 + 源码直读双重核实，标 `[待验证]` 或给出确证。
