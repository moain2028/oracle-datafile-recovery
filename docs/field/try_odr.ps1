# try_odr.ps1 — ODR self-checking trial, Windows PowerShell 5.1+ / pwsh 7.
# One file. It downloads the tool + the test datafiles, verifies SHA-256, runs the
# quick-start commands and COMPARES every number with what Oracle itself says.
# Prints PASS/FAIL per step. Never writes to the source datafiles.
#
#   powershell -ExecutionPolicy Bypass -File try_odr.ps1 [-Dir C:\odr-trial] [-Tag v0.2.0-alpha]
#
# Needs: internet, ~700 MB disk (62 MB download, ~480 MB extracted), tar.exe (built into Windows 10+).

[CmdletBinding()]
param(
    [string]$Dir = (Join-Path $PWD "odr-trial"),
    [string]$Tag = "v0.2.0-alpha",
    [string]$Golden = "golden-26ai-20261024",
    [switch]$SkipDownload,
    # Testing only: use binaries from this directory instead of downloading the Windows zip.
    [string]$ToolDir = ""
)

$ErrorActionPreference = "Stop"
$Repo = "https://github.com/moain2026/oracle-datafile-recovery"
$script:Fail = 0
$script:Pass = 0

function Step($name) { Write-Host ""; Write-Host "== $name" -ForegroundColor Cyan }
function Ok($msg)    { $script:Pass++; Write-Host "  PASS  $msg" -ForegroundColor Green }
function Bad($msg)   { $script:Fail++; Write-Host "  FAIL  $msg" -ForegroundColor Red }
function Check($cond, $msg) { if ($cond) { Ok $msg } else { Bad $msg } }

# The repository and its Releases are private: download with `gh` (logged in as an
# invited reader) or with $env:ODR_TOKEN = <read token>; otherwise copy the files into -Dir.
function Get-File($url, $out, $tag) {
    if (Test-Path $out) { Write-Host "  (exists) $out"; return }
    $name = Split-Path $out -Leaf
    Write-Host "  downloading $name"
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        & gh release download $tag -R moain2026/oracle-datafile-recovery -p $name -O $out 2>$null
        if ($LASTEXITCODE -eq 0 -and (Test-Path $out)) { return }
    }
    if ($env:ODR_TOKEN) {
        $h = @{ Authorization = "Bearer $env:ODR_TOKEN" }
        $rel = Invoke-RestMethod -Headers $h -Uri "https://api.github.com/repos/moain2026/oracle-datafile-recovery/releases/tags/$tag"
        $asset = $rel.assets | Where-Object name -eq $name | Select-Object -First 1
        if ($asset) {
            $h["Accept"] = "application/octet-stream"
            Invoke-WebRequest -Headers $h -Uri $asset.url -OutFile $out -UseBasicParsing
            return
        }
    }
    throw "cannot download ${name}: private release - run 'gh auth login' as an invited reader, set `$env:ODR_TOKEN, or copy the file into $Dir"
}

function Verify-Sha($file, $shaFile) {
    $expected = ((Get-Content $shaFile -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
    Check ($expected -eq $actual) "sha256 $(Split-Path $file -Leaf) = $actual"
    if ($expected -ne $actual) { throw "checksum mismatch for $file — stop, do not trust this download" }
}

# Parse "ODR.T: verified N · inferred N · rejected N" from odr's last stdout line.
function Run-Extract($argv, $outDir) {
    $out = & $Odr extract @argv --out $outDir 2>&1
    $code = $LASTEXITCODE
    $line = ($out | Where-Object { $_ -match 'verified \d+' } | Select-Object -Last 1)
    $m = [regex]::Match("$line", 'verified (\d+) · inferred (\d+) · rejected (\d+)')
    [pscustomobject]@{
        Code = $code
        Verified = if ($m.Success) { [int]$m.Groups[1].Value } else { -1 }
        Inferred = if ($m.Success) { [int]$m.Groups[2].Value } else { -1 }
        Rejected = if ($m.Success) { [int]$m.Groups[3].Value } else { -1 }
        Text = ($out -join "`n")
    }
}

function Expect-Layers($r, $v, $i, $j, $code, $what) {
    Check ($r.Verified -eq $v -and $r.Inferred -eq $i -and $r.Rejected -eq $j) `
        "$what → verified $($r.Verified) · inferred $($r.Inferred) · rejected $($r.Rejected) (expected $v · $i · $j)"
    Check ($r.Code -eq $code) "$what exit code $($r.Code) (expected $code)"
}

New-Item -ItemType Directory -Force $Dir | Out-Null
Set-Location $Dir
Write-Host "ODR trial in $Dir  (tool $Tag, datafiles $Golden)"

# ---------------------------------------------------------------- 1. download + verify
Step "1. Download and verify (SHA-256)"
$Zip = "odr-$Tag-x86_64-pc-windows-msvc.zip"
$Tgz = "odr_golden_26ai_v2_20261024T085903Z.tar.gz"
if (-not $SkipDownload) {
    if ($ToolDir -eq "") {
        Get-File "$Repo/releases/download/$Tag/$Zip"        $Zip        $Tag
        Get-File "$Repo/releases/download/$Tag/$Zip.sha256" "$Zip.sha256" $Tag
    }
    Get-File "$Repo/releases/download/$Golden/$Tgz"          $Tgz "$Golden"
    Get-File "$Repo/releases/download/$Golden/odr_golden_26ai_v2.sha256" "odr_golden_26ai_v2.sha256" $Golden
}
if ($ToolDir -eq "") { Verify-Sha $Zip "$Zip.sha256" }
Verify-Sha $Tgz "odr_golden_26ai_v2.sha256"

$Exe = if ($IsWindows -or $null -eq $IsWindows) { ".exe" } else { "" }
if ($ToolDir -eq "") {
    $ToolDir = Join-Path $Dir "odr-$Tag-x86_64-pc-windows-msvc"
    if (-not (Test-Path (Join-Path $ToolDir "odr.exe"))) { Expand-Archive $Zip -DestinationPath $Dir -Force }
}
$Odr = Join-Path $ToolDir "odr$Exe"
$Corrupt = Join-Path $ToolDir "odr-corrupt$Exe"
if (-not (Test-Path (Join-Path $Dir "20261024T085903Z/datafiles/odr_odr_assm01.dbf"))) {
    Write-Host "  extracting $Tgz (≈ 480 MB)"
    & tar -xzf $Tgz
    if ($LASTEXITCODE -ne 0) { throw "tar failed" }
}
$G = Join-Path $Dir "20261024T085903Z/datafiles"
$S = Join-Path $G "FREE_FREEPDB1_system01.dbf"
$A = Join-Path $G "odr_odr_assm01.dbf"
$ver = (& $Odr --version) -join ""
Check ($ver -match '^odr \d+\.\d+\.\d+') "tool runs: $ver"
$shaBefore = (Get-FileHash $A -Algorithm SHA256).Hash

if (Test-Path out) { Remove-Item out -Recurse -Force }

# ---------------------------------------------------------------- 2. dictionary
Step "2. Dictionary from SYSTEM (odr dict)"
$dict = & $Odr dict $S $A (Join-Path $G "odr_odr_mssm01.dbf") (Join-Path $G "odr_odr_big01.dbf") 2>&1
Check ($LASTEXITCODE -eq 0) "odr dict exit 0"
$dictText = $dict -join "`n"
Check ($dictText -match '0 issues') "dictionary reports 0 issues"
$odrTables = @($dict | Where-Object { $_ -match '^\s+ODR\.' })
Check ($odrTables.Count -eq 18) "18 tables in schema ODR (found $($odrTables.Count))"
Check ($dictText -match 'AL32UTF8') "database charset AL32UTF8 read from PROPS\$"

# ---------------------------------------------------------------- 3. clean extraction
Step "3. Clean extraction — 20,000 Arabic journal rows"
$j = Run-Extract @($S, $A, "--table", "ODR.T_JOURNAL") "out/journal"
Expect-Layers $j 20000 0 0 0 "T_JOURNAL"
$rows = Get-Content "out/journal/verified.csv"
Check ($rows.Count -eq 20001) "verified.csv has 20,000 rows + header"
Check (($rows | Select-String -SimpleMatch 'قيد يومية رقم 7684').Count -eq 1) "Arabic text 'قيد يومية رقم 7684' present exactly once"

# ---------------------------------------------------------------- 4. LOBs
Step "4. CLOB/BLOB inline + out-of-line (BasicFile and SecureFile)"
foreach ($t in "T_LOB_BASIC", "T_LOB_SECURE") {
    $r = Run-Extract @($S, $A, "--table", "ODR.$t") "out/$t"
    Expect-Layers $r 6 0 0 0 $t
    $csv = Import-Csv "out/$t/verified.csv"
    $row6 = $csv | Where-Object { $_.ID -eq "6" }
    Check ($row6.C.Length -eq 6000 -and ($row6.C -replace 'L', '') -eq '') "$t row 6 CLOB = 6000 × 'L' (assembled from LOB segment)"
    Check ($row6.B.Length -eq 12000 -and ($row6.B -replace '4D', '') -eq '') "$t row 6 BLOB = 6000 × 0x4D"
    $row2 = $csv | Where-Object { $_.ID -eq "2" }
    Check ($row2.C -eq 'فاتورة رقم 1 - مرفق') "$t row 2 inline Arabic CLOB exact"
}

# ---------------------------------------------------------------- 5. damage drills
Step "5. Damage drills (deterministic, original untouched)"
# odr-corrupt refuses to overwrite an existing output (by design); start clean so re-runs work.
if (Test-Path dmg) { Remove-Item dmg -Recurse -Force }
New-Item -ItemType Directory -Force dmg | Out-Null
& $Corrupt --input $A --output dmg/bitrot.dbf --block-size 8192 --seed 7 --op bitrot:400 | Out-Null
Check ($LASTEXITCODE -eq 0) "odr-corrupt bitrot:400 seed 7"
$b = Run-Extract @($S, "dmg/bitrot.dbf", "--table", "ODR.T_JOURNAL") "out/bitrot"
Expect-Layers $b 18744 0 1256 2 "bitrot → T_JOURNAL"
$clean = [System.Collections.Generic.HashSet[string]]::new([string[]](Get-Content "out/journal/verified.csv"))
$wrong = @(Get-Content "out/bitrot/verified.csv" | Where-Object { -not $clean.Contains($_) })
Check ($wrong.Count -eq 0) "verified rows from the damaged file that are NOT in the clean output: $($wrong.Count) (must be 0)"
$reasons = Import-Csv "out/bitrot/rejected.csv" | Group-Object _reason | Sort-Object Count -Descending
Check ($reasons[0].Name -eq 'block integrity: ChecksumFail') "rejection reason = '$($reasons[0].Name)' ($($reasons[0].Count) rows)"

& $Corrupt --input $A --output dmg/lobchunk.dbf --block-size 8192 --seed 7 --op bitrot:1 --blocks 3935:3935 | Out-Null
$l = Run-Extract @($S, "dmg/lobchunk.dbf", "--table", "ODR.T_LOB_BASIC") "out/lobchunk"
Expect-Layers $l 5 0 1 2 "1 flipped bit in LOB chunk block 3935 → T_LOB_BASIC"
$rej = Import-Csv "out/lobchunk/rejected.csv"
Check ($rej.Count -eq 1 -and $rej[0].ID -eq "6" -and $rej[0]._reason -match 'LOB out of line could not be assembled: Integrity .*ChecksumFail.*LV-070') `
    "only row 6 rejected, reason: $($rej[0]._reason)"

# ---------------------------------------------------------------- 6. explicit refusal
Step "6. Compressed table (COMPRESS BASIC): v0.2.0-alpha refuses it, later builds decode it (LV-071)"
$c = & $Odr extract $S $A --table ODR.T_COMPRESS --out out/compress 2>&1
$code = $LASTEXITCODE
if ($code -eq 1) {
    Ok "T_COMPRESS exit code 1: refused, not guessed"
    Check (("$c" -match 'compressed') -and ("$c" -match 'LV-069')) "message names compression and LV-069"
    Check (-not (Test-Path out/compress/verified.csv)) "no verified.csv written for a refused table"
} else {
    $r = Run-Extract @($S, $A, "--table", "ODR.T_COMPRESS") "out/compress"
    Expect-Layers $r 2000 0 0 0 "T_COMPRESS decoded through the symbol table (LV-071)"
    $csv = Import-Csv "out/compress/verified.csv"
    $bad = @($csv | Where-Object { $_.V -ne ("repeated value " + ([int]$_.ID % 5)) -or [int]$_.N -ne ([int]$_.ID % 7) })
    Check ($bad.Count -eq 0) "every decoded row equals the generating formula (V = 'repeated value ' + ID mod 5, N = ID mod 7): $($bad.Count) wrong"
}

# ---------------------------------------------------------------- 7. source untouched
Step "7. Source datafile untouched"
$shaAfter = (Get-FileHash $A -Algorithm SHA256).Hash
Check ($shaBefore -eq $shaAfter) "odr_odr_assm01.dbf sha256 unchanged"

# ---------------------------------------------------------------- summary
Write-Host ""
Write-Host "================================================================"
Write-Host ("RESULT: {0} passed, {1} failed" -f $script:Pass, $script:Fail) -ForegroundColor ($(if ($script:Fail -eq 0) { "Green" } else { "Red" }))
Write-Host "Outputs in $Dir/out/*  (verified.csv / inferred.csv / rejected.csv / report.json)"
Write-Host "Next: read QUICKSTART_ar.md, then try your own damaged .dbf: odr.exe extract <SYSTEM.dbf> <your.dbf> --table OWNER.TABLE --out out/mine"
exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
