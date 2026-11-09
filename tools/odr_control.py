#!/usr/bin/env python3
"""odr_control.py — remote kill switch for the ODR Windows app (edits odr-apps/odr/license.json).

The PUBLIC repo moain2028/odr-apps holds only the switch; this PRIVATE repo holds the code.

    export GITHUB_TOKEN=ghp_...        # Contents: write on moain2028/odr-apps
    python3 tools/odr_control.py status
    python3 tools/odr_control.py lock --message "النسخة موقوفة، تواصل مع المطوّر"
    python3 tools/odr_control.py unlock
    python3 tools/odr_control.py set-code ODR-ABCD-EFGH-2345     # new code -> everyone on the old code is locked
    python3 tools/odr_control.py block-device 3F2A-91BC
    python3 tools/odr_control.py unblock-device 3F2A-91BC
    python3 tools/odr_control.py grace 14                        # lock after 14 days offline (0 = off)
    python3 tools/odr_control.py revoke                          # delete the file = permanent lock
    python3 tools/odr_control.py hash ODR-ABCD-EFGH-2345         # just print the sha256 (no network)

Only the SHA-256 of the code is published; the code itself lives in signing/activation_code.txt (gitignored).
"""
import argparse
import base64
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

OWNER, REPO, BRANCH, PATH = "moain2028", "odr-apps", "main", "odr/license.json"
API = f"https://api.github.com/repos/{OWNER}/{REPO}/contents/{PATH}"


def normalize_code(code: str) -> str:
    ar = "٠١٢٣٤٥٦٧٨٩"
    out = []
    for ch in code:
        i = ar.find(ch)
        out.append(str(i) if i >= 0 else ch)
    return "".join(out).replace(" ", "").replace("\t", "").upper()


def code_hash(code: str) -> str:
    return hashlib.sha256(normalize_code(code).encode()).hexdigest()


def normalize_device(d: str) -> str:
    return "".join(c for c in d.upper() if c in "0123456789ABCDEF")


def token() -> str:
    t = os.environ.get("GITHUB_TOKEN", "").strip()
    if not t:
        sys.exit("GITHUB_TOKEN is not set")
    return t


def gh(method: str, url: str, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={
        "Authorization": f"token {token()}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "odr-control",
        "Cache-Control": "no-cache",
    })
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def read():
    st, d = gh("GET", API + f"?ref={BRANCH}&t={datetime.now().timestamp()}")
    if st == 404:
        return None, None
    if st != 200:
        sys.exit(f"GET failed: {st} {d.get('message')}")
    content = base64.b64decode(d["content"]).decode()
    return json.loads(content), d["sha"]


def write(doc: dict, sha, msg: str):
    doc["updatedAt"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    body = {"message": msg, "branch": BRANCH,
            "content": base64.b64encode((json.dumps(doc, ensure_ascii=False, indent=2) + "\n").encode()).decode()}
    if sha:
        body["sha"] = sha
    st, d = gh("PUT", API, body)
    if st not in (200, 201):
        sys.exit(f"PUT failed: {st} {d.get('message')}")
    print("committed:", d["commit"]["sha"][:10], "-", msg)


DEFAULT = {"schema": 1, "product": "odr", "active": True, "codeSha256": "", "message": "",
           "blockedDevices": [], "blockedMessage": "", "offlineGraceDays": 0, "contact": "+967770941666"}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("status")
    l = sub.add_parser("lock"); l.add_argument("--message", default="هذه النسخة موقوفة من المطوّر. تواصل معنا للتفعيل.")
    sub.add_parser("unlock")
    s = sub.add_parser("set-code"); s.add_argument("code")
    b = sub.add_parser("block-device"); b.add_argument("device"); b.add_argument("--message", default="")
    u = sub.add_parser("unblock-device"); u.add_argument("device")
    g = sub.add_parser("grace"); g.add_argument("days", type=int)
    sub.add_parser("revoke")
    h = sub.add_parser("hash"); h.add_argument("code")
    a = p.parse_args()

    if a.cmd == "hash":
        print(code_hash(a.code)); return

    doc, sha = read()
    if a.cmd == "status":
        if doc is None:
            print("license.json: MISSING (installed copies that saw it before are permanently locked)"); return
        print(json.dumps(doc, ensure_ascii=False, indent=2)); return
    if a.cmd == "revoke":
        if doc is None:
            print("already missing"); return
        st, d = gh("DELETE", API, {"message": "odr: revoke (permanent lock)", "branch": BRANCH, "sha": sha})
        if st != 200:
            sys.exit(f"DELETE failed: {st} {d.get('message')}")
        print("deleted — permanent lock for every copy that has seen the file"); return

    doc = doc or dict(DEFAULT)
    if a.cmd == "lock":
        doc["active"] = False; doc["message"] = a.message; msg = "odr: lock"
    elif a.cmd == "unlock":
        doc["active"] = True; doc["message"] = ""; msg = "odr: unlock"
    elif a.cmd == "set-code":
        doc["codeSha256"] = code_hash(a.code); doc.pop("code", None); msg = "odr: rotate activation code"
        print("remember to update signing/activation_code.txt (private) with:", normalize_code(a.code))
    elif a.cmd == "block-device":
        dev = normalize_device(a.device)
        lst = [normalize_device(x) for x in doc.get("blockedDevices", [])]
        if dev not in lst:
            lst.append(dev)
        doc["blockedDevices"] = lst
        if a.message:
            doc["blockedMessage"] = a.message
        msg = f"odr: block device {dev}"
    elif a.cmd == "unblock-device":
        dev = normalize_device(a.device)
        doc["blockedDevices"] = [x for x in doc.get("blockedDevices", []) if normalize_device(x) != dev]
        msg = f"odr: unblock device {dev}"
    elif a.cmd == "grace":
        doc["offlineGraceDays"] = max(0, a.days); msg = f"odr: offline grace {max(0, a.days)} days"
    else:
        sys.exit("unknown command")
    write(doc, sha, msg)


if __name__ == "__main__":
    main()
