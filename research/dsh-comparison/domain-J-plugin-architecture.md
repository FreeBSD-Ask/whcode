# 域 J：插件架构：Cordis、profile/bundle/patch、HMR

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 组合单位 | **一切皆插件**：模型适配器、工具注册表、会话日志、agent loop 本身都是插件（`docs/architecture.md` Cordis 节） | 15 个编译期 crate，依赖关系由 `tests/contract/test_layering.rs` 强制单向 | **语义不同不可比**：运行期组合 vs 编译期分层 |
| 贡献方式 | 服务（`ctx.<key>`）、类型化事件、**可逆效果**（`ctx.effect()`/`ctx.on()` 的注册在卸载时回卷）（`docs/cordis-primer.md`） | trait 实现 + 显式装配（`whcode-bin` 是唯一装配根） | **DSH 领先**：贡献可撤销 |
| 事件派发 | 五种模式：`emit`/`waterfall`/`parallel`/`serial`/`bail`，模式是事件公开契约（`cordis-primer.md`） | 无事件总线；事件是持久化记录（`EventRecord`），不是拦截点 | **结构性差异**：DSH 事件是扩展点，WHCode 事件是事实 |
| 组合层次 | profile → 有序 bundle → profile `cordis.patch.yml` → home patch → `--patch` 叠加；按 id 替换整行 config 或插入新行（`docs/architecture.md` Profiles and bundles） | 无运行期层次；feature 与 crate 依赖决定组合 | **DSH 领先** |
| 隔离 | `isolate` realm 让单会话拿到不同能力集（`docs/architecture.md`） | 无；能力集编译期固定 | **DSH 领先** |
| 热更新 | `dsh-hmr` 监听 profile 清单与两份用户 patch，重读 bundle 层并应用失效策略；`web` 模板默认开启（`packages/boot/app-boot/README.md`） | `cargo` 重编译重启；无热更新 | **DSH 领先** |
| 版本兼容 | 插件清单声明 `engines.dsh` 或 peer 范围，装载前校验；不兼容拒绝并报告风险；`compatibility.json` 存精确豁免（`app-boot/README.md`） | 无；同一 workspace 编译 | **DSH 领先** |
| 失败策略 | 可选插件失败→警告并继续；必需 id 失败→整体 dispose 并非零退出；`skippedBundles` 记录被跳过的 bundle（`app-boot/README.md`） | 编译失败即不可发布；运行期无“插件失败”态 | **DSH 领先** |
| 可诊断性 | `--dump-config` 打印合成后的完整行与来源层；`--dump-config-schema` 生成 JSON Schema（`app-boot/README.md`） | 无等价物；`doctor` 命令给出环境诊断 | **DSH 领先** |

## 2. DSH 实现解剖

### 2.1 三种贡献，一种回收

插件向共享 context 贡献：服务（抢占 `ctx.<key>`）、类型化事件、可逆效果
（`cordis-primer.md`）。关键在于**注册必须返回 disposer**，或经 `ctx.effect()` 安装，
使 reload/teardown 能按序回卷（`cordis-primer.md`）。没有“注册了就永远在”的通道。

### 2.2 补丁是整行替换，不是深合并

`cordis.patch.yml` 按 id 定位一行，**替换整份 config**（需要保留的字段必须重述）；也可插入新行
或 `!!js` 插值（`app-boot/README.md`）。bundle 的 patch 也是这样：`dsh-base` 以“一次 insert”
落在空 profile 根上，后续 bundle 与用户 patch 逐层按 id 覆盖（`packages/bundle/base/cordis.patch.yml:1`—
`4` 的注释）。整行替换避免了“深合并出意外组合”，代价是配置重复。

### 2.3 分层顺序固定，用户层最高

profile 的有序 bundle → profile patch → home patch → `--patch`（`docs/architecture.md`）。
home 层高于 profile 层这一点在文档里写明，避免“改 profile 被 home 悄悄覆盖”的困惑。

### 2.4 版本与信任边界

装载前校验 `engines.dsh` 或 peer 范围；不兼容则拒绝并**在启动器自己的副本里**把该行标成
`disabled`，模块根本不会被 import（`app-boot/README.md`）。文档同时声明“这些检查
不是对恶意包代码的沙箱”——边界诚实。

### 2.5 失败可见

可选插件失败→带标签的警告；必需 id（`agent-loop`、`webserver`、`acp`、`sdk-jsonrpc-server` 等）
失败→dispose 整个 app 并非零退出（`app-boot/README.md`）。bundle 级失败则跳过该 bundle 并
记入 `skippedBundles`，其余层继续（§Profiles）。

## 3. WHCode 现状核对

- 已实现：编译期单向依赖门禁（`tests/contract/test_layering.rs:135`、`:146`、`:161`：
  前端不依赖 store、domain 零内部依赖、前端互不依赖）；平台差异集中在 `whcode-platform`；
  新增 crate 需 ADR（`AGENTS.md` §4）。
- 已实现：单一装配根 `whcode-bin`（配置、日志、feature 组装）。
- 缺失：运行期插件加载、事件拦截总线、profile/bundle/patch 组合、隔离 realm、HMR、
  插件版本协商、失败降级。检索 `plugin`、`dlopen`、`hmr` 于 `crates/` 无实现命中。
- 语义差异：WHCode 把“组合”交给 Cargo 与 feature；DSH 把“组合”交给运行期 YAML 层。
  前者可静态证明依赖边界，后者可现场换实现。

## 4. 差异判定与可借鉴点

1. **可逆效果（disposer 契约）**：不可照搬，但可借鉴为**内部纪律**：WHCode 的注册面
   （MCP 工具、技能、GUI 视图）应有对称的注销路径，而不是只增不减。当前 `ToolRegistry`
   无 `unregister`，MCP 卸载时只能整体重建（`crates/whcode-tools/src/registry.rs:142`）。
2. **整行替换而非深合并**：可采纳为配置语义。若 WHCode 引入分层配置（用户配置覆盖默认），
   整值替换更容易证明“最终值来自哪一层”。代价：配置重复。
3. **失败分级（可选警告/必需退出）**：建议采纳为 MCP 服务器与技能的启动语义。当前 WHCode
   对技能坏 frontmatter 只告警（`skills.rs:31`—`66`），对 MCP 启动失败未分级；需要一张
   “必需/可选”清单。
4. **`--dump-config` 等价物**：可采纳为 `doctor --dump-config`。WHCode 已有 `doctor`，加一份
   “生效配置＋来源层”输出对排障价值高，成本低。
5. **运行期插件加载**：不采纳。与零 unsafe、静态分层、资源铁律（每会话不加常驻线程/句柄）
   冲突；DSH 的插件模型本身就是其资源问题的来源之一（多进程 Electron、无界 registry），
   WHCode 不应以“对齐”为名引入。
6. **HMR**：不采纳。开发期重编译已足够；常驻文件监听与资源铁律冲突。

## 5. 待验证项

1. `isolate` realm 的具体隔离边界（服务可见性？生命周期？）：`docs/architecture.md` 只给一句。
   取证：读 `packages/preset/src/**` 与 Cordis `isolate` 实现（`vendor/`）。
2. HMR 的热更新粒度（仅配置行还是模块代码）：`app-boot/README.md` 说“profile 清单与两份
   patch”，`dsh-hmr` README 可能补充模块根。取证：读 `packages/boot/hmr/README.md`。
3. WHCode 是否有 `ToolRegistry::unregister`：只读了 `registry.rs:126`—`170`，未见注销方法。
   取证：`rg -n "unregister|remove" crates/whcode-tools/src/registry.rs`。

## 6. 证据路径清单

- `docs/architecture.md`（Profiles and bundles）
- `docs/cordis-primer.md`（服务/事件/可逆效果）
- `docs/cordis-primer.md`（五种派发模式）
- `docs/cordis-primer.md`（disposer 要求）
- `packages/bundle/base/cordis.patch.yml:1`（整行替换语义）
- `packages/boot/app-boot/README.md`（必需/可选失败策略）
- `packages/boot/app-boot/README.md`（patch 层顺序）
- `packages/boot/app-boot/README.md`（HMR）
- `packages/boot/app-boot/README.md`（版本校验）
- `tests/contract/test_layering.rs:135`（前端不依赖 store）
- `tests/contract/test_layering.rs:146`（domain 零内部依赖）
- `crates/whcode-tools/src/registry.rs:142`（注册无对称注销）
