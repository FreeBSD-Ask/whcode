# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![پلیٹ فارم کی تصدیق](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | [हिन्दी](README.hi.md) | اردو | [বাংলা](README.bn.md)

Rust میں لکھا گیا، لوکل LLM کوڈنگ ہارنیس۔ ڈیٹا ایک ہی DuckDB فائل میں رہتا ہے، ہر
کارروائی کا سراغ ملتا ہے، اور اسپیک سے چلنے والی ڈویلپمنٹ کے گیٹ لوگوں کے نظم کی بجائے
پروگرام چلاتے ہیں۔ نیٹو GUI، TUI، CUI، Web اور SSH ایک ہی ایپلی کیشن سروس پر چلتے ہیں۔

- **لوکل پہلے**: ایک DuckDB فائل، جسے آپ جب چاہیں ساتھ لے جا سکتے ہیں۔ نہ اکاؤنٹ، نہ لازمی
  نیٹ ورک کنکشن۔
- **قابلِ سراغ**: ہر ٹول کال ارادہ، عمل اور اندراج کے طور پر درج ہوتی ہے۔ طے شدہ ٹریس سطح
  `redacted` ہے، اس لیے اسناد نہ لاگ میں جاتی ہیں نہ ایونٹ میں۔
- **کئی فرنٹ اینڈ**: GUI، TUI، CUI، Web اور SSH ایک ہی سروس پر کھڑے ہیں۔ ڈیسک ٹاپ انٹرفیس
  نیٹو طور پر بنتا ہے اور اس میں کوئی براؤزر انجن نہیں ڈالا جاتا۔
- **اسپیک سے ہدایت**: ضروریات، کام اور قبولیت کے معیار تینوں کے آپس میں جڑے رہتے ہیں، اور ٹیسٹ
  و اسکرپٹ اسی جڑاؤ کی جانچ کرتے ہیں۔

## موجودہ حالت

ورژن `0.1.0`، ابھی جاری نہیں ہوا۔ تبدیلیاں [CHANGELOG.md](./CHANGELOG.md) میں ہیں۔

M0 سے M6، M8 اور چاروں فرنٹ اینڈ کی برابری مکمل ہے، اور M7 کا SSH سرور بھی چلتا ہے۔ M9 کا
5.5 گھنٹے کا استحکام ٹیسٹ ابھی قابلِ قبول ڈیٹا نہیں دے سکا، اور اصل LLM فراہم کنندہ کا کلائنٹ
رابطہ نہیں دیا گیا۔ شق بہ شق پیش رفت، ادھورے کام کی وجوہات اور ثبوت کہاں ہیں، یہ
[منصوبے کی حالت کا اسنیپ شاٹ](./docs/guides/project-status.md) میں ہے۔ مستند بہیاں
[tasks.md](./specs/001-whcode-harness/tasks.md) اور
[checklist.md](./specs/001-whcode-harness/checklist.md) ہیں؛ معلوم حدود
[compatibility.md](./docs/matrices/compatibility.md) میں ہیں۔

## پلیٹ فارم معاونت

| پلیٹ فارم | کردار |
|---|---|
| Windows 11 amd64 | **قبولیت کا واحد ہدف** (`x86_64-pc-windows-gnullvm`، ADR-073) |
| Windows ARM64 | بلڈ ترسیل کا ہدف (ADR-043): CI کے نتائج پلیٹ فارم ثبوت ہیں، قبولیت نہیں |
| FreeBSD | مکمل پلیٹ فارم ثبوت چینل (سخت گیٹ): ہر `main` پر CI پورا بلڈ، ٹیسٹ اور گیٹ چلاتی ہے، کوئی بھی ناکامی اسے سرخ کر دیتی ہے؛ قبولیت کا نتیجہ نہیں |
| NetBSD | `main` پر CI خودکار چلتی ہے؛ صرف پلیٹ فارم ثبوت |
| OpenBSD / DragonFly | محض دستی جگہ؛ NetBSD کے ساتھ یہ SSH ریموٹ ڈویلپمنٹ کے ہدف ہیں (جس سے رابطہ کیا جاتا ہے، نیٹو ٹول چین لازم نہیں) |

کارکردگی کے صدک ابھی صرف Windows چینل سے آتے ہیں۔ مکمل پالیسی
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) اور
[compatibility.md](./docs/matrices/compatibility.md) میں ہے۔

## بلڈ اور چلانا

Rust 1.100.0 (`rust-toolchain.toml` میں مقفل)، LLVM سلسلة کا ٹول چین (rustc gnullvm + llvm-mingw، جو `python .github/scripts/setup-windows-llvm.py` سے نصاب ہوتا ہے) اور Python 3 درکار ہیں۔ GNU
ٹول چین کہیں استعمال نہیں ہوتا۔

```powershell
# پہلے بلڈ سے پہلے DuckDB شیئرڈ لائبریری حاصل کریں اور جانچیں (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# ماحول کی خود جانچ
cargo run -p whcode-bin --release -- doctor

# انٹرفیس: Windows پر طے شدہ نیٹو GUI؛ --tui ٹرمینل؛ --cli کمانڈ لائن
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <سیشن ID>

# Web (طے شدہ HTTPS اور خود دستخط شدہ سند؛ --tls off سے سادہ HTTP)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

بغیر دلیل چلانے پر Windows میں نیٹو GUI کھلتا ہے۔ `--tui` ٹرمینل انٹرفیس (`1` سے `6` تک ویو
بدلیں، `q` سے باہر)، `--cli` کمانڈ لائن کے لیے ہے۔ نقلی ٹرمینل (Git Bash، mintty) میں یا
کنسول کے QuickEdit انتخابی موڈ میں TUI کو کی بورڈ ان پٹ نہیں ملتا؛ اسے Windows Terminal یا
conhost میں چلائیں۔ کلیدیں اور ویو [tui.md](./docs/guides/tui.md) میں ہیں۔

کنسول دستیاب نہ ہو (CI، پائپ، ری ڈائریکشن) تو پروگرام کوئی انٹرفیس نہیں کھولتا: اشارہ چھاپتا
ہے اور کوڈ 0 کے ساتھ ختم ہوتا ہے۔

## انٹرفیس کے دروازے

| دروازہ | چلانے کا طریقہ | تفصیل |
|---|---|---|
| GUI | `whcode` (Windows پر طے شدہ) یا `whcode gui` | نیٹو، صرف پڑھنے کے لیے؛ کوئی براؤزر انجن نہیں |
| TUI | `whcode --tui` | چھ ویو صرف پڑھنے کے لیے؛ لکھنے میں دو مرحلوں کی تصدیق۔ دیکھیں [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <ذیلی کمانڈ>` یا `whcode <ذیلی کمانڈ>` | ایک ہی `--json` لفافہ اور مستحکم اخراجی کوڈ۔ دیکھیں [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | طے شدہ صرف لوپ بیک، اندرونی TLS طے شدہ طور پر فعال۔ دیکھیں [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | محدود دروازہ؛ طے شدہ طور پر سسٹم شیل نہیں ملتا۔ چار پلیٹ فارم پر براہِ موقع قبولیت مؤخر ہے |

## دستاویزات

- [docs/guides/README.md](./docs/guides/README.md): تمام گائیڈوں کی فہرست (بلڈ، TUI، CUI، Web،
  ہم وقتی اور کارکردگی، وسائل کا بجٹ، انجینئرنگ عمل)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): ضروریات کی بنیاد، کام، قبولیت کی
  فہرست اور انٹرفیس معاہدے
- [docs/adr/](./docs/adr/): معماری فیصلوں کے ریکارڈ
- [docs/matrices/](./docs/matrices/): پلیٹ فارم میٹرکس، فیصلہ بہی، مطابقت اور صلاحیت کی برابری
  کے میٹرکس
- [docs/guides/project-status.md](./docs/guides/project-status.md): منصوبے کی حالت کا اسنیپ شاٹ

## شرکت

Issues اور pull requests خوش آمدید ہیں۔ شرکت کا طریقہ اور وہ فہرست جو ہر pull request کو پوری
کرنی ہوتی ہے، [CONTRIBUTING.md](./CONTRIBUTING.md) میں ہے۔ بھیجنے کا مطلب ہے کہ آپ اپنا کام
BSD-2-Clause کے تحت لائسنس دینے پر راضی ہیں۔ منصوبے کو FreeBSD چینی کمیونٹی چلاتی ہے؛ BSD پورٹ،
چینی مقامیت اور ٹرمینل تجربے پر بحث کا خیر مقدم ہے۔

## سلامتی

سلامتی کی کمزوریاں عوامی issue سے نہ بھیجیں۔ اطلاع کے راستے اور سلامتی کی لازمی شرطیں
[SECURITY.md](./SECURITY.md) میں ہیں۔

## لائسنس

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
