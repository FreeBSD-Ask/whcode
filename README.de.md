# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![Plattformprüfung](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | Deutsch | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Ein lokaler Coding-Harness für LLM, nativ in Rust. Die Daten liegen in einer einzigen
DuckDB-Datei, jede Aktion bleibt nachvollziehbar, und die Prüfpunkte der
spezifikationsgetriebenen Entwicklung werden von Programmen durchgesetzt, nicht von der
Disziplin der Beteiligten. Native GUI, TUI, CUI, Web und SSH nutzen denselben Anwendungsdienst.

- **Lokal zuerst**: eine DuckDB-Datei, die Sie jederzeit mitnehmen können. Kein Konto, keine
  erzwungene Netzverbindung.
- **Nachvollziehbar**: Jeder Werkzeugaufruf wird als Absicht, Ausführung und Eintrag
  festgehalten. Die Standard-Ablaufverfolgung ist `redacted`, damit Zugangsdaten weder in
  Protokolle noch in Ereignisse gelangen.
- **Mehrere Oberflächen**: GUI, TUI, CUI, Web und SSH liegen auf demselben Dienst. Die
  Desktop-Oberfläche zeichnet nativ und bettet keine Browser-Engine ein.
- **Spezifikationsgetrieben**: Anforderungen, Aufgaben und Abnahmekriterien sind über eine
  dreiseitige Rückverfolgung verknüpft, und Tests wie Skripte prüfen diese Verknüpfung.

## Aktueller Stand

Version `0.1.0`, noch nicht veröffentlicht. Änderungen stehen in [CHANGELOG.md](./CHANGELOG.md).

M0 bis M6, M8 und die Gleichheit der vier Oberflächen sind umgesetzt, der SSH-Server aus M7
läuft ebenfalls. Für den 5,5-Stunden-Stabilitätslauf aus M9 fehlen noch belastbare Daten, und
die Client-Anbindung an einen echten LLM-Anbieter fehlt noch. Fortschritt je Punkt,
Gründe für Offenes und Fundstellen der Belege stehen in der
[Projektstand-Momentaufnahme](./docs/guides/project-status.md). Maßgeblich sind
[tasks.md](./specs/001-whcode-harness/tasks.md) und
[checklist.md](./specs/001-whcode-harness/checklist.md); bekannte Einschränkungen stehen in
[compatibility.md](./docs/matrices/compatibility.md).

## Plattformunterstützung

| Plattform | Rolle |
|---|---|
| Windows 11 amd64 | **Einziges Abnahmeziel** (`x86_64-pc-windows-gnullvm`, ADR-073) |
| Windows ARM64 | Ziel für Build-Auslieferung (ADR-043): CI-Ergebnisse gelten als Plattformbeleg, nicht als Abnahme |
| FreeBSD | Vollständiger Plattformbeleg-Kanal (harte Prüfung): Bei jedem `main` baut, testet und prüft die CI vollständig, jeder Fehler färbt sie rot; keine Abnahmeschlussfolgerung |
| NetBSD | Läuft auf `main` automatisch in der CI; nur Plattformbeleg |
| OpenBSD / DragonFly | Manueller Platzhalter; zusammen mit NetBSD Ziel für SSH-Fernentwicklung (die verbundene Seite, keine native Werkzeugkette nötig) |

Leistungsquantile stammen derzeit nur aus dem Windows-Kanal. Die vollständige Plattformpolitik
steht in [ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) und
[compatibility.md](./docs/matrices/compatibility.md).

## Bauen und starten

Nötig sind Rust 1.100.0 (in `rust-toolchain.toml` festgelegt), die LLVM-Werkzeugkette (rustc gnullvm + llvm-mingw, installiert über `python .github/scripts/setup-windows-llvm.py`) und
Python 3. Eine GNU-Werkzeugkette wird nirgends verwendet.

```powershell
# Vor dem ersten Bau die DuckDB-Bibliothek holen und prüfen (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# Selbstprüfung der Umgebung
cargo run -p whcode-bin --release -- doctor

# Oberflächen: unter Windows standardmäßig die native GUI; --tui für das Terminal; --cli für die Kommandozeile
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <Sitzungs-ID>

# Web (standardmäßig HTTPS mit selbstsigniertem Zertifikat; --tls off schaltet auf Klartext zurück)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

Ohne Argumente öffnet WHCode unter Windows die native GUI. `--tui` startet die Terminal-Oberfläche
(`1` bis `6` wechseln die Ansicht, `q` beendet), `--cli` die Kommandozeile. Die TUI erhält keine
Tastendrücke in einem Pseudo-Terminal (Git Bash, mintty) und nicht, solange die Konsole im
QuickEdit-Auswahlmodus steht: starten Sie sie in Windows Terminal oder conhost. Tastenbelegung
und Ansichten stehen in [tui.md](./docs/guides/tui.md).

Gibt es keine Konsole (CI, Pipe, Umleitung), öffnet das Programm keine Oberfläche: Es gibt einen
Hinweis aus und endet mit Rückgabecode 0.

## Einstiegspunkte

| Einstieg | Aufruf | Hinweis |
|---|---|---|
| GUI | `whcode` (Standard unter Windows) oder `whcode gui` | Native Oberfläche nur zum Lesen, ohne Browser-Engine |
| TUI | `whcode --tui` | Sechs Ansichten nur zum Lesen; Schreibvorgänge brauchen eine Bestätigung in zwei Schritten. Siehe [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <Unterbefehl>` oder `whcode <Unterbefehl>` | Einheitlicher `--json`-Umschlag und stabile Rückgabecodes. Siehe [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | Standardmäßig nur Loopback, eingehendes TLS standardmäßig aktiv. Siehe [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | Beschränkter Einstieg; standardmäßig keine System-Shell. Die Abnahme vor Ort auf vier Plattformen steht aus |

## Dokumentation

- [docs/guides/README.md](./docs/guides/README.md): Verzeichnis aller Leitfäden (Bauen, TUI, CUI,
  Web, Nebenläufigkeit und Leistung, Ressourcenbudget, Entwicklungsprozess)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): Anforderungsbasis, Aufgaben,
  Abnahmeliste und Schnittstellenverträge
- [docs/adr/](./docs/adr/): Architekturentscheidungen
- [docs/matrices/](./docs/matrices/): Plattformmatrix, Entscheidungsregister, Matrizen für
  Kompatibilität und Funktionsgleichheit
- [docs/guides/project-status.md](./docs/guides/project-status.md): Momentaufnahme des Projektstands

## Mitmachen

Issues und Pull Requests sind willkommen. Ablauf und die Liste, die jeder Pull Request bestehen
muss, stehen in [CONTRIBUTING.md](./CONTRIBUTING.md). Mit dem Einreichen stimmen Sie einer
Lizenzierung unter BSD-2-Clause zu. Das Projekt wird von der chinesischsprachigen
FreeBSD-Community gepflegt; Diskussionen zu BSD-Ports, chinesischer Lokalisierung und
Terminal-Ergonomie sind willkommen.

## Sicherheit

Melden Sie Sicherheitslücken nicht über öffentliche Issues. Meldewege und Sicherheitsinvarianten
stehen in [SECURITY.md](./SECURITY.md).

## Lizenz

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
