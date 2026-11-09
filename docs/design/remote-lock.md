# Remote lock (kill switch) — design

**Goal (owner's words):** "I can lock and unlock it remotely. That's all." No licence server, no hosting, no per-customer accounts — the same proven model as *Qurat Al-Mahalat* (`lib/remote/license_service.dart`), ported to C# for the Windows app.

## Two repositories

| Repo | Visibility | Holds |
|---|---|---|
| `moain2028/oracle-datafile-recovery` | private | **everything**: engine, app, `tools/odr_control.py`, `signing/activation_code.txt` (gitignored), signing keys |
| `moain2028/odr-apps` | public | `odr/license.json` (the switch), `odr/update.json`, installer in Releases, download page |

The public repo contains **no secret**: `codeSha256` is a hash; the installer is what customers download anyway.

## license.json

```json
{ "active": true, "codeSha256": "<sha256 upper-case code>", "message": "", "blockedDevices": [],
  "blockedMessage": "", "offlineGraceDays": 0, "contact": "+967770941666" }
```

| State | App behaviour |
|---|---|
| `active: true` | open; clears any earlier lock |
| `active: false` + hash | locked; asks for the code. Correct code is remembered while the hash is unchanged |
| hash changed | everyone who activated with the old code is locked again |
| device in `blockedDevices` | locked on that device only; code does not help; stays locked offline |
| file deleted (404) | permanent lock — **only** if this install has seen the file before (first-run safety) |
| offline / corrupt / 403 | **last known state. Never a false lock.** |
| `offlineGraceDays: N` | lock after N days without a successful check; reopens on contact |

Timing: check at start (non-blocking: cached state shows instantly), then on window activation at most every 10 min, and a 60 s timer calls the same throttled check. Transport: GitHub Contents API with a nonce (its edge caches 60 s), fallback `raw.githubusercontent`.

Device id = first 8 hex of SHA-256(`odr-device:` + `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`), shown as `ABCD-1234` on the lock screen with a copy button and a WhatsApp button that pre-fills it. It is never transmitted.

Brute force: after 5 wrong codes, 30 s wait doubling each failure (cap 2^8).

## Files (app/windows/Remote)
`RemoteSource.cs` (fetch, parse → Data/NotFound/Offline), `LicenseService.cs` (state table; pure `Apply()`), `Settings.cs` (`%LOCALAPPDATA%\ODR\settings.json`), `DeviceIdentity.cs`, `LockForm.cs` (RTL lock screen). `MainForm` disables itself and shows `LockForm` while `!Allowed`; the window and its state survive the lock.

## Proof
`ODR.exe --selftest-license` runs 28 checks of the state table with fake remote results (no network) — run in CI before signing. `--autotest` (14 UI checks) runs with the gate disabled so CI never depends on GitHub availability.

## Operating it
```bash
export GITHUB_TOKEN=...        # Contents: write on odr-apps ONLY (fine-grained PAT)
python3 tools/odr_control.py status | lock | unlock | set-code CODE | block-device ID | unblock-device ID | grace N | revoke
```
Or from the phone: edit `odr/license.json` on GitHub → `"active": false` → Commit.

CI (`app.yml`, tags `v*`) uploads the signed installer to `odr-apps` Releases and rewrites `odr/update.json` when the repo secret `ODR_APPS_TOKEN` exists.
