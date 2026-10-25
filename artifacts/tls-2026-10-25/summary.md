# T192 真实运行取证与资源复核（2026-10-25）

- 生成时间（UTC）：2026-10-25T07:12:37.135571+00:00
- 开始时间（UTC）：2026-10-25T07:11:36.431944+00:00
- 平台：Windows 11 amd64（`x86_64-pc-windows-msvc`）；OS 实测
  `Microsoft Windows 11 IoT 企业版订阅 LTSC`，版本 `10.0.26300`，64 位；
  逻辑处理器 16；取证脚本 Python 3.14.2
- 工具链：rustc 1.99.1 / cargo 1.99.1；curl 8.21.0（Schannel）
- 二进制：`target/debug/whcode.exe`（由 `cargo build -p whcode-bin` 产出，构建时间戳见 run.log 的 `whcode --version`）
- 默认 TLS（builtin）证书 DER SHA-256：`30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9`
- external 档位证书 DER SHA-256：`6ca496caa396e5607a2693d0386a250f982449c7e6260c7633ba4c64c09a7028`
- 跨进程 verify 对端证书 DER SHA-256：`873325ab37bac1a422e0adf94b5336f7099b2f9c42858d4289588ecf0f686339`
- 局域网取证地址：`192.168.5.4`

## 结论总表

| 编号 | 检查项 | 结果 | 证据 |
|---|---|---|---|
| 1.1 | 默认 HTTPS 启动成功（进程就绪） | 已取得 | 就绪（HTTP 200） |
| 1.2 | 启动输出含 https:// 地址与协议 | 已取得 | https://127.0.0.1:18443/ |
| 1.3 | 启动输出含证书 SHA-256 指纹 | 已取得 | 指纹 30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9 |
| 1.4 | 启动输出含证书设置页地址 | 已取得 | https://127.0.0.1:18443/settings/certs |
| 1.5 | curl -k 访问 /api/sessions 返回 200 且为合法 JSON | 已取得 | HTTP 200 |
| 1.6 | curl -k 访问 /settings/certs 返回 HTML 且含文件路径 | 已取得 | HTTP 200，7872 字节 |
| 1.7 | 不带 -k 的 curl 因自签不受信而失败 | 已取得 | curl exit 60；stderr：curl: (60) schannel: SEC_E_UNTRUSTED_ROOT (0x80090325) - 证书链是由不受信任的颁发机构颁发的。 More details here: https://curl.se/docs/sslcerts.html curl failed to verify the |
| 1.8 | 启动打印的指纹等于证书 DER 的 SHA-256 | 已取得 | 30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9 vs 30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9 |
| 1.9 | GET /api/certs 的 inbound.sha256 与启动指纹一致 | 已取得 | 30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9 vs 30b9442d30a388da09a28899a4016323aaf7e3d282686219c7cdf984da0c87e9 |
| 1.10 | 启动与查询响应均不含私钥 | 已取得 | 扫描 stdout/stderr 与响应体 |
| 2.1 | --tls off 启动成功 | 已取得 | 就绪（HTTP 200） |
| 2.2 | 启动输出为 http:// 且声明入站 TLS 已关闭 | 已取得 | http://127.0.0.1:18080/ |
| 2.3 | curl http://…/api/sessions 返回 200 | 已取得 | HTTP 200 |
| 2.4 | --tls off 不写回 settings.json、不生成证书库 | 已取得 | off 档位在 prepare_tls 中提前返回，未触碰证书库 |
| 2.5 | settings.inbound=off 重启后仍以 http 提供服务且设置未被改回 | 已取得 | HTTP 200；settings 保持 off |
| 3.1 | builtin 工作区生成了可用的服务端证书与私钥 | 已取得 | d:\WHCode\script\tls-verify\ws\s3-builtin\.whcode\certs\server-cert.pem；d:\WHCode\script\tls-verify\ws\s3-builtin\.whcode\certs\server-key.pem |
| 3.2 | external 档位以 HTTPS 提供服务且声明入站档位为 external | 已取得 | 就绪（HTTP 200） |
| 3.3 | external 加载的材料指纹与外部证书 DER SHA-256 一致 | 已取得 | 6ca496caa396e5607a2693d0386a250f982449c7e6260c7633ba4c64c09a7028 vs 6ca496caa396e5607a2693d0386a250f982449c7e6260c7633ba4c64c09a7028 |
| 3.4 | external 档位下 API 可用且 /api/certs 报告 external | 已取得 | /api/sessions HTTP 200；inbound.mode=external |
| 3.5 | external 启动与响应不含私钥 | 已取得 | 扫描 stdout/stderr 与响应体 |
| 3.6 | 不配对的证书与私钥被拒绝启动且错误可操作 | 已取得 | exit 1；Web 服务失败：parse_error: 构造入站 TLS 服务端配置失败：keys may not be consistent: KeyMismatch；请确认证书链与私钥匹配（同一密钥对），且私钥为未加密的 PKCS#8／PKCS#1／SEC1 PEM（ADR-040 决策 3）；证书库目录 d:\WHCode\script\tls-verify\ws\s3-bad\.whcode\cert |
| 4.1 | 找到本机 RFC1918 IPv4 | 已取得 | 192.168.5.4 |
| 4.2 | 局域网 + HTTPS 启动成功（认证启用） | 已取得 | 就绪（HTTP 403） |
| 4.3 | 无 token 的 HTTPS 请求被拒绝（4xx 且不返回数据） | 已取得 | HTTP 403 |
| 4.3b | 无 token 返回 401（ADR-023 措辞） | 未取得 | 实测 HTTP 403；实现返回 403 permission_denied，docs/guides/web-deployment-security.md §10.1 已登记该差异（以实现为准、ADR 待修订），Check 283 接受 401／403 |
| 4.4 | 带 Authorization: Bearer 的请求返回 200 | 已取得 | HTTP 200 |
| 4.5 | 局域网请求与响应不含私钥 | 已取得 | 扫描 stdout 与响应体 |
| 4.6 | 缺 --token-file 绑定局域网地址被拒绝（退出码非 0 且错误含认证要求） | 已取得 | exit 1；Web 服务失败：invalid_state: 局域网监听必须提供 --token-file（ADR-023：认证仅在显式 token 下启用；token 只从文件读取，不经命令行传递） |
| 5.1 | /settings/certs 返回 HTML，含只读状态摘要与文件路径 | 已取得 | HTTP 200；7856 字节 |
| 5.2 | 设置页不含私钥正文 | 已取得 | 扫描 HTML 响应体 |
| 5.3 | GET /api/certs 返回契约字段且指纹与启动输出一致 | 已取得 | HTTP 200 |
| 5.4 | POST /api/certs/modes 把 inbound 改为 off 并置 restart_required=true | 已取得 | HTTP 200；restart_required=True |
| 5.5 | 改回 builtin 成功且仍标记需重启（不热切换监听） | 已取得 | HTTP 200；restart_required=True |
| 5.6 | POST /api/certs/verify 给出明确结果（成功或明确失败，不静默） | 已取得 | HTTP 500；自探针指向同一单线程服务会占住该线程，结果为明确超时而非证书判定 |
| 5.7 | 跨进程 verify：自签对端不在信任库时明确失败 | 已取得 | HTTP 400；code=invalid_state |
| 5.8 | 追加对端证书为信任根后 verify 返回 ok 且指纹与对端一致 | 已取得 | HTTP 200；ok=True；sha256=873325ab37bac1a422e0adf94b5336f7099b2f9c42858d4289588ecf0f686339 |
| 5.9 | 证书 API 全部响应不含私钥 | 已取得 | 扫描 stdout/stderr 与全部响应体 |
| 6.1 | TLS 档位线程数不高于 --tls off（允许相等） | 已取得 | off 68 → TLS 68 |

## 场景、命令全文与证据文件

### 场景 1：默认 HTTPS（端口 18443）

```text
target\debug\whcode.exe serve --web --bind 127.0.0.1:18443 --workspace script\tls-verify\ws\s1-default
curl -k https://127.0.0.1:18443/api/sessions
curl -k https://127.0.0.1:18443/settings/certs
curl -k https://127.0.0.1:18443/api/certs
curl https://127.0.0.1:18443/api/sessions   # 不带 -k，预期失败
```

- 启动输出与证书库提示：`s1-stdout.log`、`s1-stderr.log`
- `/api/sessions`：`s1-api-sessions.headers.txt`、`s1-api-sessions.body`
- `/settings/certs`：`s1-settings-certs.headers.txt`、`s1-settings-certs.body`
- `/api/certs`：`s1-api-certs.body`
- 不带 `-k` 的失败：`s1-api-sessions-no-k.headers.txt` 与 run.log 的 curl 退出码／标准错误

### 场景 2：`--tls off`（端口 18080）与设置持久（端口 18081）

```text
target\debug\whcode.exe serve --web --bind 127.0.0.1:18080 --workspace script\tls-verify\ws\s2-off --tls off
curl http://127.0.0.1:18080/api/sessions
target\debug\whcode.exe serve --web --bind 127.0.0.1:18081 --workspace script\tls-verify\ws\s2-settings-off   # 无 --tls，settings.json inbound=off
```

- 证据：`s2-off-stdout.log`、`s2-api-sessions.body`、`s2-settings-off-stdout.log`、`s2b-api-sessions.body`
- `--tls off` 不写 `settings.json`（按次覆盖不回写）；把 `inbound: "off"` 写入设置后重启仍为明文，说明该选择不会被静默改回 HTTPS。

### 场景 3：`external` 档位（端口 18444）

```text
target\debug\whcode.exe serve --web --bind 127.0.0.1:18444 --workspace script\tls-verify\ws\s3-builtin     # 生成材料
copy ws\s3-builtin\.whcode\certs\server-cert.pem ws\s3-external\.whcode\certs\
copy ws\s3-builtin\.whcode\certs\server-key.pem  ws\s3-external\.whcode\certs\
# ws\s3-external\.whcode\certs\settings.json: {"schema_version":1,"outbound":"builtin","inbound":"external"}
target\debug\whcode.exe serve --web --bind 127.0.0.1:18444 --workspace script\tls-verify\ws\s3-external
curl -k https://127.0.0.1:18444/api/certs
```

- 配对校验（负向）：把 `s3-builtin` 的证书与 `s3-other` 的私钥拼成一对，
  `external` 档位启动失败，错误列出“匹配”与文件路径（`s3-bad-stderr.txt`）。
- **诚实标注**：外部材料是同一对自签材料，不是企业 CA 链；本组只验证
  `external` 档位加载、配对校验与档位切换路径可用，不宣称浏览器公共信任。
- 证据：`s3-external-stdout.log`、`s3-api-sessions.body`、`s3-api-certs.body`、`s3-bad-stderr.txt`

### 场景 4：局域网 token + HTTPS（192.168.5.4:18445）

```text
target\debug\whcode.exe serve --web --bind 192.168.5.4:18445 --token-file script\tls-verify\token.txt --workspace script\tls-verify\ws\s4-lan
curl -k https://192.168.5.4:18445/api/sessions          # 实测 403（ADR-023 写 401，差异已登记）
curl -k -H "Authorization: Bearer <token>" https://192.168.5.4:18445/api/sessions   # 200
target\debug\whcode.exe serve --web --bind 192.168.5.4:18445 --workspace script\tls-verify\ws\s4-notoken   # 缺 token，预期拒绝启动
```

- token 为 48 字符随机十六进制，只记录长度；值不进入任何证据文件。
- 证据：`s4-lan-stdout.log`、`s4-api-sessions-no-token.headers.txt`、`s4-api-sessions-with-token.body`、`s4-notoken-stderr.txt`

### 场景 5：设置页与证书 API（端口 18448，verify 对端端口 18450）

```text
target\debug\whcode.exe serve --web --bind 127.0.0.1:18448 --workspace script\tls-verify\ws\s5-api
target\debug\whcode.exe serve --web --bind 127.0.0.1:18450 --workspace script\tls-verify\ws\s5-peer   # verify 的对端
curl -k https://127.0.0.1:18448/settings/certs
curl -k https://127.0.0.1:18448/api/certs
curl -k -X POST --data-binary '{"inbound":"off","confirm":true}' https://127.0.0.1:18448/api/certs/modes
curl -k -X POST --data-binary '{"inbound":"builtin"}' https://127.0.0.1:18448/api/certs/modes
curl -k -X POST --data-binary '{"host":"127.0.0.1","port":18448}' https://127.0.0.1:18448/api/certs/verify
curl -k -X POST --data-binary '{"host":"127.0.0.1","port":18450}' https://127.0.0.1:18448/api/certs/verify
curl -k -X POST --data-binary '{"kind":"extra","pem":"<对端公开证书 PEM>"}' https://127.0.0.1:18448/api/certs/ca
```

- 证据：`s5-settings-certs.body`、`s5-api-certs.body`、`s5-modes-off.body`、`s5-modes-back.body`、`s5-verify.body`、`s5-verify-peer-untrusted.body`、`s5-ca-extra.body`、`s5-verify-peer-trusted.body`
- `verify` 自探针（指向自身）会占住同一单线程服务，返回明确超时；
  跨进程探针先返回证书不受信，追加对端证书为 `ca-extra` 后返回 ok。

### 场景 6：资源复核

```text
target\debug\whcode.exe serve --web --bind 127.0.0.1:18082 --workspace script\tls-verify\ws\r-off --tls off
target\debug\whcode.exe serve --web --bind 127.0.0.1:18449 --workspace script\tls-verify\ws\r-tls
```

| 指标（3 次采样中位数） | `--tls off` | 默认 TLS |
|---|---|---|
| RSS（字节） | 42942464 | 47300608 |
| 线程数 | 68 | 68 |
| 句柄数 | 215 | 216 |
| CPU（单核百分比，区间中位数） | 1.9286 | 3.078 |
| CPU（区间明细） | [1.5416, 2.3156] | [3.0825, 3.0735] |

- 线程数不增加断言：`True`
- 相对差值（同一 debug 夹具、同一方法）：RSS +4,358,144 字节（约 +4.16 MiB），
  句柄 +1，CPU 中位数 +1.15 个百分点（单核）。本任务只对线程数设断言（不得增加），
  RSS／句柄／CPU 仅记录，不设阈值；debug 构建的绝对值不代表发布包口径。
- 原始样本与方法：`resources.json`；进程输出：`r-off-stdout.log`、`r-tls-stdout.log`

## 未取得项

- 4.3b 无 token 返回 401（ADR-023 措辞）：实测 HTTP 403；实现返回 403 permission_denied，docs/guides/web-deployment-security.md §10.1 已登记该差异（以实现为准、ADR 待修订），Check 283 接受 401／403

## 诚实标注与边界

- `external` 场景使用自签材料，只证明材料加载与档位切换，不代表公共信任链。
- `verify` 自探针指向自身：服务是单线程非阻塞循环，请求处理期间无法为
  同一进程的另一条 TLS 连接推进握手，因此返回 2 秒预算用尽的明确错误；
  这不是“证书校验通过”，也不能据此判断信任库结论。跨进程探针用来区分
  信任判定路径（未追加根 → 不受信失败；追加后 → ok）。
- 无 token 的局域网请求实测为 403（`permission_denied`），与 ADR-023 的
  “401”措辞不一致；该差异已在 `docs/guides/web-deployment-security.md`
  §10.1 登记，Check 283 接受 401／403，本次未改代码。
- 私钥与 token 不入库、不入证据：证据只含路径与 SHA-256；临时工作区与
  脚本在 `script/tls-verify/`，取证完成后删除。
- 采样夹具为 debug 构建（非 release），RSS 绝对值不代表发布包口径；
  本组只用同一夹具做 off 与 TLS 的相对比较（线程、句柄、CPU 不增）。
- 浏览器实机操作（信任自签步骤、设置页交互）未在本机执行，按 ADR-037
  仍属未取得项，本次只用 curl 与进程观测取证。
- `*.headers.txt` 原为 HTTP 报文（行尾 CRLF）的原样采集；为符合仓库“文本一律 LF”
  规则，已把 CRLF 归一为 LF，其余字节未改。
