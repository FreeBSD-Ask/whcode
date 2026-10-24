# 内置字体来源与许可

本目录的字体用于 WHCode 原生 GUI 与 TUI 字体安装，**全部为 SIL Open Font License 1.1**
（可再分发；不得单独售卖字体）。每个字体目录内的许可文件为上游原文，不得删改。

| 用途 | 文件 | 上游 | 版本／来源 | SHA-256 |
|---|---|---|---|---|
| 英语比例 | `source/inter/Inter-Regular.ttf` | [rsms/inter](https://github.com/rsms/inter) | v4.1，`Inter-4.1.zip` 内 `extras/ttf/Inter-Regular.ttf` | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| 简中比例 | `source/noto-sans-sc/NotoSansCJKsc-Regular.otf` | [notofonts/noto-cjk](https://github.com/notofonts/noto-cjk) | `main` 分支 `Sans/OTF/SimplifiedChinese/`（无版本号，按 SHA-256 锁定） | `2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b` |
| 英语等宽 | `source/jetbrains-mono/JetBrainsMono-Regular.ttf` | [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono) | v2.304，`fonts/ttf/JetBrainsMono-Regular.ttf` | `e6fd0d7e91550b3ed2b735d4312474362c4716edc4fc0577a0f61ed782d5aed1` |
| 简中等宽 | `source/sarasa-mono-sc/SarasaMonoSC-Regular.ttf` | [be5invis/Sarasa-Gothic](https://github.com/be5invis/Sarasa-Gothic) | v1.0.42，`SarasaMonoSC-TTF-1.0.42.7z` 内 `SarasaMonoSC-Regular.ttf` | `82e09460f6f55010296f670b85e346ddeaf91b967724840d2bf5514137f68a83` |

## 许可文件

| 字体 | 许可原文 |
|---|---|
| Inter | `source/inter/LICENSE.txt` |
| Noto Sans CJK SC | `source/noto-sans-sc/LICENSE` |
| JetBrains Mono | `source/jetbrains-mono/OFL.txt` |
| Sarasa Mono SC | `source/sarasa-mono-sc/LICENSE` |

## 入库与运行时口径

- 本目录保存**完整字体文件**，用于离线复现与 TUI 用户级安装；下载日期 2026-10-23。
- GUI 运行时**只加载子集**（由 `.github/scripts/subset-fonts.py` 生成），避免整份字体常驻
  （FR-712）；子集缺字时由系统字体栈回退，并在文档中标注覆盖范围。
- 更新字体时必须同时更新本文件的版本、来源与 SHA-256，并重新生成子集。
