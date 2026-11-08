# 预览机制（CDP＋Playwright）评估结论

**状态**：评估完成，结论为产品侧不做（2026-11-09，FR-753／Task T262／Check 358）
| **性质**：评估结论登记，不是能力交付；产品依赖树与运行时行为均不因此文改变
| **上游**：[ADR-057](../adr/ADR-057-zcode-experience-alignment.md) 决策 9、
[plan-ui-zcode-alignment.md](../../specs/001-whcode-harness/plan-ui-zcode-alignment.md) 第 53 行

## 0. 任务认知卡（宪法原则 X）

- **方向**：回答“WHCode 要不要在产品内做浏览器预览（CDP 驱动＋Playwright 自动化）”，
  并把结论与代价写成可复评的记录，避免“研究里提过就默认要做”。
- **目标**：结论可判定（做／不做）、理由有一手锚点、代价与失效条件成文；仓库当前没有
  产品侧 CDP／Playwright 依赖这一事实可复核。
- **用户意图**（按本批指令推断）：终态功能路线要求每条提案有结论，不做也要写明为什么。
- **判据**：①结论与理由成文；②“测试侧验收工具”与“产品侧预览能力”的界限写明；
  ③未评估前不引入产品依赖这一前置条件可程序化复核。
- **非目标**：不实现预览；不改 Web 前端；不评估 GUI 侧的截图或像素比对（那是
  `gui-probe` 的验收面，与浏览器预览无关）。

## 1. 评估对象

ZCode 的预览机制是开发态工具：用 Chrome DevTools Protocol 驱动浏览器，配合
Playwright 做页面自动化，把前端渲染结果回显给编码代理（
[domain-M-frontend.md](../../research/zcode-comparison/domain-M-frontend.md) 第 11、19 行；
[actionable-gaps.md](../../research/zcode-comparison/actionable-gaps.md) 第 40 行把它标为
“开发态工具，低优先级”）。DSH 的对应能力是 `browser-use` 共享服务，挂 Playwright MCP／
Chrome DevTools MCP 一类 provider（
[domain-R-device-capabilities.md](../../research/dsh-comparison/domain-R-device-capabilities.md) 第 9、52 行）。

WHCode 的对应面不同：Web UI 是服务端直出＋原生 ESM（ADR-014），前端预算首屏 JS
不超过 50 KB（gzip）；桌面界面必须原生（ADR-026）。也就是说，仓库里没有“前端构建产物
需要浏览器调试验证”的结构性需求，只有“直出页面在真实浏览器里是否可用”的验收需求。

## 2. 现在证据（2026-11-09 复核）

1. **产品依赖树无预览依赖**：`crates/**/Cargo.toml` 检索 `playwright|chromium|cdp|devtools`
   零命中；仓库根无 `package.json`（`local/` 下的上游研究快照不计入产品树）。
2. **测试侧已有 Playwright，且只作验收**：`.github/scripts/web-ui-check.py` 用 Playwright
   驱动 Chromium 验证 Web tablist 的键盘循环、RTL、焦点与 ARIA 同步（Check 294）；
   `platform-verification.yml` 的 Windows job 临时 `pip install playwright` 并下载浏览器。
   它产出的是验收证据，不进入产品依赖，也不向用户暴露预览能力。
3. **既有验收路径已覆盖“页面是否真的能看”**：服务端直出有 HTML 契约测试与预算审计
   （`check-frontend-budget.py`），真实浏览器行为由 `web-ui-check.py` 定期复核。预览机制
   要解决的那个矛盾（“看不到页面实际渲染”）在验收通道里已经有可复现答案。
4. **没有用户需求证据**：任务与 spec 中没有任何“在 WHCode 内预览页面”的 FR；FR-753 本身
   是评估性需求。

## 3. 结论

**产品侧不做浏览器预览机制，不引入 CDP 驱动，也不把 Playwright 变成产品依赖。**

理由按形式审查排列：

1. **它不增加可判定证据**。预览的产出是“人看到的画面”，而本仓库的验收语言是程序化
   Check：直出 HTML 有契约测试，真实浏览器有 `web-ui-check.py`，视觉有 `gui-probe` 的
   像素统计。再加一条“代理自己开浏览器看页面”的路径，无法写成可复现的判定，属形式
   扩张（宪法 1.2-1）。
2. **它破坏本地优先与零运行时依赖**。CDP 需要外部浏览器可执行文件与调试端口，
   Playwright 需要驱动与浏览器下载；这与“运行时零第三方依赖”“默认回环监听”
   （ADR-023、ADR-014）正面冲突，代价落在部署者与安全面（一个可被模型驱动的浏览器
   等于新的逃逸面），收益只是开发态便利。
3. **它与前端形态的当前和解不一致**。ADR-014 选择“能多简陋就多简陋”，一个为调试
   复杂前端而生的能力，在直出＋原生 ESM 的结构里没有对应物；引进它相当于为未来可能的
   复杂前端预付成本。
4. **上游研究自己也没把它当成能力项**：ZCode 侧标为开发态工具、低优先级；DSH 侧挂在
   浏览器自动化 provider 上，属于“设备能力”而不是“对话界面”。

## 4. 不做的代价

- 开发者要在本地验证页面，需要自己起 `whcode serve --web` 再用浏览器打开；这不是新负担，
  当前验收流程本来就这么做。
- 没有“代理自动截图页面并自检”的自动化回路；若将来需要页面级视觉回归，首选扩展现有
  `web-ui-check.py` 的断言，而不是引入预览产品面。
- 若出现“用户在 WHCode 内预览自己写的页面／报告”的真实需求，本结论失效，按第 5 条复审。

## 5. 失效条件与复审触发（绑定命令＋判定）

| 触发 | 命令 | 判定 |
|---|---|---|
| 产品侧出现预览依赖 | `rg -n "playwright|devtools|chrome-remote-interface|9222" crates/ Cargo.toml` | 有命中即触发复审；命中出现在产品路径则视为本结论被绕过，须先改 ADR |
| 前端结构变化 | `rg -n "webpack|vite|rollup|tsconfig" --glob '!local/**' .` | 出现构建器或前端框架即复审 ADR-014 与本结论 |
| 验收通道失效 | `python .github/scripts/web-ui-check.py --help` 与 `platform-verification.yml` 的 tablist 步骤 | 步骤被删或长期红即复审“测试侧已覆盖”的前提 |
| 用户提出预览需求 | 新增 FR 或用户指令 | 直接复审；届时最小形态是“本地静态服务器＋系统默认浏览器打开”（零新依赖），不以 CDP 起步 |

**时限复审**：2027-05-09 或下一次 UI 批次结题时（以先到者为准）重跑上表命令。

## 6. 三重时间

- **过去来源**：ZCode 域 M 勘察（2026-10-29，`local/zcode-src`@3c5b227）、DSH 域 R
  （2026-11-04）、ADR-057 决策 9 的“倾向不采纳”。
- **现在证据**：本日检索结果（第 2 节四条）、`web-ui-check.py` 与 CI 步骤、前端预算脚本。
- **将来失效条件**：第 5 节四条触发；上游若把预览变成 harness 的默认能力，也只作为对照
  重读，不自动跟进（反进步叙事，宪法 1.4-2）。

## 7. 已知限制

- 本结论不含“页面视觉是否好看”的判断，那属于设计评审与 `ui-design.md` 的话题。
- “测试侧 Playwright”与“产品侧预览”的界限靠文档与依赖树共同维持；若未来把浏览器验收
  迁到产品二进制里，本结论的边界会失效，须重新评估。

## 8. 修订记录

- 2026-11-09（首版）：T262／FR-753 评估结论入库；结论为产品侧不做，测试侧 Playwright
  仅作验收；四条复审触发绑定命令。
