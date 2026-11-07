# SSH 回环验收证据索引（Check 25／96／99、T099）

来源：CI run [`41481740766`](https://github.com/FreeBSD-Ask/whcode/actions/runs/41481740766)
（main@ccc2cff，2026-11-02，ci workflow 全绿；含 platform-freebsd 首次硬门禁全绿）。
各文件为对应 job artifact 的原样落盘（仅行尾统一为 LF）。

## 各平台判定

| 文件 | 平台 | 判定 | 覆盖 |
|---|---|---|---|
| `freebsd-client.txt` | FreeBSD 15.1-RELEASE-p3 amd64 | `ssh_loopback_gate = GO` | 客户端公钥认证／受限命令／输出上限／超时断连；服务端认证／会话隔离／resize／安全退出（`bsd_client_acceptance_loopback` + `bsd_server_acceptance_loopback`） |
| `netbsd-client.txt` | NetBSD 11.0 amd64 | `ssh_loopback_gate = GO` | 同上四项 + 服务端四项 |
| `windows-client.txt` | Windows runner amd64 | `cargo test -p whcode-ssh` 通过 | 真实 TCP 回环（127.0.0.1）＋协议验收：主机密钥指纹／TOFU／指纹不匹配拒绝（中间人防线） |
| `windows-arm64-client.txt` | Windows ARM64 原生 | 同上 | 同上（ARM64 原生通道） |
| `openbsd-client.txt` | OpenBSD | `NO-GO（阻塞记录）` | ports rust 1.98.1 < MSRV 1.99，未执行 |
| `dragonfly-client.txt` | DragonFly BSD | `NO-GO（阻塞记录）` | dports rust 1.85.1 < MSRV 1.99，未执行 |

## 真实 sshd 互操作（记录型，`real_sshd_gate`）

同一脚本内另跑 `real_sshd_acceptance_loopback`：对目标机**系统自带 OpenSSH sshd**（真实
进程、独立密钥对、真实 TCP 回环）验证公钥认证／通道执行／退出码映射／stderr 捕获／
TOFU 记录／显式授权内容回传／超时终止。结果记在 `<platform>-client.txt` 的 `[real_sshd]`
节：`real_sshd_gate = GO／SKIPPED／NO-GO`。

- run `41568982820`：FreeBSD `real_sshd_gate = GO`（`sshd=/usr/sbin/sshd`、配置档 full、
  `REAL_SSHD_INTEROP: GO os=freebsd`）；Windows 本机实测 GO（Windows OpenSSH sshd）。
- **门禁政策**：记录型——不并入 `ssh_loopback_gate` 硬判定，稳定后再评估升门；
  跨机外部节点证据仍属 V-004／M7。
- 相关量化记录：`../bsd-perf/`（`bsd-perf.sh`，记录口径，不参与 ADR-025 预算判定）。

## 范围与边界（各记录内已注明）

- BSD 侧为**回环验收**（进程内测试桩）：验证客户端栈与服务端四项（Check 96），
  **不构成任何外部节点连通性证据**；V-004（M7）在各记录中显式标注“未取得”。
- OpenBSD／DragonFly 按现行 ADR-013 修订（2026-10-31 用户指令）无原生测试通道、
  保留手工占位；其阻塞原因（工具链 rust 低于 MSRV）随记录入库，非静默跳过。
  **恢复路径（2026-11-03 ADR-013 修订：NetBSD/OpenBSD/DragonFly 仅支持 SSH 远程
  开发，DragonFly 同日追加同待遇）**：OpenBSD／DragonFly 走真实连入取证——从
  Windows/FreeBSD 以内置 SSH 客户端连入真实主机（V-004 口径，被连入侧仅需 sshd）。
- NetBSD 定位（同修订）：被连入的 SSH 远程开发目标——原生 SSH 回环验收 GO（本目录
  `netbsd-client.txt`）即远程开发能力的直接证明；原生 store/DuckDB 经修订豁免。

## 关联证据

- store 门与进程组门、FreeBSD TUI/Web 冒烟：`../store-acceptance/`
  （`freebsd-store.txt` store_gate=GO 源码构建 DuckDB 2.0.0；
  `netbsd-store.txt` NO-GO＝上游 DuckDB `features.h` glibc 依赖，见 Check 22 注记）。
- 判定脚本：`.github/scripts/platform-matrix/bsd-ssh-client.sh`（记录头注明）。
