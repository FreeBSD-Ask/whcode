# 内置根证书包来源与许可

本目录存放 WHCode 的内置信任根资产，供 ADR-040 的“内置（`builtin`）档位”使用：
出站 HTTPS 信任的初始副本与 `reset` 恢复来源。证书数据原样入库，**不得编辑**；
需要定制信任的用户在工作区 `ca-extra.pem` 追加，不修改本文件（ADR-040 决策 2）。

## 资产清单

| 文件 | 内容 | 字节数 | SHA-256 |
|---|---|---|---|
| `ca-bundle.pem` | Mozilla 根证书 PEM 串接包（118 份） | 200116 | `796f1389bfea13c0ed78f663b765baaff54a508e5483786bd6ece73c5e3e3cbe` |
| `LICENSE-MPL-2.0.txt` | Mozilla Public License 2.0 英文原文（373 行） | 16726 | `3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04` |

- 证书份数：**118**（`-----BEGIN CERTIFICATE-----` 与 `-----END CERTIFICATE-----`
  各 118 次，成对闭合）。
- 两个文件均为 UTF-8 无 BOM、LF 换行；`ca-bundle.pem` 不含 `PRIVATE KEY` 块，
  不含任何私钥材料（只有公钥证书）。
- SHA-256 为本项目对入库字节独立重算的结果，不是抄录上游声明。

## 上游与版本

| 项 | `ca-bundle.pem` | `LICENSE-MPL-2.0.txt` |
|---|---|---|
| 上游地址 | <https://curl.se/ca/cacert.pem> | <https://www.mozilla.org/media/MPL/2.0/index.txt> |
| 下载日期 | 2026-10-25（Asia/Shanghai） | 2026-10-25（Asia/Shanghai） |
| 上游版本日期 | `Last-Modified: Tue, 20 Oct 2026 13:32:40 GMT`（HTTP 响应头实测） | `last-modified: Mon, 10 Aug 2026 15:15:32 GMT`（HTTP 响应头实测） |
| 上游字节数 | `Content-Length: 200116`（与入库字节数一致） | `Content-Length: 16726`（与入库字节数一致） |
| 上游内容类型 | `application/x-pem-file` | `text/plain` |

`ca-bundle.pem` 文件头自述的证书快照时间与响应头 `Last-Modified` 一致，原文行为
`## Certificate data from Mozilla as of: Tue Oct 20 13:32:40 2026 GMT`（原样保留，未改写）。

## 独立核验（三次复查）

1. **查证（一手）**：以 `curl.exe -sS https://curl.se/ca/cacert.pem -o assets/certs/ca-bundle.pem`
   原样下载，未经过编辑器重写；入库字节数与响应头 `Content-Length` 相同。
2. **交叉验证（两个独立来源）**：
   - 上游发布的摘要文件 <https://curl.se/ca/cacert.pem.sha256> 内容为
     `796f1389bfea13c0ed78f663b765baaff54a508e5483786bd6ece73c5e3e3cbe  cacert.pem`，
     与本项目对入库字节重算的 SHA-256 一致；
   - `ca-bundle.pem` 文件头记录的证书快照时间与 HTTP 响应头 `Last-Modified` 一致。
3. **判定**：`ca-bundle.pem` 与上游 2026-10-20 快照**字节级一致（确证）**；
   `LICENSE-MPL-2.0.txt` 由 Mozilla 官方域名提供、未改写（`last-modified` 与
   `Content-Length` 已记录，未与第二独立来源逐字节比对，此项标 `[待验证]`）。

文件头第 19 行的 `## SHA256: beb7e6dfe6499926e52c075c27bcfbe4c957f8609c575b3860273ae2806f63eb`
**不是本文件自身的摘要**（本文件自身摘要为 `796f13…`，与上述上游摘要文件一致）。
按上游 `mk-ca-bundle.pl` v1.35 的输出格式，该行应为源文件 `certdata.txt` 的摘要；
本项目**未独立复核**该对应关系，标 `[待验证]`，不作为任何校验依据。

## 许可

- **证书包数据**：源自 Mozilla 的根证书库 `certdata.txt`，按 **Mozilla Public License
  2.0（MPL-2.0）** 记录；许可证原文见本目录 `LICENSE-MPL-2.0.txt`。
  `ca-bundle.pem` 自身文件头只标明数据来源与转换工具，**不含**许可声明；许可依据取自其
  数据源 `mozilla-firefox/firefox` 仓库
  `security/nss/lib/ckfw/builtins/certdata.txt` 的文件头原文（2026-10-25 抓取）：

  ```text
  #
  # This Source Code Form is subject to the terms of the Mozilla Public
  # License, v. 2.0. If a copy of the MPL was not distributed with this
  # file, You can obtain one at http://mozilla.org/MPL/2.0/.
  ```

  同文件头记录的来源与转换信息（`ca-bundle.pem` 第 4、8—11、18 行的**跨行摘录**，
  省略处以 `……` 标出，非连续原文块）：

  ```text
  ## Certificate data from Mozilla as of: Tue Oct 20 13:32:40 2026 GMT
  ……
  ## This is a bundle of X.509 certificates of public Certificate Authorities
  ## (CA). These were automatically extracted from Mozilla's root certificates
  ## file (certdata.txt).  This file can be found in the mozilla source tree:
  ## https://raw.githubusercontent.com/mozilla-firefox/firefox/refs/heads/release/security/nss/lib/ckfw/builtins/certdata.txt
  ……
  ## Conversion done with mk-ca-bundle.pl version 1.35.
  ```

- **不得混用的许可**：Rust crate `webpki-roots` 的代码／打包许可是
  **CDLA-Permissive-2.0**（见 ADR-040 依赖批准表），与本目录的证书数据许可无关，
  不得合并成同一套声明。
- 许可原文文件名固定为 `LICENSE-MPL-2.0.txt`，内容不得删改；升级版本时整份替换。

## 更新流程

1. 重新下载：`curl.exe -sS https://curl.se/ca/cacert.pem -o assets/certs/ca-bundle.pem`
   （原样覆盖，禁止用编辑器打开另存）；
2. 重算哈希与计数：SHA-256、字节数、`BEGIN CERTIFICATE` 块数，并与上游
   `cacert.pem.sha256` 对照；
3. 更新本文件：`上游版本日期`、`字节数`、`SHA-256`、`证书份数`、`下载日期`，
   并同步 `LICENSE-MPL-2.0.txt` 的来源日期与 SHA-256（若许可原文同时更新）；
4. 跑门禁：`python .github/scripts/check-eol.py`、
   `cargo test -p whcode-tests --test layout`；工作区副本的保真刷新与种子清单
   （`ca-builtin.json`）由证书库编排按 ADR-040 处理，不在本目录手工维护。

更新须单独提交，提交信息写明旧／新 SHA-256；不得与功能改动混在同一提交。

## 复审触发

满足任一条即复审本资产并给出新的来源、日期与 SHA-256：

- **每 6 个月**定期复审一次（上次复审：2026-10-25；下次不晚于 2027-04-25）；
- **Mozilla 根证书策略变化**：根证书移除策略、信任位调整、`certdata.txt` 结构或
  分发路径变更，或 `curl.se` 停止提供该导出；
- **任一包含证书到期**：按包内证书 `notAfter` 判定，包含证书批量临近到期即换版；
- 上游来源不可达或摘要文件与下载内容不一致（此时先停止更新，登记冲突）。

资产过期的运行期后果与兜底（剩余不足 30 天告警、已过期重新生成、`reset` 恢复）见
ADR-040“决策 3”与“复审触发”；本文件只负责来源与许可的记录。
