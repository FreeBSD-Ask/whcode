# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![vérification des plateformes](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | Français | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Harnais de codage local pour agents LLM, écrit en Rust. Les données tiennent dans un seul
fichier DuckDB, chaque action reste traçable, et les barrières du développement par
spécification sont appliquées par des programmes plutôt que par la discipline. L'interface
native, le TUI, le CUI, le Web et le SSH partagent un même service applicatif.

- **Local d'abord** : un seul fichier DuckDB, que vous emportez quand vous voulez. Aucun compte,
  aucune connexion réseau imposée.
- **Traçable** : chaque appel d'outil est enregistré selon le triptyque intention, exécution,
  inscription. Le niveau de trace par défaut est `redacted` : les secrets n'atteignent ni les
  journaux ni les événements.
- **Plusieurs interfaces** : GUI, TUI, CUI, Web et SSH s'appuient sur le même service. Le bureau
  est dessiné nativement, sans moteur de navigateur embarqué.
- **Piloté par la spécification** : exigences, tâches et critères d'acceptation sont reliés dans
  les trois sens, et des tests comme des scripts vérifient ce lien.

## État actuel

Version `0.1.0`, pas encore publiée. Les changements sont dans [CHANGELOG.md](./CHANGELOG.md).

M0 à M6, M8 et la parité des quatre interfaces sont livrés ; le serveur SSH de M7 fonctionne
également. L'essai de stabilité de 5,5 heures pour M9 n'a pas encore de données valables, et le
branchement du client vers un vrai fournisseur LLM n'est pas livré. Le détail de l'avancement,
les raisons des points ouverts et l'emplacement des preuves sont dans
[l'instantané de l'état du projet](./docs/guides/project-status.md). Les registres faisant
autorité sont [tasks.md](./specs/001-whcode-harness/tasks.md) et
[checklist.md](./specs/001-whcode-harness/checklist.md) ; les limites connues sont dans
[compatibility.md](./docs/matrices/compatibility.md).

## Plateformes prises en charge

| Plateforme | Rôle |
|---|---|
| Windows 11 amd64 | **Seule cible d'acceptation** (`x86_64-pc-windows-msvc`, ADR-013) |
| Windows ARM64 | Cible de livraison des compilations (ADR-043) : les résultats de la CI valent preuve de plateforme, pas acceptation |
| FreeBSD | Canal complet de preuve de plateforme (barrière stricte) : à chaque `main`, la CI compile, teste et applique les barrières en entier, et la moindre erreur la fait échouer ; aucune conclusion d'acceptation |
| NetBSD | Démarre automatiquement dans la CI sur `main` ; preuve de plateforme seulement |
| OpenBSD / DragonFly | Emplacement manuel ; avec NetBSD, servent de cible de développement distant par SSH (côté qui reçoit la connexion, aucun outillage natif requis) |

Les quantiles de performance proviennent pour l'instant du seul canal Windows. Voir
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) et
[compatibility.md](./docs/matrices/compatibility.md) pour la politique complète.

## Compilation et exécution

Il faut Rust 1.99.1 (épinglé dans `rust-toolchain.toml`), la chaîne MSVC et Python 3. Aucun
outillage GNU n'est utilisé.

```powershell
# Récupérer et vérifier la bibliothèque partagée DuckDB avant la première compilation (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# Auto-diagnostic de l'environnement
cargo run -p whcode-bin --release -- doctor

# Interfaces : interface native par défaut sous Windows ; --tui pour le terminal ; --cli pour la ligne de commande
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <identifiant de session>

# Web (HTTPS avec certificat auto-signé par défaut ; --tls off repasse en HTTP clair)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

Lancé sans argument sous Windows, WHCode ouvre l'interface native. Ajoutez `--tui` pour le
terminal (`1` à `6` changent de vue, `q` quitte) ou `--cli` pour la ligne de commande. Le TUI ne
reçoit aucune frappe dans un pseudo-terminal (Git Bash, mintty) ni tant que la console reste en
mode sélection QuickEdit : lancez-le dans Windows Terminal ou conhost. Les raccourcis et les
vues sont décrits dans [tui.md](./docs/guides/tui.md).

Sans console disponible (CI, tube, redirection), le programme n'ouvre aucune interface : il
affiche une indication et se termine avec le code 0.

## Points d'entrée

| Entrée | Lancement | Description |
|---|---|---|
| GUI | `whcode` (par défaut sous Windows) ou `whcode gui` | Interface native en lecture seule, sans moteur de navigateur |
| TUI | `whcode --tui` | Six vues en lecture seule ; les écritures demandent une confirmation en deux temps. Voir [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <sous-commande>` ou `whcode <sous-commande>` | Enveloppe `--json` unique et codes de sortie stables. Voir [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | Boucle locale uniquement par défaut, TLS entrant activé. Voir [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | Entrée restreinte ; aucun shell système par défaut. L'acceptation sur site des quatre plateformes est reportée |

## Documentation

- [docs/guides/README.md](./docs/guides/README.md) : index de tous les guides (compilation, TUI,
  CUI, Web, concurrence et performance, budget de ressources, processus d'ingénierie)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/) : base d'exigences, tâches, liste
  d'acceptation et contrats d'interface
- [docs/adr/](./docs/adr/) : décisions d'architecture
- [docs/matrices/](./docs/matrices/) : matrice des plateformes, registre des décisions, matrices
  de compatibilité et de parité des capacités
- [docs/guides/project-status.md](./docs/guides/project-status.md) : instantané de l'état du projet

## Contribuer

Les issues et les pull requests sont bienvenues. Le processus de contribution et la liste que
chaque pull request doit valider se trouvent dans [CONTRIBUTING.md](./CONTRIBUTING.md). En
soumettant, vous acceptez de placer votre travail sous BSD-2-Clause. Le projet est maintenu par
la communauté chinoise de FreeBSD ; les discussions sur les ports BSD, la localisation chinoise
et le confort du terminal sont bienvenues.

## Sécurité

Ne signalez pas de vulnérabilité par une issue publique. Les canaux de signalement et les
invariants de sécurité sont dans [SECURITY.md](./SECURITY.md).

## Licence

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
