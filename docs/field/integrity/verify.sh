#!/usr/bin/env bash
# verify.sh [ROOT] — check an ODR folder against integrity/SHA256SUMS (Linux / macOS / Git Bash).
#   1. every file listed in SHA256SUMS exists and has that SHA-256;
#   2. SHA256SUMS.p7s is a valid signature of SHA256SUMS by the certificate in
#      ODR-code-signing.cer, whose thumbprint equals SIGNER.txt (needs openssl).
# Exit 0 = intact and signed by ODR; 1 = do not trust this folder.
set -u
I="$(cd "$(dirname "$0")" && pwd)"; ROOT="${1:-$(dirname "$I")}"
fail=0
if ( cd "$ROOT" && sha256sum -c --quiet --strict "$I/SHA256SUMS" ); then
  echo "  PASS  integrity: $(wc -l < "$I/SHA256SUMS") files match SHA256SUMS"
else echo "  FAIL  integrity: files differ from SHA256SUMS (listed above)"; fail=1; fi
want=$(grep -E '^[0-9A-F]{40}$' "$I/SIGNER.txt")
cert=$(openssl x509 -inform DER -in "$I/ODR-code-signing.cer" -noout -fingerprint -sha1 2>/dev/null | cut -d= -f2 | tr -d ':')
if ! command -v openssl >/dev/null 2>&1; then
  echo "  SKIP  signature of SHA256SUMS: openssl not installed (hashes above still checked)"
elif [ "$cert" != "$want" ]; then
  echo "  FAIL  ODR-code-signing.cer thumbprint $cert != SIGNER.txt $want"; fail=1
else
  t=$(mktemp -d); openssl x509 -inform DER -in "$I/ODR-code-signing.cer" -out "$t/ca.pem"
  if openssl cms -verify -binary -inform DER -in "$I/SHA256SUMS.p7s" -content "$I/SHA256SUMS" \
       -CAfile "$t/ca.pem" -purpose any -out /dev/null 2>"$t/err"; then
    echo "  PASS  SHA256SUMS signed by ODR certificate $want"
  else echo "  FAIL  SHA256SUMS signature invalid: $(head -1 "$t/err")"; fail=1; fi
  rm -rf "$t"
fi
exit $fail
