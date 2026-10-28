# 域 H：技能（Skills）

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 格式 | `packages/adapters/src/skills/index.ts`：目录发现委托 `scan.ts:43 scanSkillFilesUnderRoot`（根自身有 SKILL.md 先收录，再扫**一层**子目录 `:57-73`，不递归）；根解析 `roots.ts:21 resolveDefaultSkillRoots`（合并非 fallback，同层 `.zcode/skills` 优先于 `.agents/skills` `:107-118`；项目内自 `.git` 工作树根到 cwd 每层都当根 `:74-86`）；frontmatter `extractFrontmatter`（`:281`）要求 `---` 起始闭合，`parseFlatYaml` 只认顶层 `key: value`（`:309-313`），支持 `>`/`\|` 块标量（`:343`）；无 frontmatter 按目录名作 name（`:184`）；`SAFE_FRONTMATTER_KEYS` 仅 name/description/when_to_use/license/metadata 判定 `safeToAutoLoad`（`:21-27,232`） | 无 | 空白 |
| 注入 | `packages/core/src/context/sections/skills.ts`：`buildSkillsSection`（`:17`）返回 `{name:"Skills", source:"skills", injectionTarget:"meta_user", cacheHint:"dynamic"}`（`:27-36`）；内容 `- <name>: <desc>… (file: <path>)`，描述截 250 字符，预算 20,000 字符超限降级为仅名称+路径（`:9-11,39-58`） | 无 | 空白 |
| 分发 | `packages/bootstrap/src/app/bundled-skills.ts`：SEA 资产前缀 `zcode-bundled-skills/`（`:51`），首启按 manifest hash 解压到 `<cliStorageRoot>/bundled-skills/<hash>/`，临时目录 + rename 原子化、并发只一个赢家、seed marker 幂等（`:118-163`）；开发态沿候选目录原地读（`:103-116`）；根 priority 1,000,000 排在插件根之后（`:59`） | 无 | — |
| 锁版本 | **两份锁文件内容不同**：`apps/zcode-cli/skills-lock.json` 锁 **3 个**（opentui、vercel-react-best-practices、vercel-react-view-transitions，含 `skillPath`）；根 `skills-lock.json` 锁 **4 个**（agent-browser、ai-elements、dogfood、electron，无 `skillPath`）。**Grep 全库搜 `skills-lock\|computedHash\|skillsLock\|readSkillsLock\|skills_lock\|skills\.lock\|skillLock` 仅命中锁文件自身与无关 blocking/blocked 文本——仓库内无任何 `.ts`/`.js`/脚本读取或引用这两个文件** | 无 | ZCode 该机制存疑，采纳前需核对上游实际接线（0 信任）——**锁与实现脱节确认成立** |

## 2. `${ZCODE_SKILL_DIR}` 变量替换（修正原书位置）

- 实际在 **`packages/core/src/tool/handlers/skill.ts:95`**：
  `content.replace(/\$\{(CLAUDE_SKILL_DIR|ZCODE_SKILL_DIR)\}/gu, baseDirectory)`；
- 另有 shell 展开（`bootstrap/src/custom-command-shell-expansion.ts:193`）与 hooks 展开
  （`core/src/hooks/configured-runner-input.ts:122`），无上下文时显式报错。

## 3. 与 WHCode 差异修正（相对原差异书）

- 原判定**基本成立**；仅需修正锁文件数量/字段结构（3 vs 4，非单一“”集合）与
  `${ZCODE_SKILL_DIR}` 替换位置（`skill.ts:95` 内容替换，另有 shell/hooks 两处）。
- **skills-lock.json 无消费者——确认**（原差异书存疑项成立）。实际第三方技能经
  `.agents/skills/` 落地并由 `third-party/inventory.json` 记录 hash，与 `computedHash`
  无调用关系。
- WHCode 无技能系统，无对应物。

## 4. 存疑项

- `skills-lock.json` 是否为某未合入分支的生成产物、及由谁生成（无 writer 代码）`[待验证]`；
  `skillPath` 字段差异的语义 `[待验证]`；两文件是否被 CI 之外的发布脚本引用 `[待验证]`
  （当前全库无引用，倾向无）。
