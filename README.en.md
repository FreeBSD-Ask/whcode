# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![platform verification](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | English | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

A Rust-native, local-first harness for LLM coding. Everything lands in a single DuckDB file,
every action is traceable, and the gates of spec-driven development are enforced by programs
rather than by discipline. The native GUI, TUI, CUI, Web and SSH front ends share one
application service.

- **Local first**: one DuckDB file holds the data, and you can take it with you. No account,
  no forced networking.
- **Traceable**: every tool call is recorded as intent, execution and registration. The
  default trace level is `redacted`, so credentials never reach logs or events.
- **Many front ends**: GUI, TUI, CUI, Web and SSH sit on the same service. The desktop
  interface renders natively and embeds no browser engine.
- **Spec driven**: requirements, tasks and acceptance criteria are traced in three directions,
  and tests and scripts enforce that trace.

## Status

Version `0.1.0`, not yet released. See [CHANGELOG.md](./CHANGELOG.md) for changes.

M0 to M6, M8 and four-front-end parity are implemented, and the M7 SSH server is in place.
The 5.5-hour stability run for M9 has no qualifying data yet, and the client wiring for a real
LLM provider is not delivered. Per-item progress, reasons for unfinished work and evidence
locations are in the [project status snapshot](./docs/guides/project-status.md). The
authoritative ledgers are [tasks.md](./specs/001-whcode-harness/tasks.md) and
[checklist.md](./specs/001-whcode-harness/checklist.md); known limitations are in
[compatibility.md](./docs/matrices/compatibility.md).

## Platform support

| Platform | Role |
|---|---|
| Windows 11 amd64 | **Only acceptance target** (`x86_64-pc-windows-msvc`, ADR-013) |
| Windows ARM64 | Build delivery target (ADR-043): CI results count as platform evidence, not acceptance |
| FreeBSD | Full platform-evidence channel (hard gate): CI builds, tests and gates everything on `main`, and any failure turns it red; no acceptance conclusion |
| NetBSD | Runs automatically on CI with `main`; platform evidence only |
| OpenBSD / DragonFly | Manual placeholder; together with NetBSD they are SSH remote-development targets (the machine being connected to, no native toolchain required) |

Performance percentiles currently come from the Windows channel only. See
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) and
[compatibility.md](./docs/matrices/compatibility.md) for the full platform policy.

## Build and run

You need Rust 1.99.1 (pinned in `rust-toolchain.toml`), the MSVC toolchain and Python 3. No
GNU toolchain is used anywhere.

```powershell
# Fetch and verify the DuckDB shared library once before the first build (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# Environment self-check
cargo run -p whcode-bin --release -- doctor

# Interfaces: native GUI by default on Windows; --tui for the terminal UI; --cli for the command line
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <session ID>

# Web (HTTPS with a self-signed certificate by default; --tls off falls back to plain HTTP)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

Started without arguments, WHCode opens the native GUI on Windows. Add `--tui` for the terminal
UI (`1` to `6` switch views, `q` quits) or `--cli` for the command line. The TUI receives no
keystrokes inside a pseudo-terminal (Git Bash, mintty) or while the console sits in QuickEdit
selection mode, so run it in Windows Terminal or conhost. Key bindings and views are documented
in [tui.md](./docs/guides/tui.md).

When no console is available (CI, pipes, redirection) the program starts no interface at all:
it prints a hint and exits with code 0.

## Interfaces

| Entry point | How to start | Notes |
|---|---|---|
| GUI | `whcode` (default on Windows) or `whcode gui` | Native, read-only; embeds no browser engine |
| TUI | `whcode --tui` | Six read-only views; writes need a two-step confirmation. See [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <subcommand>` or `whcode <subcommand>` | One `--json` envelope and stable exit codes. See [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | Loopback only by default, inbound TLS on by default. See [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | Restricted entry point; no system shell by default. On-site acceptance on four platforms is pending |

## Documentation

- [docs/guides/README.md](./docs/guides/README.md): index of all guides (build, TUI, CUI, Web,
  concurrency and performance, resource budget, engineering process)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): requirement baseline, tasks, acceptance
  checklist and interface contracts
- [docs/adr/](./docs/adr/): architecture decision records
- [docs/matrices/](./docs/matrices/): platform matrix, decision ledger, compatibility and
  capability-parity matrices
- [docs/guides/project-status.md](./docs/guides/project-status.md): project status snapshot

## Contributing

Issues and pull requests are welcome. The contribution flow and the checklist every pull
request must pass are in [CONTRIBUTING.md](./CONTRIBUTING.md). By submitting you agree to
license your work under BSD-2-Clause. The project is maintained by the FreeBSD Chinese
Community; discussion of BSD ports, Chinese localization and terminal ergonomics is welcome.

## Security

Please do not report security vulnerabilities through public issues. Reporting channels and
the security invariants are in [SECURITY.md](./SECURITY.md).

## License

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
