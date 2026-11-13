# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![প্ল্যাটফর্ম যাচাই](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | বাংলা

Rust-এ লেখা, লোকাল-ফার্স্ট LLM কোডিং হার্নেস। ডেটা একটিমাত্র DuckDB ফাইলে থাকে, প্রতিটি
কাজের সূত্র পাওয়া যায়, আর স্পেক-চালিত ডেভেলপমেন্টের গেট মানুষের শৃঙ্খলার বদলে প্রোগ্রাম
চালায়। নেটিভ GUI, TUI, CUI, Web ও SSH একই অ্যাপ্লিকেশন সেবার ওপর চলে।

- **লোকাল আগে**: একটিমাত্র DuckDB ফাইল, যা আপনি যখন খুশি সঙ্গে নিতে পারেন। কোনো অ্যাকাউন্ট
  নেই, জোর করে নেটওয়ার্ক সংযোগও নেই।
- **সূত্রসহ**: প্রতিটি টুল কলে উদ্দেশ্য, সম্পাদন ও নিবন্ধন আলাদা করে লেখা হয়। ডিফল্ট ট্রেস
  স্তর `redacted`, তাই ক্রেডেনশিয়াল লগেও যায় না, ইভেন্টেও নয়।
- **একাধিক ফ্রন্টএন্ড**: GUI, TUI, CUI, Web ও SSH একই সেবার ওপর দাঁড়িয়ে আছে। ডেস্কটপ
  ইন্টারফেস নেটিভভাবে আঁকা হয়, ভেতরে কোনো ব্রাউজার ইঞ্জিন বসানো হয় না।
- **স্পেক দিয়ে পরিচালিত**: চাহিদা, কাজ ও গ্রহণযোগ্যতার মাপকাঠি তিনটি পরস্পর যুক্ত থাকে, আর
  পরীক্ষা ও স্ক্রিপ্ট সেই যোগসূত্র যাচাই করে।

## বর্তমান অবস্থা

সংস্করণ `0.1.0`, এখনো প্রকাশিত হয়নি। পরিবর্তন দেখুন [CHANGELOG.md](./CHANGELOG.md)।

M0 থেকে M6, M8 এবং চার ফ্রন্টএন্ডের সমতা সম্পন্ন, M7-এর SSH সার্ভারও চলে। M9-এর ৫.৫ ঘণ্টার
স্থিতিশীলতা পরীক্ষায় এখনো যোগ্য তথ্য নেই, আর সত্যিকারের LLM সরবরাহকারীর ক্লায়েন্ট সংযোগ দেওয়া
হয়নি। ধাপে ধাপে অগ্রগতি, অসম্পূর্ণ কাজের কারণ ও প্রমাণ কোথায় আছে, তা
[প্রকল্প অবস্থার স্ন্যাপশট](./docs/guides/project-status.md)-এ আছে। প্রামাণিক খাতা
[tasks.md](./specs/001-whcode-harness/tasks.md) ও
[checklist.md](./specs/001-whcode-harness/checklist.md); জানা সীমাবদ্ধতা
[compatibility.md](./docs/matrices/compatibility.md)-এ।

## প্ল্যাটফর্ম সমর্থন

| প্ল্যাটফর্ম | ভূমিকা |
|---|---|
| Windows 11 amd64 | **গ্রহণের একমাত্র লক্ষ্য** (`x86_64-pc-windows-msvc`, ADR-013) |
| Windows ARM64 | বিল্ড সরবরাহের লক্ষ্য (ADR-043): CI-র ফল প্ল্যাটফর্মের প্রমাণ, গ্রহণ নয় |
| FreeBSD | সম্পূর্ণ প্ল্যাটফর্ম-প্রমাণ চ্যানেল (কঠোর গেট): প্রতিবার `main`-এ CI পুরো বিল্ড, পরীক্ষা ও গেট চালায়, যেকোনো ব্যর্থতায় তা লাল হয়; গ্রহণের সিদ্ধান্ত নয় |
| NetBSD | `main`-এ CI নিজে থেকেই চলে; কেবল প্ল্যাটফর্মের প্রমাণ |
| OpenBSD / DragonFly | হাতে রাখা স্থান; NetBSD-র সঙ্গে এরা SSH রিমোট ডেভেলপমেন্টের লক্ষ্য (যে দিকে সংযোগ করা হয়, নেটিভ টুলচেইন লাগে না) |

কর্মক্ষমতার পার্সেন্টাইল এখন কেবল Windows চ্যানেল থেকে আসে। সম্পূর্ণ নীতি
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) ও
[compatibility.md](./docs/matrices/compatibility.md)-এ।

## বিল্ড ও চালানো

Rust 1.100.0 (`rust-toolchain.toml`-এ আটকানো), MSVC টুলচেইন ও Python 3 দরকার। GNU টুলচেইন
কোথাও ব্যবহার হয় না।

```powershell
# প্রথম বিল্ডের আগে DuckDB শেয়ার্ড লাইব্রেরি নামিয়ে যাচাই করুন (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# পরিবেশের স্ব-পরীক্ষা
cargo run -p whcode-bin --release -- doctor

# ইন্টারফেস: Windows-এ ডিফল্ট নেটিভ GUI; --tui টার্মিনাল; --cli কমান্ড লাইন
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <সেশন ID>

# Web (ডিফল্ট HTTPS ও স্ব-স্বাক্ষরিত সার্টিফিকেট; --tls off হলে সাধারণ HTTP)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

কোনো আর্গুমেন্ট ছাড়া চালালে Windows-এ নেটিভ GUI খোলে। `--tui` টার্মিনাল ইন্টারফেস (`1` থেকে
`6` পর্যন্ত ভিউ বদলায়, `q` বন্ধ করে), `--cli` কমান্ড লাইনের জন্য। ছদ্ম টার্মিনালে (Git Bash,
mintty) বা কনসোল QuickEdit নির্বাচন মোডে থাকলে TUI কিবোর্ড ইনপুট পায় না; এটি Windows
Terminal বা conhost-এ চালান। কী বাইন্ডিং ও ভিউয়ের বর্ণনা [tui.md](./docs/guides/tui.md)-এ।

কনসোল না থাকলে (CI, পাইপ, রিডাইরেক্ট) প্রোগ্রাম কোনো ইন্টারফেস খোলে না: একটি ইঙ্গিত ছেপে
কোড 0 দিয়ে শেষ হয়।

## ইন্টারফেসের প্রবেশপথ

| প্রবেশ | চালানোর নিয়ম | বিবরণ |
|---|---|---|
| GUI | `whcode` (Windows-এ ডিফল্ট) বা `whcode gui` | নেটিভ, কেবল পঠনযোগ্য; কোনো ব্রাউজার ইঞ্জিন নেই |
| TUI | `whcode --tui` | ছয়টি কেবল-পঠনযোগ্য ভিউ; লেখার কাজে দুই ধাপের নিশ্চিতকরণ। দেখুন [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <উপ-কমান্ড>` বা `whcode <উপ-কমান্ড>` | একই `--json` খাম ও স্থির এক्ज़িট কোড। দেখুন [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | ডিফল্ট কেবল লুপব্যাক, ইনবাউন্ড TLS ডিফল্টে চালু। দেখুন [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | সীমিত প্রবেশ; ডিফল্টে সিস্টেম শেল দেয় না। চার প্ল্যাটফর্মে মাঠপর্যায়ের গ্রহণ বাকি |

## নথি

- [docs/guides/README.md](./docs/guides/README.md): সব গাইডের সূচি (বিল্ড, TUI, CUI, Web,
  সমবর্তিতা ও কর্মক্ষমতা, সম্পদ বাজেট, ইঞ্জিনিয়ারিং প্রক্রিয়া)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): চাহিদার ভিত্তি, কাজ, গ্রহণ তালিকা
  ও ইন্টারফেস চুক্তি
- [docs/adr/](./docs/adr/): স্থাপত্য সিদ্ধান্তের নথি
- [docs/matrices/](./docs/matrices/): প্ল্যাটফর্ম ম্যাট্রিক্স, সিদ্ধান্তের খাতা, সামঞ্জস্য ও
  সক্ষমতার সমতার ম্যাট্রিক্স
- [docs/guides/project-status.md](./docs/guides/project-status.md): প্রকল্প অবস্থার স্ন্যাপশট

## অংশগ্রহণ

Issue ও pull request স্বাগত। অবদানের প্রক্রিয়া এবং প্রতিটি pull request-এ যে তালিকা পূরণ করতে
হয়, তা [CONTRIBUTING.md](./CONTRIBUTING.md)-এ আছে। জমা দেওয়ার অর্থ আপনার কাজ BSD-2-Clause-এ
লাইসেন্স দিতে সম্মত হওয়া। FreeBSD চীনা সম্প্রদায় প্রকল্পটি চালায়; BSD পোর্ট, চীনা স্থানীয়করণ
ও টার্মিনাল অভিজ্ঞতা নিয়ে আলোচনা স্বাগত।

## নিরাপত্তা

নিরাপত্তা দুর্বলতা প্রকাশ্য issue-তে জানাবেন না। জানানোর পথ ও নিরাপত্তার অপরিবর্তনীয় শর্ত
[SECURITY.md](./SECURITY.md)-এ আছে।

## লাইসেন্স

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
