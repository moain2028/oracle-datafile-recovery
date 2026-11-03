# trust-certificate.ps1 - OPTIONAL. Make this Windows account trust the ODR code-signing certificate,
# so Windows shows "ODR Oracle Datafile Recovery" (moain2028) as a verified publisher for odr.exe.
#
# Adds integrity\ODR-code-signing.cer (public certificate, no private key) to the CURRENT USER's
# "Trusted Root" and "Trusted Publishers" stores - no administrator rights needed; other users and
# the machine stores are untouched. Windows asks you to confirm the Root addition: check that the
# thumbprint shown equals SIGNER.txt before you click Yes.
#
#   powershell -ExecutionPolicy Bypass -File integrity\trust-certificate.ps1            # trust
#   powershell -ExecutionPolicy Bypass -File integrity\trust-certificate.ps1 -Remove    # undo
#
# You do NOT need this to use ODR: verify.ps1 checks the signature without it.
[CmdletBinding()]
param([switch]$Remove)
$ErrorActionPreference = "Stop"
$cerPath = Join-Path $PSScriptRoot "ODR-code-signing.cer"
$want = (Get-Content (Join-Path $PSScriptRoot "SIGNER.txt") | Where-Object { $_ -match '^[0-9A-F]{40}$' } | Select-Object -First 1)
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 $cerPath
if ($cert.Thumbprint -ne $want) { throw "ODR-code-signing.cer thumbprint $($cert.Thumbprint) != SIGNER.txt $want - do not trust this folder" }
Write-Host "Certificate: $($cert.Subject)"
Write-Host "Thumbprint : $($cert.Thumbprint)   valid $($cert.NotBefore.ToString('yyyy-MM-dd')) .. $($cert.NotAfter.ToString('yyyy-MM-dd'))"
foreach ($name in "Root", "TrustedPublisher") {
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store ($name, "CurrentUser")
    $store.Open("ReadWrite")
    if ($Remove) {
        $found = $store.Certificates.Find("FindByThumbprint", $want, $false)
        foreach ($c in $found) { $store.Remove($c) }
        Write-Host "  removed from CurrentUser\$name ($($found.Count))"
    } else {
        $store.Add($cert)
        Write-Host "  added to CurrentUser\$name"
    }
    $store.Close()
}
Write-Host "Done. Check: Get-AuthenticodeSignature odr.exe  ->  Status Valid"
