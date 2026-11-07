# WHCode

<p align="center">
  <img src="assets/brand/logo.svg" alt="WHCode" width="160">
</p>

[![प्लेटफ़ॉर्म सत्यापन](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml/badge.svg)](https://github.com/FreeBSD-Ask/whcode/actions/workflows/platform-verification.yml)

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | [English](README.en.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [Русский](README.ru.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md) | [العربية](README.ar.md) | हिन्दी | [اردو](README.ur.md) | [বাংলা](README.bn.md)

Rust में लिखा, लोकल-फ़र्स्ट LLM कोडिंग हार्नेस। डेटा एक ही DuckDB फ़ाइल में रहता है, हर क्रिया
का रिकॉर्ड रहता है, और स्पेक-ड्रिवन डेवलपमेंट के गेट लोगों के अनुशासन की जगह प्रोग्राम चलाते
हैं। नेटिव GUI, TUI, CUI, Web और SSH सभी एक ही एप्लिकेशन सेवा पर चलते हैं।

- **लोकल पहले**: एक DuckDB फ़ाइल, जिसे आप कभी भी साथ ले जा सकते हैं। न खाता, न ज़बरदस्ती नेटवर्क।
- **ट्रेस करने योग्य**: हर tool कॉल इरादा, निष्पादन और पंजीकरण के रूप में दर्ज होता है। डिफ़ॉल्ट
  ट्रेस स्तर `redacted` है, इसलिए क्रेडेंशियल न लॉग में जाते हैं न इवेंट में।
- **कई फ़्रंटएंड**: GUI, TUI, CUI, Web और SSH एक ही सेवा पर टिके हैं। डेस्कटॉप इंटरफ़ेस नेटिव
  रेंडर होता है और उसमें कोई ब्राउज़र इंजन नहीं घुसाया जाता।
- **स्पेक से संचालित**: ज़रूरतें, कार्य और स्वीकृति मानदंड तीन दिशाओं में जुड़े रहते हैं, और
  टेस्ट तथा स्क्रिप्ट उस जुड़ाव की जाँच करते हैं।

## मौजूदा स्थिति

संस्करण `0.1.0`, अभी जारी नहीं हुआ। बदलाव [CHANGELOG.md](./CHANGELOG.md) में हैं।

M0 से M6, M8 और चारों फ़्रंटएंड की समानता पूरी हो चुकी है, और M7 का SSH सर्वर भी चलता है। M9
का 5.5 घंटे वाला स्थिरता परीक्षण अभी योग्य डेटा नहीं दे पाया, और असली LLM प्रोवाइडर का क्लाइंट
कनेक्शन नहीं दिया गया। मद-दर-मद प्रगति, अधूरे काम के कारण और साक्ष्य कहाँ हैं, यह
[प्रोजेक्ट स्थिति स्नैपशॉट](./docs/guides/project-status.md) में है। प्रामाणिक बहियाँ
[tasks.md](./specs/001-whcode-harness/tasks.md) और
[checklist.md](./specs/001-whcode-harness/checklist.md) हैं; ज्ञात सीमाएँ
[compatibility.md](./docs/matrices/compatibility.md) में हैं।

## प्लेटफ़ॉर्म समर्थन

| प्लेटफ़ॉर्म | भूमिका |
|---|---|
| Windows 11 amd64 | **स्वीकृति का एकमात्र लक्ष्य** (`x86_64-pc-windows-msvc`, ADR-013) |
| Windows ARM64 | बिल्ड डिलीवरी लक्ष्य (ADR-043): CI के नतीजे प्लेटफ़ॉर्म साक्ष्य हैं, स्वीकृति नहीं |
| FreeBSD | पूरा प्लेटफ़ॉर्म-साक्ष्य चैनल (कड़ा गेट): हर `main` पर CI पूरा बिल्ड, टेस्ट और गेट चलाती है, कोई भी विफलता उसे लाल कर देती है; स्वीकृति निष्कर्ष नहीं |
| NetBSD | `main` पर CI अपने आप चलती है; केवल प्लेटफ़ॉर्म साक्ष्य |
| OpenBSD / DragonFly | हाथ से रखा गया स्थान; NetBSD के साथ ये SSH रिमोट डेवलपमेंट के लक्ष्य हैं (जिससे जुड़ा जाता है, नेटिव टूलचेन ज़रूरी नहीं) |

प्रदर्शन प्रतिशतक अभी केवल Windows चैनल से आते हैं। पूरी नीति
[ADR-013](./docs/adr/ADR-013-platform-scope-windows-only.md) और
[compatibility.md](./docs/matrices/compatibility.md) में है।

## बिल्ड और रन

Rust 1.99.1 (`rust-toolchain.toml` में तय), MSVC टूलचेन और Python 3 चाहिए। GNU टूलचेन कहीं
इस्तेमाल नहीं होता।

```powershell
# पहले बिल्ड से पहले DuckDB शेयर्ड लाइब्रेरी लाएँ और जाँचें (SHA256)
python .github/scripts/fetch-duckdb.py

cargo build --locked --workspace
cargo test --locked --workspace

# वातावरण का स्व-निरीक्षण
cargo run -p whcode-bin --release -- doctor

# इंटरफ़ेस: Windows पर डिफ़ॉल्ट नेटिव GUI; --tui टर्मिनल; --cli कमांड लाइन
cargo run -p whcode-bin --release --
cargo run -p whcode-bin --release -- --tui
cargo run -p whcode-bin --release -- --cli task list --session <session ID>

# Web (डिफ़ॉल्ट HTTPS और स्व-हस्ताक्षरित प्रमाणपत्र; --tls off से सादा HTTP)
cargo run -p whcode-bin --release -- serve --web
cargo run -p whcode-bin --release -- serve --web --tls off
```

बिना आर्ग्युमेंट चलाने पर Windows में नेटिव GUI खुलता है। `--tui` टर्मिनल इंटरफ़ेस (`1` से `6`
तक व्यू बदलें, `q` से बाहर), `--cli` कमांड लाइन के लिए है। छद्म टर्मिनल (Git Bash, mintty) में
या कंसोल के QuickEdit चयन मोड में TUI को कीबोर्ड इनपुट नहीं मिलता; इसे Windows Terminal या
conhost में चलाएँ। कुंजियाँ और व्यू [tui.md](./docs/guides/tui.md) में हैं।

कंसोल उपलब्ध न हो (CI, पाइप, रीडायरेक्ट) तो प्रोग्राम कोई इंटरफ़ेस नहीं खोलता: संकेत छापता है
और कोड 0 के साथ समाप्त होता है।

## इंटरफ़ेस प्रवेश

| प्रवेश | चलाने का तरीका | विवरण |
|---|---|---|
| GUI | `whcode` (Windows पर डिफ़ॉल्ट) या `whcode gui` | नेटिव, केवल पढ़ने के लिए; कोई ब्राउज़र इंजन नहीं |
| TUI | `whcode --tui` | छह व्यू केवल पढ़ने के लिए; लिखने में दो चरण की पुष्टि। देखें [tui.md](./docs/guides/tui.md) |
| CUI | `whcode --cli <उप-कमांड>` या `whcode <उप-कमांड>` | एक ही `--json` लिफ़ाफ़ा और स्थिर एक्ज़िट कोड। देखें [cui-manual.md](./docs/guides/cui-manual.md) |
| Web | `whcode serve --web` | डिफ़ॉल्ट केवल लूपबैक, इनबाउंड TLS डिफ़ॉल्ट रूप से चालू। देखें [web-deployment-security.md](./docs/guides/web-deployment-security.md) |
| SSH | `whcode serve --ssh` | सीमित प्रवेश; डिफ़ॉल्ट रूप से सिस्टम शेल नहीं मिलता। चार प्लेटफ़ॉर्म पर मौके की स्वीकृति बाकी है |

## दस्तावेज़

- [docs/guides/README.md](./docs/guides/README.md): सभी गाइडों की सूची (बिल्ड, TUI, CUI, Web,
  समवर्तीता और प्रदर्शन, संसाधन बजट, इंजीनियरिंग प्रक्रिया)
- [specs/001-whcode-harness/](./specs/001-whcode-harness/): आवश्यकता आधार, कार्य, स्वीकृति
  चेकलिस्ट और इंटरफ़ेस अनुबंध
- [docs/adr/](./docs/adr/): आर्किटेक्चर निर्णय रिकॉर्ड
- [docs/matrices/](./docs/matrices/): प्लेटफ़ॉर्म मैट्रिक्स, निर्णय बही, अनुकूलता और क्षमता
  समानता मैट्रिक्स
- [docs/guides/project-status.md](./docs/guides/project-status.md): प्रोजेक्ट स्थिति स्नैपशॉट

## योगदान

Issues और pull requests स्वागत हैं। योगदान प्रक्रिया और वह सूची जो हर pull request को पास
करनी होती है, [CONTRIBUTING.md](./CONTRIBUTING.md) में है। भेजने का अर्थ है कि आप अपना काम
BSD-2-Clause के तहत लाइसेंस देने पर सहमत हैं। प्रोजेक्ट FreeBSD चीनी समुदाय चलाता है; BSD पोर्ट,
चीनी स्थानीयकरण और टर्मिनल अनुभव पर चर्चा का स्वागत है।

## सुरक्षा

सुरक्षा कमज़ोरियाँ सार्वजनिक issue से न भेजें। रिपोर्ट के रास्ते और सुरक्षा अपरिवर्तक
[SECURITY.md](./SECURITY.md) में हैं।

## लाइसेंस

[BSD-2-Clause](./LICENSE) © 2026 FreeBSD Chinese Community.
