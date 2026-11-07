# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![平台驗證](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | 繁體中文 | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

以 Rust 原生開發、本地優先的 LLM 編碼代理框架：資料放在單一 DuckDB 檔案裡，每個動作都可
追溯，規格驅動開發的關卡由程式執行，而不是靠自律。原生 GUI、TUI、CUI、Web 與 SSH 共用
同一套應用服務。

- **本地優先**：單檔 DuckDB 保存資料，隨時可以帶走；不強制帳號，也不強制連網。
- **可追溯**：每次工具呼叫都依「意圖—執行—登記」留下記錄。預設追蹤等級為 `redacted`，
  憑證不會出現在日誌與事件裡。
- **多前端**：GUI／TUI／CUI／Web／SSH 共用同一服務；桌面介面原生繪製，不內嵌瀏覽器引擎。
- **規格驅動**：需求、任務與驗收三方追溯，並由測試與腳本強制執行。

## 目前狀態

版本 `0.1.0`，尚未發布；變更見 [CHANGELOG.md](./CHANGELOG.md)。

M0—M6、M8 與四端對等已完成，M7 的 SSH 服務端也已完成。M9 的 5.5 小時穩定性長跑尚無
合格資料，真實 LLM 供應商的用戶端接線未交付。逐項進度、未完成原因與證據位置見
[專案狀態快照](./docs/guides/project-status.md)；權威帳本是
[tasks.md](./specs/001-whcode-harness/tasks.md) 與
[checklist.md](./specs/001-whcode-harness/checklist.md)，已知限制見
[compatibility.md](./docs/matrices/compatibility.md)。

## 平台支援

| 平台 | 定位 |
|---|---|
| Windows 11 amd64 | **唯一驗收目標**（`x86_64-pc-windows-msvc`，ADR-013） |
| Windows ARM64 | 建置交付目標（ADR-043）：CI 結果只作平台證據，不進入驗收 |
| FreeBSD | 完整平台證據通道（硬性關卡）：CI 隨 `main` 全量建置、測試與關卡，任一失敗即紅；不作驗收結論 |
| NetBSD | 隨 CI 於 `main` 自動執行；僅作平台證據 |
| OpenBSD／DragonFly | 手動預留；與 NetBSD 同為 SSH 遠端開發目標（作為被連入端，不要求原生工具鏈） |

效能分位數目前只取自 Windows 通道。平台口徑詳見
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) 與
[compatibility.md](./docs/matrices/compatibility.md)。

## 建置與執行

需要 Rust 1.99.1（已由 `rust-toolchain.toml` 鎖定）、MSVC 工具鏈與 Python 3；全程不使用
任何 GNU 工具鏈。

```powershell
# 首次建置前先取得並驗證 DuckDB 動態函式庫（SHA256）
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# 環境自我檢查
cargo run -p whcode-bin --release -- doctor

# 介面：Windows 預設開啟原生 GUI；--tui 終端介面；--cli 命令列
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <工作階段 ID>

# Web（預設 HTTPS 與自簽憑證；--tls off 回到明文）
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

不帶參數啟動時，Windows 會開啟原生 GUI；加 `--tui` 進入終端介面（`1`—`6` 切換檢視，
`q` 離開），加 `--cli` 使用命令列。TUI 在虛擬終端（Git Bash／mintty）或主控台處於
快速編輯選取模式時收不到鍵盤輸入，請改用 Windows Terminal 或 conhost 執行；完整按鍵與
檢視說明見 [tui.md](./docs/guides/tui.md)。

沒有可用主控台時（CI、管線、重新導向），程式不啟動介面，印出提示後以退出碼 0 結束。

## 介面入口

| 入口 | 啟動方式 | 說明 |
|---|---|---|
| GUI | `whcode`（Windows 預設）或 `whcode gui` | 原生唯讀介面，不內嵌瀏覽器引擎 |
| TUI | `whcode --tui` | 六個唯讀檢視，寫入操作需兩段確認，見 [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <子命令>` 或直接 `whcode <子命令>` | `--json` 統一信封與穩定退出碼，見 [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | 預設只監聽回送位址，入站 TLS 預設啟用，見 [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | 受限入口，預設不授予系統 Shell；四平台現場驗收延後 |

## 文件

- [docs/guides/README.md](./docs/guides/README.md)：全部指南的索引（建置、TUI、CUI、Web、
  並行與效能、資源預算、工程流程）
- [specs/001-whcode-harness/](./specs/001-whcode-harness/)：需求基線、任務、驗收清單與介面契約
- [docs/adr/](./docs/adr/)：架構決策記錄
- [docs/matrices/](./docs/matrices/)：平台矩陣、決策台帳、相容性與能力對等矩陣
- [docs/guides/project-status.md](./docs/guides/project-status.md)：專案狀態快照

## 參與

歡迎 Issue 與 PR。貢獻流程與每份 PR 必過的核對清單見 [CONTRIBUTING.md](./CONTRIBUTING.md)。
提交即表示同意以 BSD-2-Clause 授權本專案。專案由 FreeBSD 中文社群維護，
歡迎討論 BSD 移植、中文本地化與終端體驗。

## 安全

請勿透過公開 Issue 回報安全漏洞，回報管道與安全不變量見 [SECURITY.md](./SECURITY.md)。

## 授權

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD 中文社群（FreeBSD Chinese Community）。
