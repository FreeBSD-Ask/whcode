# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![プラットフォーム検証](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | 日本語 | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Rust ネイティブでローカル優先の LLM コーディングハーネスです。データは DuckDB の 1 ファイルに
収まり、あらゆる操作が追跡できます。仕様駆動開発のゲートは、人の規律ではなくプログラムが
執行します。ネイティブ GUI、TUI、CUI、Web、SSH の各フロントエンドは同じアプリケーション
サービスを共有します。

- **ローカル優先**: DuckDB の単一ファイルに保存し、データはいつでも持ち出せます。アカウント
  も強制的なネットワーク接続もありません。
- **追跡可能**: ツール呼び出しは「意図—実行—登録」の形で記録されます。既定のトレース
  レベルは `redacted` で、認証情報はログにもイベントにも残りません。
- **複数のフロントエンド**: GUI／TUI／CUI／Web／SSH が同じサービスを使います。デスクトップ
  画面はネイティブ描画で、ブラウザエンジンを埋め込みません。
- **仕様駆動**: 要件・タスク・受け入れ基準を三者間で相互に追跡し、その追跡をテストと
  スクリプトが強制します。

## 現状

バージョン `0.1.0`、未リリースです。変更は [CHANGELOG.md](./CHANGELOG.md) を参照してください。

M0—M6、M8 と 4 フロントエンドの同等性は実装済みで、M7 の SSH サーバーも動作します。M9 の
5.5 時間安定性試験にはまだ合格できるデータがなく、実 LLM プロバイダーのクライアント接続は
未提供です。項目ごとの進捗、未完了の理由、根拠の所在は
[プロジェクト状態スナップショット](./docs/guides/project-status.md) にあります。権威ある台帳は
[tasks.md](./specs/001-whcode-harness/tasks.md) と
[checklist.md](./specs/001-whcode-harness/checklist.md)、既知の制限は
[compatibility.md](./docs/matrices/compatibility.md) です。

## 対応プラットフォーム

| プラットフォーム | 位置づけ |
|---|---|
| Windows 11 amd64 | **唯一の受け入れ対象**（`x86_64-pc-windows-gnullvm`、ADR-073） |
| Windows ARM64 | ビルド成果物の配布対象（ADR-043）。CI の結果はプラットフォーム証拠にとどまり、受け入れには含めない |
| FreeBSD | 完全なプラットフォーム証拠チャネル（ハードゲート）。`main` のたびに CI が全量ビルド・テスト・ゲートを実行し、1 つでも失敗すれば赤になる。受け入れ結論にはしない |
| NetBSD | `main` で CI が自動実行。プラットフォーム証拠のみ |
| OpenBSD／DragonFly | 手動の枠のみ。NetBSD と同様に SSH リモート開発の接続先（つながれる側であり、ネイティブなツールチェーンは要求しない） |

性能の分位値は現在 Windows チャネルのみです。プラットフォームの方針は
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) と
[compatibility.md](./docs/matrices/compatibility.md) を参照してください。

## ビルドと実行

Rust 1.100.0（`rust-toolchain.toml` で固定）、LLVM 系ツールチェーン（rustc gnullvm ＋ llvm-mingw。`python .github/scripts/setup-windows-llvm.py` で導入）、Python 3 が必要です。
GNU ツールチェーンは一切使いません。

```powershell
# 初回ビルドの前に DuckDB 共有ライブラリを取得して検証する（SHA256）
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# 環境の自己診断
cargo run -p whcode-bin --release -- doctor

# 画面: Windows は既定でネイティブ GUI、--tui で端末 UI、--cli でコマンドライン
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <セッション ID>

# Web（既定は HTTPS と自己署名証明書、--tls off で平文に戻る）
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

引数なしで起動すると、Windows ではネイティブ GUI が開きます。`--tui` で端末 UI（`1`—`6` で
ビュー切替、`q` で終了）、`--cli` でコマンドラインです。TUI は疑似端末（Git Bash／mintty）
や、コンソールが QuickEdit の選択モードにある間はキー入力を受け取れません。Windows
Terminal か conhost で実行してください。キー割り当てと各ビューの説明は
[tui.md](./docs/guides/tui.md) にあります。

利用できるコンソールがない場合（CI、パイプ、リダイレクト）、画面は起動せず、案内を表示して
終了コード 0 で終わります。

## 画面の入口

| 入口 | 起動方法 | 説明 |
|---|---|---|
| GUI | `whcode`（Windows の既定）または `whcode gui` | ネイティブの読み取り専用画面。ブラウザエンジンを埋め込まない |
| TUI | `whcode --tui` | 6 つの読み取り専用ビュー。書き込みは 2 段階の確認を経る。詳細は [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <サブコマンド>` または `whcode <サブコマンド>` | `--json` の共通エンベロープと安定した終了コード。詳細は [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | 既定はループバックのみ、受信側 TLS は既定で有効。詳細は [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | 制限付きの入口。既定ではシステムシェルを渡さない。4 プラットフォームでの実機受け入れは保留 |

## ドキュメント

- [docs/guides/README.md](./docs/guides/README.md)：全ガイドの索引（ビルド、TUI、CUI、Web、
  並行性と性能、リソース予算、開発プロセス）
- [specs/001-whcode-harness/](./specs/001-whcode-harness/)：要件ベースライン、タスク、
  受け入れチェックリスト、インターフェース契約
- [docs/adr/](./docs/adr/)：アーキテクチャ決定記録
- [docs/matrices/](./docs/matrices/)：プラットフォームマトリクス、決定台帳、互換性と機能同等性のマトリクス
- [docs/guides/project-status.md](./docs/guides/project-status.md)：プロジェクト状態スナップショット

## 貢献

Issue と PR を歓迎します。貢献の手順と、すべての PR が通すべきチェックリストは
[CONTRIBUTING.md](./CONTRIBUTING.md) にあります。提出した時点で BSD-2-Clause での許諾に
同意したものとみなします。本プロジェクトは FreeBSD 中国語コミュニティが維持しており、
BSD 移植、中国語ローカライズ、端末での使い勝手についての議論を歓迎します。

## セキュリティ

脆弱性は公開 Issue で報告しないでください。報告窓口とセキュリティ不変条件は
[SECURITY.md](./SECURITY.md) にあります。

## ライセンス

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD 中国語コミュニティ（FreeBSD Chinese Community）。
