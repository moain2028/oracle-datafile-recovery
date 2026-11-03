#!/usr/bin/env bash
# make_integrity.sh DIR VERSION — write DIR/integrity/ for a release archive or the evaluator bundle.
#
#   integrity/ODR-code-signing.cer   public certificate (DER) — the only key material shipped
#   integrity/SIGNER.txt             its SHA-1 thumbprint (what verify.* compare against)
#   integrity/VERSION.txt            version + commit + build time
#   integrity/verify.ps1, verify.sh  checkers (hashes, signed hash list, Authenticode on .exe)
#   integrity/trust-certificate.ps1  optional: trust the certificate on this Windows machine
#   integrity/README.txt             what all this is (English + Arabic)
#   integrity/SHA256SUMS             sha256 of EVERY other file under DIR (relative paths, LF)
#   integrity/SHA256SUMS.p7s         detached CMS signature of SHA256SUMS by the ODR key
#
# Signing key: env ODR_SIGN_PFX_B64 (base64 PKCS#12) + ODR_SIGN_PASS. The private key only
# ever exists in a mktemp directory that is removed on exit. Runs on Linux and on the
# Windows runner's Git Bash (sha256sum, openssl, find, sort are all there).
set -euo pipefail
DIR="${1:?usage: make_integrity.sh DIR VERSION}"; VERSION="${2:?usage: make_integrity.sh DIR VERSION}"
SRC="$(cd "$(dirname "$0")/.." && pwd)/docs/field/integrity"
: "${ODR_SIGN_PFX_B64:?ODR_SIGN_PFX_B64 is not set (repository secret)}"
: "${ODR_SIGN_PASS:?ODR_SIGN_PASS is not set (repository secret)}"

I="$DIR/integrity"; mkdir -p "$I"
cp "$SRC/ODR-code-signing.cer" "$SRC/SIGNER.txt" "$SRC/verify.ps1" "$SRC/verify.sh" \
   "$SRC/trust-certificate.ps1" "$SRC/README.txt" "$I/"
chmod +x "$I/verify.sh"
printf 'ODR %s\ncommit %s\nbuilt %s\n' "$VERSION" "${GITHUB_SHA:-$(git -C "$SRC" rev-parse HEAD 2>/dev/null || echo unknown)}" \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$I/VERSION.txt"

K="$(mktemp -d)"; trap 'rm -rf "$K"' EXIT
printf '%s' "$ODR_SIGN_PFX_B64" | base64 -d > "$K/s.pfx"
printf '%s' "$ODR_SIGN_PASS" > "$K/pass"
openssl pkcs12 -in "$K/s.pfx" -passin "file:$K/pass" -nokeys -clcerts -out "$K/c.pem" 2>/dev/null
openssl pkcs12 -in "$K/s.pfx" -passin "file:$K/pass" -nocerts -nodes -out "$K/k.pem" 2>/dev/null
# The secret and the shipped public certificate must be the same certificate.
want=$(grep -E '^[0-9A-F]{40}$' "$SRC/SIGNER.txt")
have=$(openssl x509 -in "$K/c.pem" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')
[ "$want" = "$have" ] || { echo "signing secret thumbprint $have != SIGNER.txt $want" >&2; exit 1; }

# Hash every file except the two outputs; sorted, './'-free relative paths, LF line ends.
( cd "$DIR" && find . -type f ! -path ./integrity/SHA256SUMS ! -path ./integrity/SHA256SUMS.p7s -print \
    | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r f; do sha256sum "$f"; done ) > "$K/sums"
sed 's/ \*/  /' "$K/sums" > "$I/SHA256SUMS"
openssl cms -sign -binary -md sha256 -nosmimecap -outform DER -signer "$K/c.pem" -inkey "$K/k.pem" \
  -in "$I/SHA256SUMS" -out "$I/SHA256SUMS.p7s"
# Self-check with the shipped verifier (hashes + list signature; no Authenticode on this host).
bash "$I/verify.sh" "$DIR"
echo "integrity: $(wc -l < "$I/SHA256SUMS") files hashed and signed (signer $have)"
