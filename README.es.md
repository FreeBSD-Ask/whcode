# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![verificación de plataformas](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | Español | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Arnés de codificación con modelos LLM, nativo en Rust y local por diseño. Los datos caben en un
solo archivo DuckDB, cada acción queda registrada y las barreras del desarrollo por
especificación las aplican programas, no la disciplina del equipo. La interfaz nativa, la TUI,
la CUI, la Web y el SSH comparten un mismo servicio de aplicación.

- **Local primero**: un solo archivo DuckDB, y se lo lleva cuando quiera. Sin cuentas y sin
  conexión obligatoria.
- **Trazable**: cada llamada a una herramienta se anota como intención, ejecución y registro. El
  nivel de traza por defecto es `redacted`, así que las credenciales no llegan a los registros ni
  a los eventos.
- **Varias interfaces**: GUI, TUI, CUI, Web y SSH usan el mismo servicio. El escritorio se dibuja
  de forma nativa, sin motor de navegador incrustado.
- **Guiado por especificación**: requisitos, tareas y criterios de aceptación se enlazan en tres
  direcciones, y las pruebas y los scripts comprueban ese enlace.

## Estado actual

Versión `0.1.0`, aún sin publicar. Los cambios están en [CHANGELOG.md](./CHANGELOG.md).

M0—M6, M8 y la paridad de las cuatro interfaces están implementados, y el servidor SSH de M7
también funciona. La prueba de estabilidad de 5,5 horas de M9 todavía no tiene datos válidos, y
la conexión del cliente con un proveedor real de LLM no está entregada. El avance por punto, los
motivos de lo pendiente y la ubicación de las pruebas están en
[la instantánea del estado del proyecto](./docs/guides/project-status.md). Los registros que
mandan son [tasks.md](./specs/001-whcode-harness/tasks.md) y
[checklist.md](./specs/001-whcode-harness/checklist.md); las limitaciones conocidas están en
[compatibility.md](./docs/matrices/compatibility.md).

## Plataformas compatibles

| Plataforma | Papel |
|---|---|
| Windows 11 amd64 | **Único objetivo de aceptación** (`x86_64-pc-windows-msvc`, ADR-013) |
| Windows ARM64 | Objetivo de entrega de compilaciones (ADR-043): lo que produce la CI es prueba de plataforma, no aceptación |
| FreeBSD | Canal completo de prueba de plataforma (barrera estricta): en cada `main` la CI compila, prueba y aplica las barreras completas, y cualquier fallo la pone en rojo; no da conclusión de aceptación |
| NetBSD | La CI lo ejecuta sola en `main`; solo prueba de plataforma |
| OpenBSD / DragonFly | Marcador manual; junto con NetBSD sirven de destino para desarrollo remoto por SSH (el lado al que se conecta, sin cadena de herramientas nativa) |

Los percentiles de rendimiento salen hoy solo del canal de Windows. La política completa está en
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) y
[compatibility.md](./docs/matrices/compatibility.md).

## Compilar y ejecutar

Hacen falta Rust 1.99.1 (fijado en `rust-toolchain.toml`), la cadena MSVC y Python 3. No se usa
ninguna herramienta GNU.

```powershell
# Obtener y verificar la biblioteca compartida de DuckDB antes de la primera compilación (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# Autodiagnóstico del entorno
cargo run -p whcode-bin --release -- doctor

# Interfaces: en Windows la GUI nativa es lo predeterminado; --tui abre el terminal; --cli la línea de comandos
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <identificador de sesión>

# Web (HTTPS con certificado autofirmado por defecto; --tls off vuelve a HTTP sin cifrar)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

Al arrancar sin argumentos en Windows se abre la GUI nativa. Añada `--tui` para el terminal (`1`
a `6` cambian de vista, `q` sale) o `--cli` para la línea de comandos. La TUI no recibe teclas
dentro de un pseudoterminal (Git Bash, mintty) ni mientras la consola esté en modo de selección
QuickEdit: ejecútela en Windows Terminal o conhost. Las teclas y las vistas están en
[tui.md](./docs/guides/tui.md).

Cuando no hay consola disponible (CI, tuberías, redirección), el programa no abre ninguna
interfaz: muestra un aviso y termina con código 0.

## Puntos de entrada

| Entrada | Cómo se lanza | Notas |
|---|---|---|
| GUI | `whcode` (predeterminado en Windows) o `whcode gui` | Interfaz nativa de solo lectura, sin motor de navegador |
| TUI | `whcode --tui` | Seis vistas de solo lectura; escribir exige confirmación en dos pasos. Ver [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <subcomando>` o `whcode <subcomando>` | Un sobre `--json` común y códigos de salida estables. Ver [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | Solo bucle local por defecto, con TLS de entrada activado. Ver [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | Entrada restringida; no entrega un shell del sistema por defecto. La aceptación en sitio de las cuatro plataformas está pendiente |

## Documentación

- [docs/guides/README.md](./docs/guides/README.md): índice de todas las guías (compilación, TUI,
  CUI, Web, concurrencia y rendimiento, presupuesto de recursos, proceso de ingeniería)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): línea base de requisitos, tareas,
  lista de aceptación y contratos de interfaz
- [docs/adr/](./docs/adr/): registros de decisiones de arquitectura
- [docs/matrices/](./docs/matrices/): matriz de plataformas, registro de decisiones, matrices de
  compatibilidad y de paridad de capacidades
- [docs/guides/project-status.md](./docs/guides/project-status.md): instantánea del estado del proyecto

## Participar

Se aceptan issues y pull requests. El proceso de contribución y la lista que toda pull request
debe superar están en [CONTRIBUTING.md](./CONTRIBUTING.md). Al enviar cambios acepta licenciarlos
bajo BSD-2-Clause. El proyecto lo mantiene la comunidad china de FreeBSD; son bienvenidas las
discusiones sobre ports de BSD, localización al chino y comodidad del terminal.

## Seguridad

No informe vulnerabilidades por medio de un issue público. Los canales de reporte y los
invariantes de seguridad están en [SECURITY.md](./SECURITY.md).

## Licencia

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
