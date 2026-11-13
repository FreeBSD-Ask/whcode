# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![التحقق من المنصات](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | العربية | [हिन्दी](README.hi.md) | [اردو](README.ur.md) | [বাংলা](README.bn.md)

بيئة برمجة LLM محلية أولاً، مكتوبة بلغة Rust. تُحفظ البيانات في ملف DuckDB واحد، وكل إجراء
قابل للتتبع، وبوابات التطوير القائم على المواصفات تنفّذها برامج لا انضباط الأشخاص. تشترك واجهة
سطح المكتب الأصلية والواجهة النصية TUI وسطر الأوامر CUI والويب وSSH في خدمة تطبيق واحدة.

- **المحلية أولاً**: ملف DuckDB واحد، تأخذه معك متى شئت. لا حساب ولا اتصال شبكي إلزامي.
- **قابلية التتبع**: كل استدعاء أداة يُسجَّل على هيئة نية ثم تنفيذ ثم تسجيل. مستوى التتبع
  الافتراضي `redacted`، فلا تصل بيانات الاعتماد إلى السجلات ولا إلى الأحداث.
- **واجهات متعددة**: تعمل واجهات GUI وTUI وCUI والويب وSSH فوق الخدمة نفسها. تُرسم واجهة سطح
  المكتب أصلياً ولا تُضمّن محرك متصفح.
- **قائم على المواصفات**: تُربط المتطلبات والمهام ومعايير القبول بترابط ثلاثي، وتتحقق
  الاختبارات والسكربتات من هذا الربط.

## الحالة الراهنة

الإصدار `0.1.0`، ولم يُنشر بعد. التغييرات في [CHANGELOG.md](./CHANGELOG.md).

أُنجزت مراحل M0 حتى M6 وM8 وتكافؤ الواجهات الأربع، ويعمل خادم SSH الخاص بالمرحلة M7. لا توجد
بعد بيانات صالحة لاختبار الاستقرار البالغ 5.5 ساعة في M9، ولم يُسلَّم ربط العميل بمزوّد LLM
حقيقي. تجد التقدم بنداً بنداً وأسباب ما لم يكتمل ومواضع الأدلة في
[لقطة حالة المشروع](./docs/guides/project-status.md). السجلان المرجعيان هما
[tasks.md](./specs/001-whcode-harness/tasks.md) و
[checklist.md](./specs/001-whcode-harness/checklist.md)، والقيود المعروفة في
[compatibility.md](./docs/matrices/compatibility.md).

## المنصات المدعومة

| المنصة | الدور |
|---|---|
| Windows 11 amd64 | **هدف القبول الوحيد** (`x86_64-pc-windows-msvc`، ADR-013) |
| Windows ARM64 | هدف تسليم البناء (ADR-043): نتائج CI دليل على المنصة لا قبول |
| FreeBSD | قناة كاملة لأدلة المنصة (بوابة صارمة): مع كل `main` تبني CI وتختبر وتطبّق البوابات كاملة، وأي فشل يجعلها حمراء؛ دون استنتاج قبول |
| NetBSD | تعمل CI تلقائياً على `main`؛ دليل منصة فقط |
| OpenBSD / DragonFly | موضع يدوي؛ ومع NetBSD هدفان للتطوير البعيد عبر SSH (الطرف المتصل به، دون حاجة إلى سلسلة أدوات أصلية) |

تُؤخذ شرائح الأداء المئوية حالياً من قناة Windows وحدها. السياسة الكاملة في
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) و
[compatibility.md](./docs/matrices/compatibility.md).

## البناء والتشغيل

تحتاج إلى Rust 1.100.0 (مثبّت في `rust-toolchain.toml`) وسلسلة أدوات MSVC وPython 3. لا تُستخدم
أدوات GNU في أي موضع.

```powershell
# اجلب مكتبة DuckDB المشتركة وتحقق منها قبل البناء الأول (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# فحص ذاتي للبيئة
cargo run -p whcode-bin --release -- doctor

# الواجهات: في Windows تُفتح واجهة سطح المكتب افتراضياً؛ و`--tui` للواجهة النصية و`--cli` لسطر الأوامر
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <معرّف الجلسة>

# الويب (HTTPS بشهادة موقّعة ذاتياً افتراضياً؛ و`--tls off` يعود إلى نص صريح)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

عند التشغيل دون وسائط تفتح WHCode واجهة سطح المكتب في Windows. أضف `--tui` للواجهة النصية
(`1` إلى `6` لتبديل المشاهد، و`q` للخروج) أو `--cli` لسطر الأوامر. لا تتلقى الواجهة النصية
ضغطات المفاتيح داخل طرفية زائفة (Git Bash أو mintty) ولا ما دامت الطرفية في وضع التحديد
QuickEdit؛ شغّلها في Windows Terminal أو conhost. تجد المفاتيح والمشاهد في
[tui.md](./docs/guides/tui.md).

إذا لم تتوفر طرفية (CI أو أنبوب أو إعادة توجيه) فلا تفتح الواجهة: تُطبع ملاحظة وينتهي التنفيذ
بالرمز 0.

## مداخل الواجهات

| المدخل | أمر التشغيل | البيان |
|---|---|---|
| GUI | `whcode` (الافتراضي في Windows) أو `whcode gui` | واجهة أصلية للقراءة فقط، دون محرك متصفح |
| TUI | `whcode --tui` | ستة مشاهد للقراءة فقط، والكتابة تتطلب تأكيداً على مرحلتين. انظر [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <أمر فرعي>` أو `whcode <أمر فرعي>` | مظروف `--json` موحّد ورموز خروج ثابتة. انظر [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | الاستماع على العنوان الراجع فقط افتراضياً، وTLS الوارد مفعّل افتراضياً. انظر [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | مدخل مقيّد، ولا يُمنح صدفة النظام افتراضياً. القبول الميداني على المنصات الأربع مؤجّل |

## الوثائق

- [docs/guides/README.md](./docs/guides/README.md): فهرس جميع الأدلة (البناء، TUI، CUI، الويب،
  التزامن والأداء، ميزانية الموارد، عملية الهندسة)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): خط أساس المتطلبات والمهام وقائمة
  القبول وعقود الواجهات
- [docs/adr/](./docs/adr/): سجلات قرارات المعمارية
- [docs/matrices/](./docs/matrices/): مصفوفة المنصات وسجل القرارات ومصفوفتا التوافق وتكافؤ القدرات
- [docs/guides/project-status.md](./docs/guides/project-status.md): لقطة حالة المشروع

## المساهمة

نرحّب بالـ issues وطلبات الدمج. تجد إجراءات المساهمة والقائمة التي يجب أن يجتازها كل طلب دمج في
[CONTRIBUTING.md](./CONTRIBUTING.md). إرسالك يعني موافقتك على ترخيص عملك بموجب BSD-2-Clause.
يتولى المشروع مجتمع FreeBSD الناطق بالصينية، ونرحّب بالنقاش حول نقل BSD والتعريب الصيني
وتجربة الطرفية.

## الأمن

لا تبلّغ عن الثغرات عبر issues عامة. قنوات التبليغ وثوابت الأمن في
[SECURITY.md](./SECURITY.md).

## الترخيص

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
