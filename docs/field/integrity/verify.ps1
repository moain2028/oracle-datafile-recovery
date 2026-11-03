# verify.ps1 - check an ODR folder against integrity\SHA256SUMS. Windows PowerShell 5.1+ / pwsh 7.
#   1. every file listed in SHA256SUMS exists and has that SHA-256;
#   2. SHA256SUMS.p7s is a valid signature of SHA256SUMS made by the ODR certificate
#      (thumbprint in SIGNER.txt; public part in ODR-code-signing.cer);
#   3. every listed .exe carries an Authenticode signature by that same certificate
#      (Windows only). The certificate is self-issued: Windows reports it "not trusted"
#      unless trust-certificate.ps1 was run - that is expected; what matters is that the
#      signature is intact and the signer thumbprint is exactly SIGNER.txt.
# Exit 0 = intact and signed by ODR; 1 = do not trust this folder.
#
#   powershell -ExecutionPolicy Bypass -File integrity\verify.ps1 [-Root <folder>]
[CmdletBinding()]
param([string]$Root = "")
$ErrorActionPreference = "Stop"
$I = $PSScriptRoot
if ($Root -eq "") { $Root = Split-Path $I -Parent }
$Root = (Resolve-Path $Root).Path
$script:fail = 0
function Say($ok, $msg) {
    if ($ok) { Write-Host "  PASS  $msg" -ForegroundColor Green }
    else { Write-Host "  FAIL  $msg" -ForegroundColor Red; $script:fail = 1 }
}

$want = (Get-Content (Join-Path $I "SIGNER.txt") | Where-Object { $_ -match '^[0-9A-F]{40}$' } | Select-Object -First 1)

# 1. hashes
$bad = @(); $n = 0; $exes = @()
foreach ($line in [System.IO.File]::ReadAllLines((Join-Path $I "SHA256SUMS"))) {
    if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { continue }
    $n++
    $hash = $Matches[1]; $rel = $Matches[2]
    $p = Join-Path $Root ($rel -replace '/', [System.IO.Path]::DirectorySeparatorChar)
    if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { $bad += "missing: $rel"; continue }
    if ((Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLower() -ne $hash) { $bad += "changed: $rel"; continue }
    if ($rel -like '*.exe') { $exes += $p }
}
foreach ($b in $bad) { Write-Host "        $b" -ForegroundColor Red }
Say ($n -gt 0 -and $bad.Count -eq 0) "integrity: $n files match SHA256SUMS"

# 2. signature of the hash list (detached CMS, verified without installing the certificate)
try {
    try { Add-Type -AssemblyName System.Security -ErrorAction Stop } catch { }
    $content = New-Object System.Security.Cryptography.Pkcs.ContentInfo (, [System.IO.File]::ReadAllBytes((Join-Path $I "SHA256SUMS")))
    $cms = New-Object System.Security.Cryptography.Pkcs.SignedCms ($content, $true)
    $cms.Decode([System.IO.File]::ReadAllBytes((Join-Path $I "SHA256SUMS.p7s")))
    $cms.CheckSignature($true)   # signature only: the certificate is self-issued
    $signer = $cms.SignerInfos[0].Certificate.Thumbprint
    $cer = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 (Join-Path $I "ODR-code-signing.cer")
    Say ($signer -eq $want -and $cer.Thumbprint -eq $want) "SHA256SUMS signed by ODR certificate $signer (expected $want)"
} catch {
    Say $false "SHA256SUMS signature invalid: $($_.Exception.Message)"
}

# 3. Authenticode on the executables (Windows only)
if ((Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue) -and ($exes.Count -gt 0)) {
    foreach ($e in $exes) {
        $s = Get-AuthenticodeSignature -LiteralPath $e
        $tp = if ($s.SignerCertificate) { $s.SignerCertificate.Thumbprint } else { "" }
        # Valid = certificate trusted on this PC; UnknownError/NotTrusted = intact but self-issued root.
        $intact = @('Valid', 'UnknownError', 'NotTrusted') -contains "$($s.Status)"
        $ts = if ($s.TimeStamperCertificate) { "timestamped" } else { "no timestamp" }
        Say ($intact -and $tp -eq $want) "$(Split-Path $e -Leaf) Authenticode: status $($s.Status), signer $tp, $ts"
    }
}
exit $script:fail
