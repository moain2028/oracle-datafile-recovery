#!/usr/bin/env bash
# try_odr.sh — ODR self-checking trial, Linux x86_64. Twin of try_odr.ps1.
# Downloads the tool + the test datafiles, verifies SHA-256, runs the quick-start
# commands and COMPARES every number with what Oracle itself says. PASS/FAIL per step.
# Never writes to the source datafiles.
#
#   bash try_odr.sh [DIR] [TAG]          # default DIR=./odr-trial, TAG=v0.2.0-alpha
#   ODR_TOOLDIR=/path/with/odr bash try_odr.sh DIR   # testing: use local binaries, skip tool download
#
# Needs: curl, tar, sha256sum, python3 (CSV checks), ~700 MB disk.
set -u
DIR="${1:-$PWD/odr-trial}"; TAG="${2:-v0.2.0-alpha}"; GOLDEN="golden-26ai-20261024"
REPO="https://github.com/moain2026/oracle-datafile-recovery"
TGZ="odr_golden_26ai_v2_20261024T085903Z.tar.gz"; TAR="odr-$TAG-x86_64-unknown-linux-gnu.tar.gz"
PASS=0; FAIL=0
step() { printf '\n== %s\n' "$1"; }
ok()   { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }
check(){ if [ "$1" = 0 ]; then ok "$2"; else bad "$2"; fi; }
get()  { [ -f "$2" ] && { echo "  (exists) $2"; return; }; echo "  downloading $1"; curl -sSL -o "$2" "$1"; }
sha()  { local exp act; exp=$(awk '{print tolower($1)}' "$2"); act=$(sha256sum "$1" | awk '{print $1}')
         if [ "$exp" = "$act" ]; then ok "sha256 $1 = $act"; else bad "sha256 $1 MISMATCH"; echo "stop: do not trust this download"; exit 1; fi; }
# run_extract OUT args... → sets V I R CODE
run_extract() { local out="$1"; shift; local txt
  txt=$("$ODR" extract "$@" --out "$out" 2>&1); CODE=$?
  read -r V I R < <(printf '%s\n' "$txt" | grep -o 'verified [0-9]* · inferred [0-9]* · rejected [0-9]*' | tail -1 | awk '{print $2,$5,$8}')
  : "${V:=-1}" "${I:=-1}" "${R:=-1}"; LAST="$txt"; }
expect() { # expect V I R CODE what
  if [ "$V" = "$1" ] && [ "$I" = "$2" ] && [ "$R" = "$3" ]; then ok "$5 → verified $V · inferred $I · rejected $R"; else bad "$5 → verified $V · inferred $I · rejected $R (expected $1 · $2 · $3)"; fi
  check "$([ "$CODE" = "$4" ]; echo $?)" "$5 exit code $CODE (expected $4)"; }

mkdir -p "$DIR" && cd "$DIR" || exit 1
echo "ODR trial in $DIR  (tool $TAG, datafiles $GOLDEN)"

step "1. Download and verify (SHA-256)"
if [ -z "${ODR_TOOLDIR:-}" ]; then
  get "$REPO/releases/download/$TAG/$TAR" "$TAR"; get "$REPO/releases/download/$TAG/$TAR.sha256" "$TAR.sha256"; sha "$TAR" "$TAR.sha256"
  [ -x "odr-$TAG-x86_64-unknown-linux-gnu/odr" ] || tar -xzf "$TAR"
  TOOLDIR="$PWD/odr-$TAG-x86_64-unknown-linux-gnu"
else TOOLDIR="$ODR_TOOLDIR"; fi
get "$REPO/releases/download/$GOLDEN/$TGZ" "$TGZ"; get "$REPO/releases/download/$GOLDEN/odr_golden_26ai_v2.sha256" odr_golden_26ai_v2.sha256
sha "$TGZ" odr_golden_26ai_v2.sha256
[ -f 20261024T085903Z/datafiles/odr_odr_assm01.dbf ] || { echo "  extracting $TGZ (≈ 480 MB)"; tar -xzf "$TGZ" || exit 1; }
ODR="$TOOLDIR/odr"; CORRUPT="$TOOLDIR/odr-corrupt"
G="$PWD/20261024T085903Z/datafiles"; S="$G/FREE_FREEPDB1_system01.dbf"; A="$G/odr_odr_assm01.dbf"
ver=$("$ODR" --version); check "$(printf '%s' "$ver" | grep -Eq '^odr [0-9]+\.[0-9]+\.[0-9]+'; echo $?)" "tool runs: $ver"
sha_before=$(sha256sum "$A" | awk '{print $1}')
rm -rf out dmg

step "2. Dictionary from SYSTEM (odr dict)"
dict=$("$ODR" dict "$S" "$A" "$G/odr_odr_mssm01.dbf" "$G/odr_odr_big01.dbf" 2>&1); check $? "odr dict exit 0"
check "$(printf '%s' "$dict" | grep -q '0 issues'; echo $?)" "dictionary reports 0 issues"
n=$(printf '%s\n' "$dict" | grep -c '^ *ODR\.'); check "$([ "$n" = 18 ]; echo $?)" "18 tables in schema ODR (found $n)"
check "$(printf '%s' "$dict" | grep -q AL32UTF8; echo $?)" "database charset AL32UTF8 read from PROPS\$"

step "3. Clean extraction — 20,000 Arabic journal rows"
run_extract out/journal "$S" "$A" --table ODR.T_JOURNAL; expect 20000 0 0 0 "T_JOURNAL"
n=$(wc -l < out/journal/verified.csv); check "$([ "$n" = 20001 ]; echo $?)" "verified.csv has 20,000 rows + header"
n=$(grep -c 'قيد يومية رقم 7684' out/journal/verified.csv); check "$([ "$n" = 1 ]; echo $?)" "Arabic text 'قيد يومية رقم 7684' present exactly once"

step "4. CLOB/BLOB inline + out-of-line (BasicFile and SecureFile)"
for t in T_LOB_BASIC T_LOB_SECURE; do
  run_extract "out/$t" "$S" "$A" --table "ODR.$t"; expect 6 0 0 0 "$t"
  python3 - "out/$t/verified.csv" "$t" <<'PY'
import csv,sys
rows={r['ID']:r for r in csv.DictReader(open(sys.argv[1],encoding='utf-8'))}; t=sys.argv[2]
def chk(c,m): print(('  PASS  ' if c else '  FAIL  ')+m); return c
r6,r2=rows['6'],rows['2']
ok=chk(len(r6['C'])==6000 and set(r6['C'])=={'L'}, f"{t} row 6 CLOB = 6000 × 'L' (assembled from LOB segment)")
ok&=chk(len(r6['B'])==12000 and r6['B']=='4D'*6000, f"{t} row 6 BLOB = 6000 × 0x4D")
ok&=chk(r2['C']=='فاتورة رقم 1 - مرفق', f"{t} row 2 inline Arabic CLOB exact")
sys.exit(0 if ok else 1)
PY
  rc=$?; if [ $rc = 0 ]; then PASS=$((PASS+3)); else PASS=$((PASS+3-rc)); FAIL=$((FAIL+1)); fi
done

step "5. Damage drills (deterministic, original untouched)"
mkdir -p dmg
"$CORRUPT" --input "$A" --output dmg/bitrot.dbf --block-size 8192 --seed 7 --op bitrot:400 >/dev/null 2>&1; check $? "odr-corrupt bitrot:400 seed 7"
run_extract out/bitrot "$S" dmg/bitrot.dbf --table ODR.T_JOURNAL; expect 18744 0 1256 2 "bitrot → T_JOURNAL"
wrong=$(comm -13 <(sort out/journal/verified.csv) <(sort out/bitrot/verified.csv) | wc -l)
check "$([ "$wrong" = 0 ]; echo $?)" "verified rows from the damaged file that are NOT in the clean output: $wrong (must be 0)"
n=$(grep -c 'block integrity: ChecksumFail$' out/bitrot/rejected.csv); check "$([ "$n" = 1256 ]; echo $?)" "rejection reason = 'block integrity: ChecksumFail' ($n rows)"
"$CORRUPT" --input "$A" --output dmg/lobchunk.dbf --block-size 8192 --seed 7 --op bitrot:1 --blocks 3935:3935 >/dev/null 2>&1
run_extract out/lobchunk "$S" dmg/lobchunk.dbf --table ODR.T_LOB_BASIC; expect 5 0 1 2 "1 flipped bit in LOB chunk block 3935 → T_LOB_BASIC"
n=$(wc -l < out/lobchunk/rejected.csv); reason=$(grep -o 'column 2: LOB out of line could not be assembled: Integrity { rdba: 67112799, status: ChecksumFail } (LV-070)' out/lobchunk/rejected.csv | wc -l)
check "$([ "$n" = 2 ] && [ "$reason" = 1 ] && grep -q '^16,[0-9]*,[0-9]*,6,' out/lobchunk/rejected.csv; echo $?)" "only row 6 rejected, reason cites ChecksumFail on rdba 67112799 (LV-070)"

step "6. Compressed table is refused, not guessed"
msg=$("$ODR" extract "$S" "$A" --table ODR.T_COMPRESS --out out/compress 2>&1); code=$?
check "$([ $code = 1 ]; echo $?)" "T_COMPRESS exit code $code (expected 1)"
check "$(printf '%s' "$msg" | grep -q compressed && printf '%s' "$msg" | grep -q LV-069; echo $?)" "message names compression and LV-069"
check "$([ ! -f out/compress/verified.csv ]; echo $?)" "no verified.csv written for a refused table"

step "7. Source datafile untouched"
sha_after=$(sha256sum "$A" | awk '{print $1}'); check "$([ "$sha_before" = "$sha_after" ]; echo $?)" "odr_odr_assm01.dbf sha256 unchanged"

printf '\n================================================================\nRESULT: %d passed, %d failed\n' "$PASS" "$FAIL"
echo "Outputs in $DIR/out/*  (verified.csv / inferred.csv / rejected.csv / report.json)"
echo "Next: read QUICKSTART_ar.md, then try your own damaged .dbf: odr extract <SYSTEM.dbf> <your.dbf> --table OWNER.TABLE --out out/mine"
[ "$FAIL" = 0 ]
