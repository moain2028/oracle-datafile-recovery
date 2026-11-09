# Windows app (v0.7) — structure

**Goal (owner):** organised, clear, every page explains itself (ℹ️), covers diagnose / triage / fingerprint, luxury RTL look, ready to try.

## Layout
```
app/windows/
├── Program.cs            entry: ODR.exe | --autotest DIR | --selftest-license
├── MainForm.cs           shell: navy sidebar (6 pages), status bar, licence gate, AutoTest driver
├── OdrTool.cs            runs odr.exe / odr-corrupt.exe, quoting, cancel, Arabic error explanations
├── Csv.cs                RFC-4180 reader for the result CSVs
├── Ui/
│   ├── Theme.cs          colours (navy #0E1E40, gold #D6A42E), fonts, button factories, BorderPanel, ScoreGauge
│   ├── Help.cs           the ℹ️ texts (Arabic) + the help dialog
│   ├── HomePage.cs       four doors + drag-drop a .dbf to diagnose
│   ├── RecoverPage.cs    the previous whole window: files → tables → extract → preview (unchanged logic)
│   ├── DiagnosePage.cs   `odr diagnose --json --lang ar`: gauge, verdict, facts, finding cards (cause/evidence/parameters/recovery/ORA), damage classes, regions, next steps; HTML/DB3F/JSON export
│   ├── DiagnoseModel.cs  tolerant parser of the diagnose JSON + Arabic titles for finding codes
│   ├── TriagePage.cs     `odr triage --json`: worst-first table, colour by band, CSV export, double-click → diagnose
│   ├── FingerprintPage.cs `odr dict --json` → `fingerprint learn`; `fingerprint match --json` → suggestions with confidence
│   └── SettingsPage.cs   out dir, device id (copy / WhatsApp), licence state + recheck, versions, links
├── Remote/               remote lock (see remote-lock.md)
├── tests/LicenseTests.cs 28 checks, `--selftest-license`
├── installer/odr.iss     Inno Setup (Arabic)
└── docs/README_AR.txt    shipped with the installer
```

## Principles
- The engine is `odr.exe`; the app never re-implements analysis. Every page is a thin, honest view of one CLI command's JSON.
- Arabic first, RTL everywhere; numbers stay Latin in monospace where they are identifiers (paths, hashes, commands).
- Nothing is hidden: every finding card shows the evidence numbers it rests on and the Oracle errors that would have appeared.
- Read-only promise repeated on Home and About; SHA-256 shown on the diagnose page with the "unchanged" mark.

## Proof (CI, `app.yml`)
`--selftest-license` (28) → sign → installer → silent install on the Windows runner → `--autotest`: 4 demo files, 18 tables, exact row counts, Arabic text, preview, **diagnose on a healthy demo file (score ≥ 90, 0 damaged, HEALTHY)**, screenshots of all six pages → uninstall → publish to `odr-apps`.
