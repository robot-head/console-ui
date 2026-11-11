[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackageRoot,
    [Parameter(Mandatory)][string]$ExpectedSubject,
    [string]$OutputJson
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$files = @(Join-Path $PackageRoot 'console-ui.exe')
$library = Join-Path $PackageRoot 'console_ui.dll'
if (Test-Path $library) { $files += $library }
$evidence = @()
foreach ($file in $files) {
    if (-not (Test-Path $file -PathType Leaf)) { throw "Missing application binary: $file" }
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid') { throw "Invalid Authenticode signature: $file ($($signature.Status))" }
    if ($signature.SignerCertificate.Subject -cne $ExpectedSubject) { throw "Unexpected Authenticode publisher: $file" }
    if (-not $signature.TimeStamperCertificate) { throw "Missing trusted timestamp: $file" }
    if ($signature.SignerCertificate.EnhancedKeyUsageList.ObjectId -notcontains '1.3.6.1.5.5.7.3.3') {
        throw "Certificate does not permit code signing: $file"
    }
    $evidence += @{ file = (Split-Path $file -Leaf); subject = $signature.SignerCertificate.Subject; thumbprint = $signature.SignerCertificate.Thumbprint; timestamp_subject = $signature.TimeStamperCertificate.Subject }
}
if ($OutputJson) {
    New-Item -ItemType Directory -Force (Split-Path $OutputJson -Parent) | Out-Null
    @{ native_signing = 'authenticode'; files = $evidence } | ConvertTo-Json -Depth 5 | Set-Content $OutputJson -Encoding utf8
}
Write-Output 'Application Authenticode signatures, publisher, code-signing usage, and timestamps verified.'
