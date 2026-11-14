# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![플랫폼 검증](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | 한국어 | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Rust 네이티브, 로컬 우선 LLM 코딩 하네스입니다. 데이터는 DuckDB 파일 하나에 모이고 모든 동작을
추적할 수 있으며, 명세 주도 개발의 게이트는 사람의 규율이 아니라 프로그램이 집행합니다.
네이티브 GUI, TUI, CUI, Web, SSH 프런트엔드는 하나의 애플리케이션 서비스를 공유합니다.

- **로컬 우선**: DuckDB 단일 파일에 저장하므로 데이터를 언제든 가져갈 수 있습니다. 계정도,
  강제 네트워크 연결도 없습니다.
- **추적 가능**: 도구 호출은 '의도—실행—등록' 형태로 기록됩니다. 기본 추적 수준은
  `redacted`이며, 자격 증명은 로그와 이벤트에 남지 않습니다.
- **여러 프런트엔드**: GUI／TUI／CUI／Web／SSH가 같은 서비스를 씁니다. 데스크톱 화면은
  네이티브로 그리며 브라우저 엔진을 넣지 않습니다.
- **명세 주도**: 요구사항, 작업, 수용 기준을 삼자 간에 추적하고, 그 추적을 테스트와
  스크립트가 강제합니다.

## 현재 상태

버전 `0.1.0`, 아직 배포하지 않았습니다. 변경 내역은 [CHANGELOG.md](./CHANGELOG.md)에 있습니다.

M0—M6, M8과 네 프런트엔드 동등성이 구현되었고 M7의 SSH 서버도 동작합니다. M9의 5.5시간
안정성 시험은 아직 요건을 충족하는 데이터가 없고, 실제 LLM 공급자 클라이언트 연결은 제공되지
않았습니다. 항목별 진행 상황, 미완료 사유, 근거 위치는
[프로젝트 상태 스냅샷](./docs/guides/project-status.md)에 있습니다. 권위 있는 장부는
[tasks.md](./specs/001-whcode-harness/tasks.md)와
[checklist.md](./specs/001-whcode-harness/checklist.md)이며, 알려진 제한은
[compatibility.md](./docs/matrices/compatibility.md)에 있습니다.

## 플랫폼 지원

| 플랫폼 | 역할 |
|---|---|
| Windows 11 amd64 | **유일한 수용 대상** (`x86_64-pc-windows-gnullvm`, ADR-073) |
| Windows ARM64 | 빌드 배포 대상 (ADR-043). CI 결과는 플랫폼 증거일 뿐 수용에는 넣지 않습니다 |
| FreeBSD | 완전한 플랫폼 증거 채널(하드 게이트). `main`마다 CI가 전량 빌드·테스트·게이트를 돌리고 하나라도 실패하면 빨강이 됩니다. 수용 결론은 아닙니다 |
| NetBSD | `main`에서 CI가 자동 실행. 플랫폼 증거만 해당 |
| OpenBSD／DragonFly | 수동 자리만 확보. NetBSD와 함께 SSH 원격 개발 대상(연결을 받는 쪽이며 네이티브 툴체인은 요구하지 않음) |

성능 분위수는 현재 Windows 채널에서만 나옵니다. 플랫폼 방침은
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md)과
[compatibility.md](./docs/matrices/compatibility.md)를 보십시오.

## 빌드와 실행

Rust 1.100.0(`rust-toolchain.toml`에 고정), LLVM 계열 툴체인(rustc gnullvm + llvm-mingw, `python .github/scripts/setup-windows-llvm.py`로 설치), Python 3이 필요합니다. GNU
툴체인은 어디에도 쓰지 않습니다.

```powershell
# 첫 빌드 전에 DuckDB 공유 라이브러리를 받아 검증합니다 (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# 환경 자가 점검
cargo run -p whcode-bin --release -- doctor

# 화면: Windows는 기본이 네이티브 GUI, --tui는 터미널 UI, --cli는 명령줄
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <세션 ID>

# Web(기본은 HTTPS와 자체 서명 인증서, --tls off이면 평문)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

인자 없이 실행하면 Windows에서는 네이티브 GUI가 열립니다. `--tui`는 터미널 UI(`1`—`6`로
보기 전환, `q`로 종료), `--cli`는 명령줄입니다. TUI는 의사 터미널(Git Bash／mintty)이나
콘솔이 QuickEdit 선택 모드일 때 키 입력을 받지 못합니다. Windows Terminal이나 conhost에서
실행하십시오. 키 배치와 각 보기 설명은 [tui.md](./docs/guides/tui.md)에 있습니다.

콘솔을 쓸 수 없으면(CI, 파이프, 리다이렉션) 화면을 띄우지 않고 안내만 출력한 뒤 종료 코드
0으로 끝납니다.

## 화면 진입점

| 진입점 | 실행 방법 | 설명 |
|---|---|---|
| GUI | `whcode`(Windows 기본) 또는 `whcode gui` | 네이티브 읽기 전용 화면. 브라우저 엔진을 넣지 않음 |
| TUI | `whcode --tui` | 읽기 전용 보기 6개. 쓰기는 2단계 확인. 자세한 내용은 [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <하위 명령>` 또는 `whcode <하위 명령>` | `--json` 공통 봉투와 안정된 종료 코드. 자세한 내용은 [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | 기본은 루프백만, 수신 TLS 기본 활성화. 자세한 내용은 [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | 제한된 진입점. 기본으로 시스템 셸을 주지 않음. 네 플랫폼 현장 수용은 보류 |

## 문서

- [docs/guides/README.md](./docs/guides/README.md): 모든 안내서의 색인(빌드, TUI, CUI, Web,
  동시성과 성능, 자원 예산, 개발 프로세스)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): 요구사항 기준선, 작업, 수용
  체크리스트, 인터페이스 계약
- [docs/adr/](./docs/adr/): 아키텍처 결정 기록
- [docs/matrices/](./docs/matrices/): 플랫폼 매트릭스, 결정 장부, 호환성과 기능 동등성 매트릭스
- [docs/guides/project-status.md](./docs/guides/project-status.md): 프로젝트 상태 스냅샷

## 참여

이슈와 PR을 받습니다. 기여 절차와 모든 PR이 통과해야 하는 체크리스트는
[CONTRIBUTING.md](./CONTRIBUTING.md)에 있습니다. 제출하는 시점에 BSD-2-Clause로
라이선스로 제공하는 데 동의한 것으로 봅니다. 이 프로젝트는 FreeBSD 중국어 커뮤니티가 유지하며, BSD 포팅,
중국어 현지화, 터미널 사용성 논의를 환영합니다.

## 보안

취약점은 공개 이슈로 신고하지 마십시오. 신고 경로와 보안 불변 조건은
[SECURITY.md](./SECURITY.md)에 있습니다.

## 라이선스

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD 중국어 커뮤니티(FreeBSD Chinese Community).
