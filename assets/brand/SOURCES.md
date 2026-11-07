# 品牌资产来源与派生（assets/brand/）

**决定记录**：[ADR-061](../../docs/adr/ADR-061-brand-assets.md) | **强制级**：本目录只放品牌资产；
新增资产类型需在该 ADR 追加修订记录。

## 1. 母版

| 文件 | 来源 | 许可 | SHA-256 |
|---|---|---|---|
| `logo.svg` | 项目作者提供（2026-11-07），非上游第三方素材 | BSD-2-Clause（随本仓库，版权主体 FreeBSD 中文社区） | `c3d2dce9a5222618c5b57a5c23810b99dd57f0398cc48ae3b6b13f3f1c489a6c` |

**结构如实记录**：`logo.svg` 是**位图封装**，不是矢量图形——`<defs>` 内嵌两个
base64 PNG（整页 `img1` 1080×1080、叠加层 `img2` 414×487），由两个 `<use>` 按
`x`/`y` 偏移叠合（`img2` 在 `358,327`）。因此：

- 放大不无损，图标只能由该位图**降采样**得到；
- 任何派生必须按 `<use>` 偏移合成两层，只取第一层会丢字（脚本对此显式报错退出）。

## 2. 派生产物（入库，字节级可复现）

**唯一生成入口**：

```bash
python .github/scripts/make-brand-assets.py          # 重新生成
python .github/scripts/make-brand-assets.py --check  # 校验产物与母版一致（非零退出即漂移）
```

| 产物 | 消费方 | 形态 | 字节数 |
|---|---|---|---|
| `crates/whcode-gui/assets/brand/icon-64.rgba` | `whcode-gui` 窗口图标（`include_bytes!`） | 64×64 原始 RGBA，无压缩无元数据 | 16384 |
| `crates/whcode-web/frontend/favicon.ico` | `whcode-web` 的 `/favicon.ico` | ICO，16/32/48 三帧 | 8665 |

同一母版、同一 pillow 版本下两次生成字节一致（`--check` 可判定）。**跨机一致不承诺**：
pillow 的重采样实现随版本变化，`--check` 失败先比对 pillow 版本，再判定是否母版被改。

## 3. 终端形态（不由位图派生）

CUI／TUI 的 ASCII 字标在 `whcode-domain::brand::ASCII_LOGO`（源码常量，含尺寸测试），
**不是**把位图降采样打印——1080×1080 的古籍版面降采样后只剩噪点，认不出是 WHCode。
该字标是手工字形表拼出的 `WHCODE` 块字形＋一行定位说明，纯 ASCII、语言中立。
