# ADR-040：TLS 证书库与 Web 内置证书

- **状态**：已批准（2026-10-25，用户指令“内置安全证书、可编辑、可启用关闭，关闭用系统证书”）；
  本 ADR **设计先于实现**，决策据此定稿，实现与实测证据未取得处一律标 `[待验证]`
- **来源**：FR-626／FR-627（**待 T184 登记**：主 `specs/001-whcode-harness/spec.md`
  当前尚无该两号，本 ADR 只作设计依据，不宣称需求已入账）；NFR-213（安全优先、默认回环、
  凭据不落日志）、NFR-215（零 unsafe、有界缓冲、零新增常驻线程）、NFR-219（威胁模型、
  攻击面、证据、代价、失效条件）；ADR-031（出站 TLS 与信任根现状）、ADR-023（局域网边界与
  认证）、ADR-022（Origin 匹配语义）、ADR-032（四端对等与理由码）；
  `.trae/specs/add-tls-certificate-store/spec.md`（已批准的变更总纲）
- **责任人**：Web 适配层负责人（入站 TLS 与服务循环）、LLM 变更维护者（出站信任装配）、
  安全与可观察性负责人（证书库、审计与威胁模型）、前端对等负责人（登记表与理由码）
- **退出条件**：需要公网暴露、需要操作系统吊销／策略语义、需要 mTLS、需要证书热切换、
  内置证书包更新周期无法维持、`rustls`／`ring` 出现需处置的安全公告、或 Web 端编辑
  不再满足运维需求（需要 headless 自动化）时，修订本 ADR 并给出迁移与回退证据

## 背景与矛盾

两处现状都把“信任什么”固定在代码或默认值里，用户既看不见也改不了：

1. **出站**：`crates/whcode-llm/Cargo.toml` 以 `ureq = { default-features = false,
   features = ["rustls"] }`（`http` feature，默认关闭）接入 HTTPS，信任根由 `ureq 3.5.0`
   携带的 `webpki-roots`（`Cargo.lock` 实测 `1.0.11`）在编译期固定。企业代理或内网 CA
   不受信任，用户也无法移除某个不再信任的根；ADR-031 第 4 条缺口（“代理与自定义 CA
   未支持”）正是此处。
2. **入站**：`crates/whcode-bin/src/serve.rs` 的 `ServeCli` 只有 `--web`／`--ssh`／
   `--bind`／`--token-file`／`--workspace`／`--config`，无任何 TLS 参数；`serve_web` 以
   `format!("http://{addr}")` 构造 Origin 允许集并直接打印 `http://` 地址。ADR-023
   明确“不承诺 TLS”，于是局域网模式下 token 与业务数据在链路上明文可见。

矛盾：**安全默认值（回环、token、Origin）已经把“谁可以连”收敛了，却没有回答“链路本身
是否可信”**。继续不做，局域网内同网段抓包即可读取 token（ADR-023 已把令牌当作唯一认证
手段）；一律强制内置，又会让浏览器默认拒绝自签证书、把“可控的明文”换成“不可用的
HTTPS”。本 ADR 因此不接受二选一，而是把证书做成**可见、可编辑、可关闭**的工作区配置，
并给出关闭路径（系统证书库／明文）。

## 决策

### 1. 工作区证书库（唯一落点）

- 目录 `<workspace>/.whcode/certs/`，与工作区数据库同一信任边界（同用户、同 ACL）。
- 文件与上限（上限即拒绝阈值，不是建议值）：

  | 文件 | 内容 | 上限 |
  |---|---|---|
  | `settings.json` | 出站档位、入站档位、来源版本与时间戳 | 16 KiB |
  | `ca-builtin.pem` | 可编辑的内置根证书副本（PEM 串接） | 512 KiB |
  | `ca-extra.pem` | 用户追加根证书（两档位都生效） | 512 KiB |
  | `ca-builtin.json` | 种子清单：来源版本、SHA-256、是否被修改 | 16 KiB |
  | `server-cert.pem` | 入站证书链（`external` 档） | 64 KiB |
  | `server-key.pem` | 入站私钥（`external` 档，**未加密**） | 16 KiB |

  表中 `ca-builtin.pem`／`ca-extra.pem` 的 512 KiB、`server-cert.pem` 的 64 KiB 与
  `server-key.pem` 的 16 KiB 与变更 spec 的请求上限一致；`settings.json` 与
  `ca-builtin.json` 的 16 KiB 是本 ADR 增补的设计上限（spec 只要求“有界”），实现期须
  落为常量并配上限测试，不得留作隐含默认值。

- **明文反射**：全部为公开格式（PEM／JSON），可直接复制、编辑、删除，也可被 `reset`
  覆盖回内置默认；不使用私有容器、不做混淆（NFR-218 退出权）。
- **原子写入**：同目录临时文件 + 重命名替换，失败即回滚并保持原文件字节不变；
  审计写入失败时同样回滚（与 `tests/contract` 既有事务口径一致）。
- **容量有界**：条目数、单文件字节数与 API 请求体总长（640 KiB）均在解析前判定；
  超限返回 `parse_error`／`invalid_state` 与具体原因，不做截断（NFR-215）。
- **禁止项**：不写入私钥到日志、事件、审计正文与 API 响应；不存在
  `disable_verification` 或等价“跳过校验”开关，`Dangerous` 级配置无法经设置页写入。

### 2. 出站信任：两档，永不关闭校验

| 档位 | 信任来源 | 追加 | 说明 |
|---|---|---|---|
| `builtin`（默认） | `ca-builtin.pem`（内置 Mozilla 根证书副本，资产 `assets/certs/ca-bundle.pem`） | `ca-extra.pem` | 用户可编辑、可 `reset`；文件未被修改时随程序版本保真刷新 |
| `system` | 操作系统证书库（Windows ROOT 存储，经 `rustls-native-certs`） | `ca-extra.pem` | 内置副本不再参与；状态响应标注来源为 `system` |

- **两档都不关闭**证书链与主机名校验；不存在“信任所有”档位。
- **系统库为空不得静默回落**：读取成功但证书数为 0 时返回可操作错误（含来源、平台与
  补救方式），不得改用内置库成功。
- **种子与保真刷新**：`ca-builtin.json` 记录种子来源版本与 SHA-256；启动时若工作区副本
  自种入后未被修改且内置资产更新，则刷新并记录旧／新 SHA-256；若已被修改则保留副本并
  显式提示“已修改、内置版本较新”。`reset` 从内置资产恢复并清除该标记。
- **资产许可**：`assets/certs/ca-bundle.pem` 的上游、日期、SHA-256 与许可见
  `assets/certs/SOURCES.md`；证书数据本身按 MPL-2.0 记录。注意区分：crate
  `webpki-roots` 的代码／打包许可是 CDLA-Permissive-2.0（本机 registry 一手核验），
  二者不得混用同一套许可声明。

### 3. 入站 Web TLS：三档（BREAKING）

| 档位 | 行为 | 备注 |
|---|---|---|
| `builtin`（默认） | 首次运行按工作区生成 ECDSA P-256 自签证书，397 天有效期 | 每工作区唯一私钥，不随包分发 |
| `external` | 用户提供的 PEM 证书链 + **未加密**私钥 | 带口令私钥、证书与私钥不匹配一律拒绝 |
| `off` | 明文 HTTP（ADR-023 现状） | 保留为显式回退，不被静默改回 |

- **SAN 集合**：`localhost`、`127.0.0.1`、`::1`、本机名、绑定 IP 字面量。
- **变更需重启**：档位与证书替换在重启 `serve --web` 后生效；API 响应携带
  `restart_required = true`，不热切换监听、不断开现连接。
- **优先级**：`serve --web --tls <mode>` 参数 > 工作区设置 > 默认（`builtin`）。
  参数是**按次覆盖**，不回写工作区设置。
- **过期处理**：距到期不足 30 天给出警告；已过期时 `builtin` 自动重新生成并记录新旧
  指纹，`external` 不自动替换（用户资产不得被程序改写）。
- **安全边界不变**：TLS 不是认证替代品——RFC1918 绑定仍必须提供 `--token-file`；
  Origin 允许集在启用 TLS 时 scheme 由 `http` 改为 `https`（ADR-022 的 host／端口匹配
  语义不变）。
- **单线程握手有界**：沿用 `crates/whcode-bin/src/serve.rs` 的单线程非阻塞状态机
  （`Phase` 增加 TLS 握手阶段），每连接一份 TLS 缓冲、总连接数仍受 `MAX_CLIENTS = 96`
  约束，握手在既有 `REQUEST_IDLE_TIMEOUT`（5 s）内未完成即关闭；**不新增常驻线程**，
  不为每次握手创建线程（NFR-211／215）。

### 4. 编辑面：Web 设置页 + 受控 API + 四端登记

- **页面**：`/settings/certs`（服务端直出 + 原生 ESM，纳入 FR-618 预算；无 JS 时显示
  只读状态与文件路径）。
- **API**：`/api/certs/*`（状态、档位、CA 编辑／重置、服务端证书上传／重新生成／下载、
  连通性校验），全部经既有认证（`check_auth`）、Origin（`check_origin`，ADR-022）与
  能力判定。
- **能力**：`settings.cert.read`／`settings.cert.write`（新增于
  `crates/whcode-app/src/authz.rs` 的 `Capability`；当前 9 项中无此二项，实现期补齐）。
- **确认与审计**：降档（入站 `off`、写空 `ca-builtin.pem`、上传外部私钥）须显式
  `confirm = true`；成功变更写 `tls_config_change` 审计（目标、动作、档位、证书份数、
  SHA-256），不含 PEM 正文与私钥；审计失败即回滚文件并返回错误。
- **私钥只进不出**：响应任何字段都不含私钥或其内容；下载面只提供证书链与指纹。
- **连通性校验**：`POST /api/certs/verify` 以当前出站信任完成一次握手，**同步**执行，
  单次最多阻塞服务循环 2 秒，不发送任何凭据。
- **四端登记**（ADR-032 单表口径）：

  | 能力 | Web | CUI／TUI | GUI |
  |---|---|---|---|
  | `settings.cert.read`／`settings.cert.write` | `Implemented`（`/settings/certs`、`/api/certs/*`） | `Excluded`：`web_settings_only` | `Excluded`：`gui_read_only_v1`／`six_view_scope` |

  被排除端调用返回 `not_supported`（CUI 退出码 55、Web 侧 501 同码信封）。

### 5. 依赖逐项批准表

只批准下列依赖；**每项都不得引入第二个加密提供者**，不得使用 `unsafe`，不得引入 GNU
工具链（D-01：Windows 只用 msvc）。版本策略统一为：`Cargo.toml` 显式锁定，`Cargo.lock`
为事实源，升级须同 PR 更新锁文件与许可证复核记录。

| 依赖 | 用途 | 版本策略（2026-10-25 事实） | 许可证（一手核验） | 替代方案 | 风险与失效条件 |
|---|---|---|---|---|---|
| `rustls` | 入站 TLS 服务端与出站信任装配 | 直接依赖，`ring` 提供者；锁文件已有 `0.23.46`（依赖 `ring`／`rustls-pki-types`／`rustls-webpki`／`subtle`／`zeroize`，**无** `aws-lc-rs`） | `Apache-2.0 OR ISC OR MIT` | 不复用 `aws-lc-rs`（`russh` 栈已带，见风险栏） | 提供者不一致会导致两份 crypto 后端并存；失效条件：`ring` 停更或需要 FIPS／平台验证器 |
| `rustls-native-certs` | 读取 OS 证书库（Windows ROOT 存储） | 新增；**锁文件中不存在**，具体版本 `[待验证]` | `[待验证]`（本机 registry 无缓存，无法一手核验） | 平台专有 API（`whcode-platform` 内 `cfg`） | 读库失败或 0 证书须报错；失效条件：需要吊销／OS 策略语义时改用平台验证器 |
| `rustls-pki-types` | 证书／私钥／DNS 名称类型（与 `rustls` 同源） | 直接依赖，锁文件 `1.15.1` | `MIT OR Apache-2.0` | 不另造封装 | 版本必须与 `rustls 0.23.46` 兼容；破坏性升级跟随 `rustls` |
| `rcgen` | 生成每工作区 ECDSA P-256 自签证书 | 新增；**锁文件中不存在**，具体版本 `[待验证]`；必须选 `ring` 后端而非 `aws-lc-rs` | `[待验证]` | 手写 DER 构造（否决：易错且无审计面） | 默认特性可能拉入 `aws-lc-rs`，须显式限定特性；失效条件：需要 CSR／多主体扩展时重评 |
| `x509-parser` | 读取服务端证书主题／SAN／有效期／指纹（状态展示与校验） | 新增；**锁文件中不存在**，具体版本 `[待验证]` | `[待验证]` | `rustls` 内置解析（不足以取全部展示字段） | 解析器与 `rustls` 判定不一致时以 `rustls` 为准；失效条件：`rustls-pki-types` 提供等价 API |
| `sha2` | 证书与指纹的 SHA-256 | 新增直接依赖；锁文件**同时存在** `0.10.9` 与 `0.11.1`（`0.11.1` 由 `russh` 栈的 `ed25519-dalek`／`p256`／`pageant`／`bcrypt-pbkdf` 使用） | `MIT OR Apache-2.0` | 复用 `ring` 的摘要（接口不同，需自行封装） | 择一即可**不新增版本**；实际选择以 `cargo tree -i sha2` 复核为准 `[待验证]`；失效条件：上游二选一收敛后跟随 |

**明确不引入**：`rustls-platform-verifier`（见“备选方案”）、`openssl`／`openssl-sys`、
任何 Async TLS 运行时（`tokio-rustls` 等）、任何第二加密提供者（`aws-lc-rs` 只作为
`russh` 的传递依赖存在，自有代码不引用）。第三方依赖内部含 C 与汇编（`ring`、
`aws-lc-sys`）不构成自有 Rust 的 `unsafe` 例外（NFR-215 只约束自有 crate）。

## 威胁模型与代价（NFR-219）

- **资产**：①入站私钥（可冒充服务端解密流量）；②信任根集合（可被用于中间人）；
  ③审计与状态（可被篡改以掩盖变更）。
- **攻击面**：
  1. **本机同用户进程**：可直接读 `.whcode/certs/`（与数据库同一 ACL 边界），也可向
     `ca-extra.pem` 写入伪造根证书——本 ADR **不承诺**同用户隔离；
  2. **局域网持 token 用户**：可经 API 改档位／换证书，须 `settings.cert.write` 且留审计；
  3. **跨站请求**：浏览器从非允许 Origin 发起写请求须 403（ADR-022 语义延续，scheme
     切换为 `https`）；
  4. **超大请求／慢握手**：单连接缓冲与总连接数有界（`MAX_CLIENTS = 96`、请求体上限、
     5 s 空闲上限）；
  5. **恶意 CA**：用户自行追加的根会被无条件信任（这是“可编辑”的必然代价），
     对策是明文文件、可见状态与可 `reset`。
- **证据**：以“配置可见 + 变更可审计 + 提交可回滚 + 状态不含私钥”为判据；实现期以
  契约测试（越权、跨站、超限、私钥不进响应）与真实浏览器握手实测取证 `[待验证]`。
- **代价**：自签证书在浏览器默认被拒（需手工信任或改 `external`）；私钥落盘依赖
  OS ACL；仅 Web 端可编辑；入站变更需重启；`system` 档位叠加 `ca-extra.pem` 时由
  `rustls` 校验，不复用 Windows 的策略与吊销检查；连通性校验同步执行最多阻塞 2 秒；
  每连接 TLS 缓冲在 96 连接上限下占用可测但非零；证书包存在陈旧风险。
- **明确不承诺**：同用户／管理员隔离、公网暴露、吊销（CRL／OCSP）与 OS 信任策略、
  mTLS、密钥口令保护（`external` 私钥为未加密文件）。

## 正负和解

- **正面**：出站信任从编译期常量变成可见配置，企业 CA 可追加、可疑根可移除、可一键
  回内置；`serve --web` 由明文默认变为 HTTPS 默认，局域网 token 不再裸奔；入站可换成
  企业签发证书消除浏览器警告；全部文件为公开格式，退出与备份路径不变；四端差异以
  理由码显式登记，不使用“说不清”。
- **负面（逐项，含承担者）**：
  1. 自签证书默认不被浏览器信任（承担者：用户；缓和：状态页给出指纹与信任指引）；
  2. 私钥明文落盘，安全性依赖 OS ACL 与同用户隔离假设（承担者：用户与安全复核人）；
  3. 设置页仅 Web 提供，CUI／TUI／GUI 无法完成轮换（承担者：运维用户与对等门禁）；
  4. 入站档位／证书变更需重启（承担者：运维用户）；
  5. `system` 档位叠加追加证书时 `rustls` 不复用 Windows 策略与吊销（承担者：安全复核人）；
  6. 连通性校验同步阻塞服务循环，单次 ≤ 2 秒（承担者：同机其他客户端）；
  7. 内置证书包存在陈旧风险，需保真刷新与来源记录（承担者：发布负责人）；
  8. 每连接 TLS 缓冲叠加到 96 客户端上限，内存与握手成本需实测（承担者：性能负责人）；
  9. 依赖面新增 6 项直接依赖，其中 3 项（`rustls-native-certs`／`rcgen`／`x509-parser`）
     引入仓库此前没有的 crate，版本与许可证待核验，供应链审计面扩大（承担者：安全复核人）；
  10. 入站默认从 `http` 改为 `https` 属 BREAKING（承担者：脚本化访问与既有文档）。
- **和解理由**：以“可见配置 + 显式关闭 + 审计”替代“固定常量 + 无 TLS”，让默认更安全
  而退出成本不升高——关闭路径仍是 `--tls off`／`system`／明文，且不需要改代码或删文件。
  拒绝在本轮引入异步 TLS 与平台验证器，是为了守住“单线程有界 + 单一加密提供者”的现有
  和解，未取得的代价（异步收益、OS 吊销）如实登记而不假装已解决。

## 三重时间

- **过去来源**：ADR-031 把出站 TLS 定为 `rustls` + 编译期 `webpki-roots`，并明确
  “升级到企业代理／自定义 CA”是退出条件之一；ADR-023 明确“不承诺 TLS”并要求局域网
  必须 token；ADR-022 把 Origin 匹配定为 scheme + host，且当前允许集写死为 `http`。
  三份 ADR 共同造成“链路不可信 + 信任不可改”的现状。
- **现在证据**（2026-10-25，本机 `d:\WHCode`，均为一手文件核验）：
  - `Cargo.lock`：`ureq 3.5.0` → `rustls 0.23.46`（`ring`／`rustls-pki-types 1.15.1`／
    `rustls-webpki 0.103.16`／`subtle`／`zeroize`）、`webpki-roots 1.0.11`、`ring 0.17.14`、
    `sha2` 0.10.9 与 0.11.1 并存；`rustls-native-certs`／`rcgen`／`x509-parser` **不在锁文件**。
  - `crates/whcode-llm/Cargo.toml`：`ureq` 为 optional，`features = ["http"]` 默认关闭。
  - `crates/whcode-web/Cargo.toml`：无 TLS 与网络依赖（仅 `whcode-app`／`whcode-domain`／
    `serde_json`／`duckdb` 类型透传）。
  - `crates/whcode-bin/src/serve.rs`：`ServeCli` 无 `--tls`；`serve_web` 以
    `format!("http://{addr}")` 构造 Origin；`MAX_CLIENTS = 96`；
    `REQUEST_IDLE_TIMEOUT`／`WRITE_STALL_TIMEOUT = 5 s`；单线程非阻塞多路复用。
  - `crates/whcode-app/src/authz.rs`：`Capability` 共 9 项，无 `settings.cert.read|write`。
  - `crates/whcode-domain/src/audit.rs`：`AuditAction` 无 `tls_config_change`。
  - `assets/` 仅 `fonts/`；`assets/certs/ca-bundle.pem` **尚未生成**（生成、SHA-256 与
    `SOURCES.md` 属实现期产物）。
  - 许可证一手核验（本机 `~/.cargo/registry/src/*/` 的 `Cargo.toml`）：`ring`
    `Apache-2.0 AND ISC`、`rustls` `Apache-2.0 OR ISC OR MIT`、`rustls-pki-types`
    `MIT OR Apache-2.0`、`rustls-webpki` `ISC`、`sha2` `MIT OR Apache-2.0`、`ureq`
    `MIT OR Apache-2.0`、`webpki-roots` `CDLA-Permissive-2.0`。
  - **已知不一致（如实登记，不在本 ADR 内擅自改 spec）**：变更 spec 写
    `webpki-roots 1.0.10`，`Cargo.lock` 实测为 `1.0.11`；以可复现文件为准。
  - **实现证据未取得**：自签证书生成、握手、浏览器信任、内存与预算、审计落库、四端
    拒绝语义，全部标 `[待验证]`，不得以设计代替实测。
- **将来失效条件与复审触发**：见文末“复审触发”。任一成立即修订本 ADR 并补迁移与回退
  证据，不得沿用“当前和解”。

## 备选方案（已否决）

- **`rustls-platform-verifier`（OS 策略／吊销）**：能拿到平台吊销与策略语义，但**无法
  叠加**用户追加的 `ca-extra.pem`（其信任来源封闭），与本 ADR 的核心能力冲突；
  且新增平台差异需落到 `whcode-platform`，扩大改动面。保留为“需要 OS 吊销语义”时的
  候选，届时须新 ADR。
- **随包分发固定私钥／固定自签证书**：所有工作区共用一份私钥即等于公开私钥，
  任何同网段用户都可解密流量，直接否决（已由 spec 的“每工作区独立私钥”场景锁定）。
- **反向代理终止 TLS（nginx／caddy）**：满足加密但不满足“内置、可编辑、可关闭”的
  用户指令；还引入外部进程与额外凭据面，并把 Origin／token 语义拆到两处判定。
- **继续无 TLS（维持 ADR-023 现状）**：局域网内 token 与业务数据明文可读，与“安全优先”
  的既有和解冲突；仅在用户显式 `--tls off` 时作为退路保留。
- **入站只做 `external`（不内置自签）**：空工作区将无法启动 HTTPS，默认体验倒退，
  且把证书获取成本推给每个用户。

## 影响

- **需求**：`specs/001-whcode-harness/spec.md` 新增 FR-626／FR-627（待 T184 登记）、
  修订 FR-606（默认 TLS、关闭须显式）；`contracts/app-service.md`（证书命令面）、
  `contracts/cui-cli.md`（`serve --web --tls <mode>`）、`quickstart.md`、
  `tasks.md`（T184—T192）、`checklist.md`（Check 274—288）。
- **代码归属**（不跨层）：
  - `whcode-domain`：`tls` 设置类型、`AuditAction::TlsConfigChange`、i18n 词条；
  - `whcode-app`：`cert` 证书库编排（读写、原子替换、reset、刷新）、`Capability` 两项、
    `SurfaceEntry` 与理由码；
  - `whcode-llm`（`http` feature 内）：`http::tls` 信任装配（`builtin`／`system` + 追加）；
  - `whcode-web`：入站 TLS 配置类型（纯数据类型：档位、上限、状态字段）、`/api/certs/*`
    路由、`/settings/certs` 资产与 `surface_map` 映射；**不引入 `rustls`**；
  - `whcode-bin`：`serve` 装配（`--tls` 解析与优先级）、**TLS 握手状态机**（落在既有
    `serve_loop` 旁）、`doctor` 证书状态、证书库初始化。
- **依赖落点与 ADR-031 边界**：`rustls` 只出现在 `whcode-llm`（出站，`http` feature）与
  装配根 `whcode-bin`（入站握手），维持 ADR-031“网络依赖只进装配根 + `whcode-llm`
  可选 feature、四个前端 crate 零网络依赖”的既有边界；`rcgen`／`x509-parser`／`sha2`
  属本地密码学与解析、不发起网络请求，随证书库编排落在 `whcode-app`。因此
  **握手状态机不迁入 `whcode-web`**——若迁入，该前端 crate 将获得 `rustls` 依赖，
  与 ADR-031 的边界冲突，而本 ADR 不允许顺手改写该边界。
- **与变更 spec 的措辞差异（如实登记）**：spec 的“受影响代码”段把“`tls` 服务端配置与
  非阻塞握手状态机”整体记在 `crates/whcode-web`；本 ADR 拆分为“`whcode-web` 只持配置
  类型与路由、握手在装配根”，属设计细化。T184 登记任务时须按本 ADR 对齐 spec 措辞，
  不得两处各自表述。
- **依赖**：见决策 5 的批准表。
- **门禁**：`.github/scripts/benchmarks/check-frontend-budget.py`（新资产）、
  `tests/contract/test_frontend_parity.rs`（新能力四端格）、`tests/contract/test_layout.rs`
  （资源说明与白名单）、`traceability.py`、`check_counts.py`。
- **资源说明**：`docs/guides/repository-layout.md` 第 23 行现仅登记 `assets/fonts/`
  （“新增资源类型需 ADR”），本 ADR 即为 `assets/certs/` 的批准依据；该表行同步属实现期
  文档任务，**未同步前布局门禁按旧表判定**。
- **不受影响**：DuckDB schema 与迁移、`whcode-store`、SSH 服务端与客户端、运行时
  payload、TUI／GUI 视图代码（仅登记表状态）、发布包结构。

## 回退路径

- **入站回退**：`--tls off` 或入站档位 `off` 即回到 ADR-023 的明文形态；`serve` 循环的
  TLS 阶段可整段摘除（协议字节变换与安全判定未改）。
- **出站回退**：档位置 `system` 或删除 `ca-extra.pem`；`rustls` 调用点移除后回到
  `ureq` 的 `webpki-roots` 默认（ADR-031 现状）。
- **文件回退**：删除 `.whcode/certs/` 即恢复“首次使用重建内置副本”；`reset` 提供逐文件
  级回退，不需要改代码。
- **登记表回退**：`settings.cert.*` 两格若需撤销，须同 PR 移除能力、路由、页面与矩阵行，
  不得只删一格（ADR-032 第 5 条）。

## 已知未完成与缺口

1. **真实 Provider 调用仍归 T163／T167**：本 ADR 只批准接入信任来源，不改
   `llm run` 的四端 `Reserved` 状态（ADR-031 的客户端未交付事实不变）。
2. **连通性校验同步执行**：单次最多阻塞服务循环 2 秒，异步化需 ADR-015 并发档案，
   本轮不做。
3. **内置证书包更新流程未定**：资产版本、刷新节奏、SHA-256 记录与 `SOURCES.md` 生成
   均属实现期；陈旧风险由复审触发兜底。
4. **BSD 仅代码级交叉检查**：`aws-lc`／`cc` 既有缺口不变，不改变 ADR-013 的验收范围；
   不宣称 BSD 上的 TLS 行为已实测。
5. **spec 版本号笔误**：`webpki-roots 1.0.10` 与 `Cargo.lock` 的 `1.0.11` 不一致，
   由 T184 登记时一并修正。
6. **三项依赖许可证未一手核验**（`rustls-native-certs`／`rcgen`／`x509-parser`）：
   引入前须从 crate 元数据或上游 `LICENSE` 取证并记入依赖记录，不得沿用二手结论。
7. **`assets/certs/` 与布局白名单**：新资源类型已获批，但 `repository-layout.md` 与
   `test_layout.rs` 同步未完成。

## 0 信任校验

- **查证（一手）**：依赖版本与提供者来自 `Cargo.lock` 与 `crates/*/Cargo.toml`；
  许可证来自本机 registry 缓存中每个 crate 的 `Cargo.toml`；服务循环常量与
  `--tls` 缺失来自 `crates/whcode-bin/src/serve.rs`；能力与审计枚举缺失来自
  `authz.rs`／`audit.rs`；资产缺失来自 `assets/` 列举。以上均为文件级事实，不依赖记忆。
- **交叉验证**：同一事实至少两个来源——`ureq → webpki-roots` 既见于 `Cargo.lock` 的
  `dependencies` 列表，也见于变更 spec 的现状描述（版本号不一致已单独登记）；
  “无 `--tls`”既由 `ServeCli` 字段确认，也由 `parse` 的可用参数错误串确认；
  “单线程”由模块文档的并发设计档案与 `serve_loop` 签名（`&mut Connection` 顺序处理）
  双重确认。
- **判定**：现状（无 TLS、无证书库、无相关能力与审计）为**确证**；依赖批准表的
  “已存在版本”为确证，“新增版本与许可证”为**未核验**（`[待验证]`）；全部行为性结论
  （握手、证书生成天数、内存、预算）为**未取得**，不得默认成立。

## 复审触发

- 需要公网暴露或多来源白名单配置（ADR-023 退出条件成立）；
- 需要操作系统吊销（CRL／OCSP）或平台信任策略，此时评估 `rustls-platform-verifier`；
- 需要 mTLS 或客户端证书认证；
- 需要证书／档位热切换或异步 TLS，此时须补 ADR-015 并发设计档案与回退验证；
- 内置证书包更新周期无法维持，或资产版本落后超过复审期（到期须给出新的来源与 SHA-256）；
- `rustls`／`ring`／`rustls-native-certs`／`rcgen` 出现需处置的安全公告，或加密提供者收敛
  为单一时重评依赖表；
- Web 端编辑不再满足运维（需要 headless 自动化轮换），届时复审理由码
  `WEB_SETTINGS_ONLY` 并评估 CUI 面；
- 登记表与矩阵漂移而门禁通过（负向自测失效），立即复审门禁本身（ADR-032 触发⑦）。

**复审责任**：Web 适配层负责人（入站与服务循环）、LLM 变更维护者（出站装配）、
安全与可观察性负责人（威胁模型与依赖）、前端对等负责人（四端登记）。
