# ADR-047：发布链可重现构建（同提交重建字节一致）

- **状态**：已批准（2026-10-29，用户指令"确保可重现构建"，三项设计变更获"全部批准"）
- **来源**：v0.1.0-m0.1 发布后的重建对照实验（§1）；AGENTS.md §5 G5"标签指向可复现提交"；
  2026-10-23 用户要求"版本号 = 二进制构建时间"（本 ADR 修订其发布路径语义，见 §3）
- **责任人**：发布链（build-release.yml／package-windows.py／whcode-bin build.rs）维护者
- **退出条件**：出现跨机字节一致需求（须另立 ADR 评估 vendored 工具链与压缩器固定）；
  rustc／MSVC 版本演进导致同机重建不再一致且不可接受时复审本 ADR

## 1. 事实与证据（0 信任实测，2026-10-29）

对照实验：v0.1.0-m0.1 的 CI 发布包（run 40721288124 产物，sha256 `4be934e1…d431466`）
与本地同提交（109afce）全新构建重建包（sha256 `1c8d7fe6…6c513a5d`），逐条目对比：

- 除 `whcode.exe` 外**所有条目未压缩内容一致**：含 `duckdb.dll`（61 311 784 字节，
  CRC 相同）——预编译载荷 SHA256 强制校验（ADR-DB-BUILD）有效，差异不在内容而在元数据与 exe；
- zip 总 SHA256 不同，归因于**三个独立墙钟源**：
  1. `WHCODE_BUILD_STAMP`：版本号定义为构建墙钟（2026-10-23 用户要求），
     `whcode-bin/build.rs` 只取系统时钟、不支持外部注入 → exe 字节随构建时刻变化；
  2. `PACKAGE-INFO.txt` 的 `generated_utc`：取打包墙钟；
  3. zip 条目 `date_time`：`package-windows.py` 用 `ZipFile.write()` 默认取文件磁盘 mtime。
- **跨机源**（同实验实测，明确排除在目标外）：`duckdb.dll` CRC 相同而 deflate 压缩流差约
  800 KB（两台机器 Python 的 zlib 实现差异）；`whcode.exe` 未压缩内容差 9 216 字节
  （MSVC 工具链／代码生成差异）。二者都不因消除墙钟而消失。

结论：**同机同工具链**下消除三个墙钟源即可达成"同一提交两次全新构建 → zip 字节一致"；
跨机一致被 zlib 与 MSVC 阻挡，属另一个（大得多的）工程，不在本 ADR 范围。

## 2. 决策

1. **版本戳可注入**（`whcode-bin/build.rs`）：环境变量 `WHCODE_BUILD_STAMP` 已设置且
   合规式时原样透传（接受与墙钟路径同门校验的两种形式：`YYYY-MM-DDTHH:MM±HH:MM`（22）
   与 `YYYY-MM-DDTHH:MMZ`（17））；未设置或值非法时回退构建墙钟，非法值输出构建告警、
   不静默接受；新增 `cargo:rerun-if-env-changed` 使注入值变化驱动重建。
2. **发布构建注入提交时间**（`build-release.yml`）：checkout 后以 `git log -1 --format=%ct`
   导出 `SOURCE_DATE_EPOCH` 与 `WHCODE_BUILD_STAMP`（UTC `Z` 形式）写入 `GITHUB_ENV`，
   作用于该 job 的构建与打包步骤。发布物版本戳语义 = "该二进制源码对应的提交时刻"，
   不再字面等于编译墙钟；同一提交的 tag 重建得到恒定的 `--version`。
3. **打包确定性**（`.github/scripts/package-windows.py`）：尊重 `SOURCE_DATE_EPOCH`——
   `generated_utc` 与 zip 条目 `date_time` 改由基准秒导出（UTC 元组，早于 1980 钳到格式
   下限）；条目属性固定（`create_system = 0`、`external_attr = 0o644 << 16`）；条目顺序
   由既有 `sorted()` 保证。未设置该变量时保持墙钟行为（本地临时打包不伪装确定性），
   非法值显式中止。
4. **归因字段保留并补全**：`PACKAGE-INFO.txt` 保留 `rustc`／`python`／`os`（同机恒定、
   跨机差异的真实归因），新增 `source_date_epoch`；安装验证记录补 `python` 与
   `source_date_epoch`，证据搬家不销毁。
5. **范围声明**（2026-10-29 修订，见 §5）：同一提交 + 同一构建环境（同机，或同一 CI
   runner 镜像）+ **全新构建（全量 clean，含依赖重编重链）** → zip SHA256 一致；
   跨环境（不同机器／镜像）明确不支持——zlib 压缩流与 MSVC 工具链差异是物理输入。
   发布页 `.sha256` 仍是发布物的权威校验和。

## 3. 正负和解

- **得**：同一提交可验证重建（同环境）；CI 发布产物可复核；发布物 `--version` 恒定，
  安装验证记录与包内版本可交叉核对不漂移；G5"标签指向可复现提交"从过程可复现
  升级为可实测的字节复现（同环境）。
- **失**：发布物版本号不再字面等于编译时刻（语义改为提交时刻）——对 2026-10-23
  要求的修订，用户 2026-10-29 批准；本地未注入时仍为构建墙钟，开发语义不变。
- **承担者**：排查"两个包为何不同"的人需要读 `PACKAGE-INFO.txt`／安装验证记录的
  环境字段（rustc／python／os／source_date_epoch 齐全，可归因）。
- **不做的事**：不为跨机一致而 vendored 工具链、固定 zlib 或去掉环境归因字段——
  代价（仓库体积、维护负担、归因损失）当前大于收益。

## 4. 三重时间

- **过去**：2026-10-23 版本号 = 构建墙钟（用户要求）；2026-10-28 ADR-043 发布链落地，
  2026-10-29 首次重建对照暴露三个墙钟源与两个跨机源（§1）。
- **现在**：§1 实测证据；修复后验证已执行（见 §5 修订记录）：同提交两次全新构建
  SHA256 相同，注入生效与非法值回退均实测确认。
- **将来失效条件与复审触发**：rustc 工具链升级、MSVC/SDK 版本变化、新增 C 依赖
  （如 aws-lc-sys 更新）改变编译行为 → 同机重建可能不再一致，须重测并复审本 ADR；
  出现跨机一致需求 → 另立 ADR。

## 5. 修订记录

- 2026-10-29 初版：三项修复（版本戳注入、`SOURCE_DATE_EPOCH` 驱动打包、zip 条目
  时间固定）+ 范围声明（同机一致，跨机排除并归因）。
- 2026-10-29 验证实测（独立 worktree，HEAD＝9d27e22，EPOCH=`1793204134`，
  版本戳 `2026-10-28T16:15Z`）：fmt／clippy（全 workspace）／`test -p whcode-bin`／
  layering／layout／fetch-duckdb --check 全部通过；注入后 `whcode --version` 输出
  `2026-10-28T16:15Z`；两次全新构建（`cargo clean -p whcode-bin` 强制 build.rs 重跑）
  打包产物 SHA256 **相同**：`ed0ebf56000fb0bc0429939480dcf7fe04947f305021b453ce34471f1e58aa9e`；
  非法注入值 `"not-a-stamp"` 触发构建告警并回退墙钟（`2026-10-29T00:53+08:00`）。
  编号注记：本 ADR 曾编号 046，因并行会话的同号 ADR 文件已存在，改为 047。
- 2026-10-29 **第四墙钟源（v0.1.0-m0.2 发布后复核发现并修复）**：tag 触发的两次 CI
  发布运行（同一提交 0d4ecc8）zip SHA256 不同——逐条目解剖仅 `whcode.exe` 差 24 字节，
  其中 PE COFF `TimeDateStamp` 解码即两次**真实链接时刻**（`17:51:03Z`／`18:08:46Z`，
  与发布时间吻合），另有调试目录时间戳与 16 字节 GUID；`PACKAGE-INFO.txt` 的差异仅为
  exe 哈希行（打包层三源确定性成立）。根因：MSVC 链接器默认把链接墙钟嵌入 PE 头与
  调试元数据；本机 `cargo clean -p` 双构建测不出它（依赖缓存、仅重链 bin 时该元数据
  恰好稳定），全量 clean 双构建完全复现（时间戳 `0x6ae23dad`／`0x6ae23f4b`，zip
  `4e341e86…`／`d1f76f88…` 不同）。**修复**：`.cargo/config.toml` 为
  `x86_64-pc-windows-msvc` 与 `aarch64-pc-windows-msvc` 两目标加
  `rustflags = ["-C", "link-arg=/Brepro"]`（链接器以内容哈希替代墙钟）；验证：全量
  clean 双构建 PE 时间戳相同（`0x53980bbe`）且 zip SHA256 **相同**
  （`54bf733d6d99639c4b2d8f6ec160745028bd68f390c683e7611eab06c1292b09`）。config
  `rustflags` 路径与实验的 `RUSTFLAGS` env 路径产物逐字节一致（指纹等价，二次构建 7 秒命中）。
  范围声明随之修订（§2.5）："全新构建"明确为全量 clean；同一 CI 镜像内运行间一致
  预期成立（m0.3 起按此口径复核）。v0.1.0-m0.2 的两批资产（`e1b9fd06…`/`def1af91…`
  与 `2d5f28d3…`/`1d48dd26…`）均为通过验证的等价发布物，差异仅为链接元数据。
- 2026-10-29 **最终验收（v0.1.0-m0.3，提交 75d9138）**：tag 触发的发布运行与以同一
  tag 为 ref 的 `workflow_dispatch` 重跑（run 40759097860），两批发布包 SHA256
  **逐架构逐字节一致**：amd64 `f9303bfe395731cae165577216623e942034ad94a3ab8335602ab2e77ae78dc5`、
  arm64 `faf803839d04364bc5512e9e005c2f5f07ba937a8b1c762df171d4f181804cda`；发布物
  `--version` 输出 `2026-10-28T19:01Z`＝提交时刻（端到端确定性）。可重现构建目标达成：
  同一提交在同一构建环境内，无论谁在何时重建，发布包字节相同。
