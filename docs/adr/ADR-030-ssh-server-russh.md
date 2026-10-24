# ADR-030：M7 SSH 服务端选型——自研 russh 服务端

- **状态**：批准（2026-10-24，M7 交付批次 T096—T098）
- **来源**：ADR-005 未决条件①（服务端实现选型，TBD-05）；tasks.md T096—T098；FR-607/608/611
- **责任人**：SSH 入口负责人
- **复审触发**：russh 0.64.x 出现不兼容升级或安全通告；SSH 命令速率高到命令队列背压成为
  常态（见 adapter.rs 并发档案失效条件）；恢复任一 BSD 平台验收（连同 ADR-013 修订）；
  用户要求交互式 PTY / 系统 Shell 实际执行（连同 M8 运行时 payload 评审）。

## 1. 决策

SSH 服务端**自研**，基于 `russh 0.64.1` 的 `russh::server`（`Server` + `Handler`）实现；
否决“复用系统 sshd 受限子系统”方案。选型同时结清 ADR-005 未决条件③：客户端
（`proto.rs`）接入 known_hosts 风格信任文件（`<workspace>/ssh/known_hosts`，TOFU），
不再“恒接受”。

## 2. 形式审查（它解决什么真实矛盾）

真实矛盾：M7 要求在 Windows 11 amd64（唯一验收平台）上提供“公钥认证 + 会话隔离 +
受限 WHCode 入口”的 SSH 服务端，而系统 sshd 在该平台没有受限子系统等价物——若复用
sshd，权限判定将脱离 `whcode-app::authz` 的统一拒绝语义，形成第二套权限模型（违反
“拒绝判定只发生在本层”与 C-03）。自研 russh 服务端把认证、白名单、写路径全部收敛进
`whcode-ssh` → `whcode-app` 一条链，矛盾消除。

## 3. 备选方案与否决理由

### 3.1 系统 sshd 受限子系统（否决）

- **否决理由**：
  1. 验收平台 Windows 11 amd64 无 sshd 受限子系统等价物（OpenSSH for Windows 的
     `ForceCommand` 语义受限且随 Windows 版本漂移，无法作为权限边界交付）；
  2. 部署面大：要求目标机安装并配置 sshd 服务、密钥目录与策略文件落在仓库之外，
     WHCode 失去对配置的审计能力（退出权/可复现原则）；
  3. BSD 侧“部署 sshd + force-command”确实免自研，但 ADR-013 已把 BSD 全部延后，
     不能为延后平台引入 Windows 侧的架构妥协；
  4. 受限命令集须经 shell 解释，白名单绕过面（引号、转义、PATH 注入）比进程内判定大。
- **保留**：若未来恢复 FreeBSD/NetBSD 原生验收且实测 russh 服务端在目标平台不可用，
  本 ADR 按复审触发重开。

### 3.2 自研 russh 服务端（选定）

- **正**：认证/会话/权限/写路径进程内闭环，复用 `whcode-app` 与单写者执行器（Check 98
  可用进程内测试证明）；russh 0.64 已是仓库锁定依赖（ADR-011/ADR-005 接受的客户端
  依赖），服务端不新增外部 crate；Ed25519 单算法面收敛（FR-611）。
- **负（承担者）**：协议实现面与审计成本由 WHCode 承担——缓解：russh 承担 SSH 协议栈
  （kex/加密/通道），WHCode 只写 Handler 层；服务端安全默认仅回环监听， exposures 面小。
- **和解理由**：权限一致性与部署可复现性是 FR-608/ADR-005 的硬要求，上述代价可承担。

## 4. 关键实现决策

### 4.1 主机密钥（服务端身份）

- 首次启动生成 **Ed25519** 主机密钥，OpenSSH 格式落盘 `<workspace>/ssh/host_key`
  （临时文件 + rename，避免半写文件）；此后加载同一把。
- 指纹（`SHA256:…`）输出到启动日志（`serve --ssh` stdout）；`doctor` 集成由装配根
  以 `whcode_ssh::hostkey::load_or_create_host_key` 的 `fingerprint()` 取值——本批次
  未改 `doctor` 命令（文件边界限制），登记为待接线项。
- **密钥材料不进日志/事件/审计**：审计与日志只允许 `SHA256:…` 指纹形态。
- 生成用 CSPRNG：`rand 0.10`（ThreadRng，ChaCha12，OS 熵播种）。`rand 0.10` 已是
  russh 的发布依赖（同版本，Cargo.lock 不新增 crate）；替代方案（自造熵源、依赖
  `ssh_key::rand_core` 传递特性统一）均更不可审计，故在 `whcode-ssh` 显式声明。

### 4.2 公钥认证

- 仅 `publickey` 方法（`MethodSet` 只含 `PublicKey`）、仅 Ed25519（`auth_publickey`
  前置算法判定 + `hostkey` 加载时校验）；授权清单
  `<workspace>/ssh/authorized_keys`（缺失即启动失败——fail closed，无隐式授权）。
- 拒绝与接受都写审计（只记指纹）；认证拒绝在 russh 常时拒绝延时之上无额外旁路。

### 4.3 受限入口与 Shell 权限（T097、FR-608、§10.2）

- 白名单：`sessions`、`session show|new`、`tasks`、`task`（映射到 `whcode-app` 命令与
  能力：SessionRead/SessionCreate/TaskRead）；白名单外 → 非 0 退出码（126）+ stderr
  结构化说明 + 审计留痕，不 panic、不静默。
- **任意系统 Shell 为显式独立权限**：`SshPolicy { allow_shell: bool,
  shell_actors: BTreeSet<String> }`，默认全关。能力表达方式的选择（任务书允许二选一）：
  以**配置开关 + 显式授权名单**在 SSH 策略层表达，而不是给 `whcode-app::Capability` 加
  “Shell”变体——后者超出本批次文件边界且会把传输层权限语义注入应用层能力枚举。
  语义等价“专门能力”：默认值可见（全关）、可改、可撤销（移出名单）。
- **显式边界（未实现事项如实登记）**：M7 交付 Shell 的**权限判定与审计**；实际执行属
  M8 运行时 payload（ADR-024）——放行的 Shell 请求返回结构化“未实现”说明（非 0 退出码），
  **不启动任何子进程**。交互式 Shell 通道（`shell_request`）一律协议级拒绝。

### 4.4 单写者执行器与并发档案（T098、ADR-005、C-03、NFR-216/217）

- 结构：装配根打开**唯一一条** `Connection`，所有权移交专用执行线程
  （`whcode-ssh-exec`，`std::thread`，单线程、无每会话线程）；russh Handler（async）
  经**有界命令队列**（`tokio::sync::mpsc`，容量 64，`try_send`）投递
  [`WhCodeCommand`]，以 `oneshot` 一次性回执异步等待——**Handler 只投递请求并等待结果**，
  不触库。队列满 → `Backpressure`（可重试）；等待上限 30 s → `Timeout`。
  （文献依据：tokio `mpsc` 有界通道与背压语义，tokio 官方文档 `sync::mpsc` 一手来源；
  模式先例为 `crates/whcode-store/src/service.rs` WriterService——同一仓库内已验证的
  “有界通道 + 专职线程 + 回执”形态，ADR-017。）
- tokio 运行时：`serve --ssh` 用 **current_thread** 运行时（`serve_ssh_blocking` 内建，
  主线程即事件循环）；线程账本 = 1 事件循环 + 1 执行线程，与会话数无关。依赖特性
  `tokio = ["net", "rt", "sync", "time"]`：`sync` 供有界队列/回执（russh 自身已无条件
  启用同一特性，不引入新代码）；`rt` 仅为 current_thread 运行时——**不引入
  rt-multi-thread**（任务约束），`macros` 留在 dev-dependencies（仅测试）。
- 正：写路径全局唯一（类型层面 Handler/Acceptor 不含 `Connection`；运行层面审计
  `wrote` 计数可核）；无锁竞争（单线程顺序使用连接，ADR-002 同一和解）。
- 负（承担者：吞吐与时延）：全部 SSH 命令串行过执行线程；慢命令占用队列槽位时其他
  会话可能拿到背压拒绝（可重试）——这是“单写者”的直接代价，当前和解。
- 失效条件：背压拒绝成为常态（> 64 命令/秒或慢命令占比高）——届时按 ADR 另行评估，
  不得静默扩队列。
- 回退路径：`WhCodeCommand`/`CommandOutcome` 为纯数据，执行线程内部可整体替换。

### 4.5 客户端主机密钥校验（结清 ADR-005 阻断项③）

- 信任文件：`<workspace>/ssh/known_hosts`，known_hosts 风格，每行
  `[host]:port SHA256:指纹`（端口一律显式，规避 IPv6 冒号二义）。
- TOFU：未记录 → 记录指纹 + stderr 打印供人工核对后放行；已记录且匹配 → 放行；
  不匹配 → 结构化拒绝（连接中止，`ExecReport.status = failed`，note 携带期望指纹与
  行号；**不自动改写信任文件**——改绑必须人工）；文件不可读 → 拒绝（fail closed）。
- `SshTarget.known_hosts_path` 为必填字段：没有信任文件的客户端不提供中间人防护，
  调用方必须显式给出（安全默认，不留“跳过校验”的方便门）。

## 5. 三重时间

- **过去**：ADR-005（SSH 为独立传输入口、复用应用服务、无每会话数据库写路径）与
  ADR-011（平台承诺修订、russh 作为已接受依赖）构成直接来源；C-03/ADR-003 禁止前端
  触库是本设计的硬约束输入。
- **现在**：russh 0.64.1 源码（`examples/echoserver.rs`、`src/server/mod.rs`）为 API
  一手依据；`crates/whcode-store/src/service.rs`（WriterService）为并发模式先例；
  进程内测试（`cargo test -p whcode-ssh --lib`）为当前证据——**四 BSD 平台证据仍未
  取得**（Check 96/97/99，ADR-013 延后，不虚勾）。
- **将来失效条件**：见“复审触发”。另：若 russh 上游变更 `Handler` trait 形态
  （RPITIT → 其他），server.rs 需随版本适配——russh 版本锁定由 Cargo.lock 承担。

## 6. 影响

- 约束 crate：`whcode-ssh`（新增 `server/policy/adapter/hostkey/trust` 模块）；
  `whcode-bin` 装配根新增 `whcode-ssh` 路径依赖（`serve --ssh` 接线所必需，工作区内
  路径依赖，不引入外部 crate）。
- 依赖变更：`whcode-ssh` 的 `tokio` 特性从 `["net","time"]` 扩为 `["net","rt","sync",
  "time"]`（不新增 crate）；新增 `rand = "0.10"`（与 russh 的发布依赖同版本，lockfile
  不变）；`duckdb = "1.10506.1"`（仅 `Connection` 类型透传，同 `whcode-web` 先例；
  dev-dependencies 移除一切 store 依赖以过分层门禁，迁移 SQL 以 `include_str` 复用）。
- Check 对应：Check 98 可由进程内测试证明（单写者）；Check 96/97/99 的平台证据仍缺，
  维持“未取得/延后”登记，不在本 ADR 声称。
