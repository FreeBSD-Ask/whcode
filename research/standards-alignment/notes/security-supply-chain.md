# T9 研读笔记：信息安全与软件供应链

**研读日期**：2026-11-14

## 1. ISO/IEC 27001:2022（ISMS 要求）

| 事实 | 值 | 证据层级 |
|---|---|---|
| 现行版 | 27001:2022（第 3 版，2022-10-25） | 确证（检索多源交叉） |
| Annex A | 93 控制项、四主题（组织 37/人员 8/物理 14/技术 34；11 新增、24 合并） | 确证 |
| 配套 | 27002:2022 为实施指南（非认证）；控制项五属性 | 确证 |

**对 WHCode**：ISMS 是组织级认证体系（4—10 章要求＋SoA）；单人项目做认证是合规表演（违反安全反教条）。可取的是 Annex A 技术主题里与本仓安全措施同构的条目做**自查对照**（不是对标认证）：访问控制（回环＋token）、加密（rustls/入站默认 TLS）、日志脱敏（logging.rs）、供应链完整性（依赖锁定）、技术脆弱性管理（每周 action-audit、Dependabot）。判定预判：思想已覆盖＋部分自查项可显式化；不做 ISMS。

## 2. NIST SSDF（SP 800-218，v1.1）

| 事实 | 值 | 证据层级 |
|---|---|---|
| 现行 | SP 800-218（2022-02，v1.1）；2025 年有 attestation 指南延伸 | 确证（csrc.nist.gov 官方页交叉） |
| 结构 | 4 组（PO 组织准备/PS 保护软件/PW 产出安全软件/RV 响应漏洞）、19 实践、42 任务 | 确证 |

**对 WHCode 的映射（本轮研读最“贴脸”的一份）**：

| SSDF 实践组 | 本地落点 | 初判 |
|---|---|---|
| PO（组织准备） | AGENTS.md 硬边界、philosophy、ADR、角色表 §4.1 | 已覆盖 |
| PS（保护软件） | 凭据不落库/日志、action SHA 固定＋action-versions.json、Dependabot、cargo --locked、vendor SHA256 校验（fetch-duckdb） | 已覆盖 |
| PW（产出安全软件） | 零 unsafe、clippy -D warnings、Code Scanning 零告警铁律、check-code-scanning.py、威胁模型（ADR-070/ADR-023）、脱敏测试 | 已覆盖（超配——多数项目无“零 open 告警”铁律） |
| RV（响应漏洞） | 已知缺口表、0 信任复测、CHANGELOG 修复段、ADR 复审触发 | 部分（无独立漏洞披露渠道/SECURITY.md 之外的过程成文较少——落地候选：RV 实践一段） |

SSDF 是免费、任务级、模型无关的框架——比 27001 更适合本仓粒度。判定预判：高覆盖；可做一张 19 实践×本地落点对照表进 matrix（落地候选）。

## 3. SLSA v1.0（供应链完整性）

| 事实 | 值 | 证据层级 |
|---|---|---|
| 现行 | v1.0 build track：L0 无保证 → L1 有 provenance → L2 托管构建签名 provenance → L3 加固构建 | 确证（slsa.dev 官方交叉） |
| 关键辨析 | **SHA 固定 action 是卫生基线（Scorecard 要求），但 SHA 固定≠provenance**；provenance 须签名可验证才构成完整性保证 | 确证（slsa.dev＋GitHub issue/分析交叉） |

**对 WHCode**：§5.1 action 铁律（最新 release＋固定 40 位 SHA＋action-versions.json＋每周联网复核）在“依赖面”上已是高配；构建产出的**签名 provenance（L1/L2）**是真实缺口——build-release.yml 产物无 attestation。落地候选：GitHub Artifact Attestations（托管 runner 上达 L2）——必须写正负和解（新增 action 依赖面 vs 发布产物可验证性），且 action 需过 §5.1 版本+SHA 铁律。倾向：候选登记，等真实矛盾（发布被引用/分发后）再立项。

## 4. 其他（awareness）

- IEC 62443（工业自动化安全）：不适用（非工业产品）。
- ISO/IEC 15408（CC 评估）：不适用。
- GB/T 22239-2019（等保 2.0）：不适用（非中国关键信息系统运营者；无上线运营面）。
- OWASP ASVS/Cheat Sheets：Web 面对照资源；本仓 web-deployment-security.md 的“明确不承诺”清单＋三层破坏性保护已覆盖同构项；Host 头校验/安全响应头/限速为显式登记缺口——维持现状（回环/私网边界下威胁模型不支持加开销），不因“行业标准有”而加。
