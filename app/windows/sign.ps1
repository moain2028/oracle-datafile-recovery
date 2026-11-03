# sign.ps1 FILE... - Authenticode-sign with the ODR certificate (SHA-256 + RFC 3161 timestamp) and verify.
# Needs env ODR_SIGN_PFX_B64 + ODR_SIGN_PASS (repository secrets). Used by .github/workflows/app.yml.
param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Files)
$ErrorActionPreference = "Stop"
$pfx = Join-Path $env:RUNNER_TEMP "odr-sign.pfx"
[IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:ODR_SIGN_PFX_B64))
try {
    $st = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1
    $want = (Get-Content (Join-Path $PSScriptRoot "..\..\docs\field\integrity\SIGNER.txt") | Where-Object { $_ -match '^[0-9A-F]{40}$' })
    foreach ($f in $Files) {
        $ok = $false
        foreach ($ts in "http://timestamp.digicert.com", "http://timestamp.sectigo.com") {
            & $st.FullName sign /f $pfx /p $env:ODR_SIGN_PASS /fd SHA256 /tr $ts /td SHA256 /d "ODR Oracle Datafile Recovery" $f | Out-Host
            if ($LASTEXITCODE -eq 0) { $ok = $true; break }
        }
        if (-not $ok) { throw "signing failed: $f" }
        $s = Get-AuthenticodeSignature $f
        if ($s.SignerCertificate.Thumbprint -ne $want -or -not $s.TimeStamperCertificate) { throw "signature check failed: $f $($s.Status)" }
        Write-Host "signed: $(Split-Path $f -Leaf)  signer $($s.SignerCertificate.Thumbprint)  timestamped"
    }
} finally { Remove-Item $pfx -Force -ErrorAction SilentlyContinue }
