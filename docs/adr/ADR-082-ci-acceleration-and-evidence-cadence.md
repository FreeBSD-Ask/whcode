# ADR-082：CI 加速与证据节奏（P7.4）

- **状态**：已批准（2026-11-16；用户请求“CI 有没有加速方案？目前太慢了”并经
  “A+B+C+E+F 全上”批准；ADR-013、ADR-037 同日追加修订记录）
- **来源**：2026-11-16 实测（fa04830 运行 `43876481237` 各 job 墙钟与测试套件耗时）；
  ADR-013（FreeBSD 硬门禁、BSD 证据身份）、ADR-037（CI 是唯一验收入口）、
  ADR-025（测量口径）、ADR-033（长跑通道）、ADR-075（runner 类别与基线）
- **责任人**：CI 维护者、发布维护者
- **退出条件**：见 §8（绑定命令与判定规则）

## 1. 问题与边界

一次 push 的全部平台 job 实测墙钟（run `43876481237`，main@fa04830）：

| job | 墙钟 | 构成 |
|---|---|---|
| platform-freebsd | 39.8 min（本轮红于 A2） | QEMU 起机、`pkg update`＋X11 栈、rustup 从零、DuckDB 2.0.0 源码构建、全量构建与测试 |
| measurements | 34.2 min | 构建 4.3；NFR-201/202 单步 20.6；其余 9.2 |
| platform-windows-arm64 | 26.2 min | 又跑一遍全量测试 |
| platform-windows | 21.9 min | 测试步 14.2 |
| platform-netbsd | >40 min | 与 FreeBSD 同构（VM 内全量） |

三个可归因的浪费：

1. **没有并发取消**：同一天三次推送时，前两次的整套矩阵都跑完了才被新提交取代。
2. **没有跨运行缓存**：每个 Windows job 每轮重下 llvm-mingw（191 MB）与 DuckDB 载荷，
   `target/` 全冷编译；`setup-windows-llvm.py` 与 `fetch-duckdb.py` 的缓存只活在临时
   runner 里。
3. **工作量本身可搬**：Windows 测试步 14.2 分钟＝编译 3.1＋运行 11.3，运行时间的
   8.78 分钟（78%）来自 `extreme_scale` 一个套件；纯文档推送仍付整批 40 分钟；NetBSD
   只作平台证据却每次 push 都跑。

边界（非目标）：

- 不删测试、不用 `#[ignore]` 藏起、不缩小 1M 事件夹具；
- 不放宽任何验收判据（唯一验收目标仍是 Windows 11 amd64／`x86_64-pc-windows-gnullvm`，
  FreeBSD 仍是硬门禁）；
- 不做测试分片成多 job（16 个 crate 每 job 都要全量编译，净亏）；
- 不引入 sccache 等新设施（16 crate 规模收益不足以承担一层依赖）。

## 2. 决策

1. **并发取消（A）**：`platform-verification.yml` 顶层
   `concurrency.group = <workflow>-<ref>-<event_name>`，
   `cancel-in-progress` 只对 `push` 与 `pull_request` 为真。group 带事件名是为了让
   schedule 的周任务不会取消正在进行的 push 验证；分支维度保证不同 ref 互不干扰。
   **被取消的运行不是绿**，不得引用为证据（ADR-037 修订）。
2. **有界缓存（B）**：三处缓存，键必须收敛且绑定输入，
   - `${{ runner.temp }}/whcode-toolcache` ← `llvm-mingw-20261104-ucrt-x86_64`；
   - `vendor/duckdb/.cache` ← `duckdb-2.0.0-payload-<通道>`；
   - `~/.cargo/registry`、`~/.cargo/git`、`target` ← `cargo-<通道>-<hash(Cargo.lock,
     rust-toolchain.toml)>`，`restore-keys` 只复用同通道旧键。
   缓存**只省时间**：工具链与载荷的 SHA256 与字节数校验仍由脚本逐次执行，校验失败照旧
   中止。`actions/cache` 取 v6.1.0（固定 SHA，入 `action-versions.json`）。
   **首轮实测后撤回 `target`**（2026-11-16，见 §4）：Cargo 缓存含 `target` 时体积
   2.70 GB（amd64）／2.30 GB（arm64），触犯 §5 的「单键 >2 GB 即撤回」；现只缓存
   `~/.cargo/registry` 与 `~/.cargo/git`。将来做有界 target 缓存须先在日志里取到
   `target/debug/{deps,build,incremental,.fingerprint}` 的体积分布（工作流内已有诊断步）。
3. **`extreme_scale` 周期专项（C）**：windows、windows-arm64、FreeBSD 三条通道的全量
   测试一致改为 `cargo test … -- --skip extreme_scale`，新增 `extreme-scale` job 在
   schedule 与 `workflow_dispatch(extreme=true)` 运行 `windows-latest` 与
   `windows-11-arm` 两个矩阵项，证据按平台上传 artifact。理由：该套件断言的是与平台
   无关的 O(1) 内存与翻页不变量（X2—X5），不随每次推送的代码差异改变。
4. **文档推送分流（E）**：新增 `triage` job 以 `git diff --name-only HEAD~1 HEAD` 判定，
   白名单仅 `docs/`、`specs/`、`artifacts/`、`i18n/`、根级 `*.md`；白名单之外（含
   `.github/`、`crates/`、`tests/`、`Cargo.*`）一律按代码处理；无父提交或 triage 失败
   亦按代码处理（宁多跑，不漏证）。纯文档推送由 `docs-gates` 接手，跑 EOL、文风、
   知难行易、ADR 限制、标准基线、计数、追溯、i18n PO、工作流语义、action 版本共十项。
   `platform` 与 `measurements` 用 `needs: triage`＋`always()`：pull_request 上 triage
   被跳过时矩阵仍运行（`needs` 的默认语义会把跳过当不满足，必须显式 always()）。
5. **NetBSD 周期专项（F）**：NetBSD 步骤从平台矩阵抽出为独立 job，仅在 schedule 与
   `workflow_dispatch(netbsd=true)` 运行，保留同名公开注解（判据行与进度阶段以 notice
   公开）。平台矩阵自此为 windows／windows-arm64／freebsd／openbsd／dragonfly 五项。
   NetBSD 仍只作平台证据、不阻断 CI（ADR-013 修订）。
6. **BSD 阶段计时（D）**：FreeBSD 与 NetBSD 的 `*-progress.txt` 每行附
   `at=<UTC>`，使 VM 内阶段耗时在失败时也可拆解；只增信息，不改判据行匹配。

## 3. 正负和解

- **正面**：被取代的推送不再跑完整矩阵；重复下载与冷编译不再重复；最贵的单套件按周期
  覆盖；文档提交以分钟级门禁代替四十分钟矩阵；VM 阶段开始可测量。
- **负面**：（i）缓存带来“陈旧产物”风险，可能掩盖增量构建问题；（ii）`extreme_scale`
  的证据从“每次推送”降为“每周”，回归发现延迟最长 7 天；（iii）纯文档推送不再产生该
  提交的五平台证据，验收只引用最近一次代码提交的运行；（iv）NetBSD 证据周期同样降为
  每周；（v）`triage` 的路径白名单一旦漏掉应当算代码的目录，会把代码改动当文档。
- **承担者**：CI 维护者（键与白名单的维护）、发布维护者（引用哪一次运行作证据）、
  各通道维护者（缓存异常时的排障）。
- **和解理由**：（i）缓存键绑定 `Cargo.lock`／工具链／固定版本，SHA256 与字节数校验
  逐次照跑，且 §5 给出“省时不足即撤回”的量化判据——不达标就撤，不做长期赌注；
  （ii）（iii）（iv）的延迟只影响发现时间，不影响判据本身：失败仍红，且 1M 事件不变量
  与文档改动无语义耦合；（v）白名单取保守方向——误判为代码只是多花时间，误判为文档
  才会漏证，故白名单只列明确无语义耦合的目录。

## 4. 三重时间

- **过去**：ADR-037（2026-10-25）把 CI 定为唯一验收入口后，任何加速都必须先回答“是否
  改变判据”；ADR-013 修订（2026-10-31）把 FreeBSD 定为硬门禁，本轮只改其测试集合的
  一项与 NetBSD 的运行时机；`proto.rs` 的既有注释（“预算取 3 s：过小的预算会在平台上
  把慢误判为超时”）是本轮“不要把机器速度当判据”的先例。
- **现在**：上表的墙钟与套件耗时是实测值。**改后首轮**（run `43894584718`，
  main@`b803dde`，全绿）与改前同口径对比：

  | job | 改前 | 改后 | 变化 |
  |---|---|---|---|
  | platform-freebsd | 39.8（红） | 28.2（绿） | −11.6 |
  | platform-windows-arm64 | 26.2 | 20.0 | −6.2 |
  | platform-windows | 21.9 | 25.1 | +3.2（首轮缓存全冷：fmt/clippy 3.0 分钟 vs 1.5，交叉检查 3.0 vs 1.9） |
  | measurements | 34.2 | 35.6 | +1.4（同上） |

  **第二轮（run `43902805603`，`14c2f3b`，缓存命中）**：windows 16.6、arm64 14.8、
  freebsd 25.6、measurements 27.8，全部 success；Windows 测试步 6.80 分钟（改前 14.2，
  首轮 8.38），ARM64 测试步 9.07、FreeBSD 25.6（改前 39.8）。相对改前：windows −5.3、
  ARM64 −11.4、FreeBSD −14.2、measurements −6.4 分钟。

  关键路径由 FreeBSD（39.8）转到 measurements（35.6）。FreeBSD 的 24.8 分钟 VM 阶段现已
  可按时间戳拆解（`freebsd-progress.txt`）：DuckDB 2.0.0 源码构建 16.4 分钟是最大一块，
  workspace 构建 1.6、workspace 测试 2.4、SSH 回环 1.7。缓存体积实测：llvm-mingw
  344 MB×2、DuckDB 载荷 38+18 MB、`target` 2.70+2.30 GB、vmactions VM 镜像另占约 3.5 GB。
  **自我纠正**：此前以为“四个 BSD job 每轮从零装 rustup”，实际 vmactions 自带 VM 镜像缓存
  （`freebsd-15.1-*`／`netbsd-11.*`／`openbsd-8.0-*`），`toolchain-ok` 实测 9 秒；BSD 侧
  真正的固定成本是 `pkg install` 与 DuckDB 源码构建，不是工具链下载。
- **将来**：仓库缓存总量逼近配额、runner 缓存策略变化、或 `extreme_scale` 时长被其他
  结构（例如增量夹具）改变时，本 ADR 失效（§8）。

## 5. 可判定判据与撤回条件

| 判据 | 绑定命令 | 不满足时 |
|---|---|---|
| 并发组含 workflow／ref／event，且仅 push／PR 取消 | `python .github/scripts/check-workflows.py` | 红灯，修工作流 |
| 缓存键绑定版本／`Cargo.lock`／工具链 | `python .github/scripts/check-actions.py`＋人工核对键串 | 红灯，修键 |
| 缓存确实省时 | 首轮运行日志：命中记录与测试步时长 | **省时 <2 分钟或单键 >2 GB ⇒ 移除该缓存并登记**（2026-11-16 已按此撤回 `target`：2.70 GB／2.30 GB） |
| push 路径不再跑 `extreme_scale` | `grep -n "skip extreme_scale" .github/workflows/platform-verification.yml`（三处） | 红灯 |
| 周期专项存在且两平台 | `grep -n "extreme-scale" .github/workflows/platform-verification.yml` | 红灯 |
| 文档推送只跑 docs-gates | 首个纯文档推送的 run 页面 | 记录为缺口并修 triage |
| NetBSD 不在矩阵内 | 工作流 matrix 无 `netbsd` 项 | 红灯 |

`target` 缓存的撤回是**硬条件**：它不是“最好有”，而是“要么省时 ≥2 分钟且单键 ≤2 GB，
要么删掉”。

## 6. 标准与一手文献研读（2026-11-16 取读）

- GitHub Docs，《Usage limits, billing, and administration》→ “Cache storage (per
  repository)” 为 **10 GB**（Free／Pro／Team／Enterprise Cloud 同值）；超量需提额并计费。
  来源：`docs.github.com/en/actions/reference/limits`（2026-11-16 取读）。
- GitHub Docs，《Caching dependencies to speed up workflows》→ 缓存条目“超过 7 天未访问
  即被移除”；分支可见性为“当前分支或默认分支创建的缓存可被恢复”，PR 运行还可恢复
  基分支缓存，**不能**恢复子分支或兄弟分支的缓存。单键大小上限该页未载明，标
  `[待验证]`，因此本 ADR 用“单键 ≤2 GB”的**自设**撤回阈值，不冒充平台限制。
  来源：`docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows`
  （2026-11-16 取读）。
- 《ISO/IEC/IEEE 12207:2017》7.2.2（配置管理的构建与连续集成语境）：构建与测试的
  自动化产物应可追溯、可重现——本 ADR 以“缓存不参与判定、SHA256 逐次校验”满足
  可重现性，不改变产物来源。
- 既有决策：ADR-033（长跑通道的窗口与分辨力纪律）、ADR-075（runner 类别差异）——
  本轮不把托管 runner 的耗时差异升级为性能结论。

## 7. 已知限制

1. **`target` 不进缓存（已撤回）**：首轮实测单键 2.70 GB／2.30 GB，且仓库缓存总量当时
   已达 9.3 GB（vmactions VM 镜像 3.5 GB、其余旧键若干），接近 10 GB 上限——继续留会
   挤掉更值钱的镜像缓存。代价是冷编译仍在每次运行发生；补偿手段是工作流内的体积诊断步，
   为将来按子目录（`deps`／`build`／`.fingerprint`）划定有界范围提供数据。
2. **缓存可见性受分支规则限制**：PR 运行只能恢复基分支缓存，不能恢复兄弟分支——同一
   仓库多分支并行时命中率会下降（平台行为，不是配置错误）。
3. **文档推送不产出该提交的平台证据**：验收引用最近一次代码提交的运行；若某次纯文档
   推送同时改变了会被测试读取的数据（例如 `tests/fixtures` 下的内容），白名单会把
   `tests/` 判为代码，不会走入此限制。
4. **`extreme_scale` 回归发现延迟最长 7 天**：1M 事件的 O(1) 不变量若在某次推送中被破坏，
   最早在下一次 schedule 或手动 dispatch 暴露。
5. **NetBSD 证据周期为每周**：跨平台差异的发现延迟同样最长 7 天。
6. **`triage` 依赖 `HEAD~1`**：force-push 或重写历史后单次判定可能失真，此时按代码处理
   （保守方向），但会让该次推送多跑一轮矩阵。
7. **省时数字尚未取得**：本文所有“预计节省”均为待验证；首轮运行后按 §5 回填。

## 8. 失效条件与复审触发

- **绑定命令**：`python .github/scripts/check-actions.py --online` 报告缓存 action 不再是
  最新 release ⇒ 复审本 ADR 的 action 版本与键格式；
- **绑定命令**：`grep -c "skip extreme_scale" .github/workflows/platform-verification.yml`
  不等于 3 ⇒ 三条通道不再一致，复审决策 3；
- **绑定命令**：仓库缓存总量接近 10 GB（`gh api /repos/{owner}/{repo}/actions/caches`
  求和）或出现驱逐告警 ⇒ 复审决策 2（先降键粒度、再撤 target 缓存）；
- **绑定命令**：`python .github/scripts/check-workflows.py` 失败 ⇒ 复审 §2 的 if／needs
  组合（GitHub 对 `needs` 跳过语义的变更会首先在这里暴露）；
- 若 Windows 测试步在无缓存命中时也降到 8 分钟以下（说明 `extreme_scale` 不再是主要
  成本），或 1M 夹具改为增量生成，则决策 3 的前提消失，应复审是否恢复 push 期运行；
- 若平台范围或验收目标变更（ADR-013／ADR-073 修订），本 ADR 的“哪些通道跑什么”表
  须同批复核。

## 修订记录

- **2026-11-16（同日实测回填）**：首轮运行（`43894584718`，main@`b803dde`）全绿，按 §4
  记录前后墙钟；`target` 缓存因单键体积超出自设上限被撤回（决策 2、限制 1）；同时更正
  “BSD job 每轮从零装工具链”的错误前提——vmactions 自带 VM 镜像缓存。判据与门禁语义
  未变，只是把写下的撤回规则用在了自己身上。
