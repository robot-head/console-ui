$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot '../verify-windows-signatures.ps1'
$root = Join-Path $env:RUNNER_TEMP 'signature-policy-test'
New-Item -ItemType Directory -Force $root | Out-Null
$candidates = @((Get-Process -Id $PID).Path, "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe")
$fixture = $null
foreach ($candidate in $candidates) {
    $signature = Get-AuthenticodeSignature -LiteralPath $candidate
    if ($signature.Status -eq 'Valid' -and $signature.TimeStamperCertificate) { $fixture = $candidate; break }
}
if (-not $fixture) { throw 'No trusted timestamped Microsoft executable found for the policy test' }
$subject = (Get-AuthenticodeSignature -LiteralPath $fixture).SignerCertificate.Subject
$application = Join-Path $root 'console-ui.exe'
Copy-Item $fixture $application
& $verify -PackageRoot $root -ExpectedSubject $subject -OutputJson (Join-Path $root 'evidence/result.json')
$rejected = $false
try { & $verify -PackageRoot $root -ExpectedSubject 'CN=Unexpected publisher' } catch { $rejected = $true }
if (-not $rejected) { throw 'Unexpected publisher was accepted' }
$bytes = [System.IO.File]::ReadAllBytes($application)
$bytes[1024] = $bytes[1024] -bxor 1
[System.IO.File]::WriteAllBytes($application, $bytes)
$rejected = $false
try { & $verify -PackageRoot $root -ExpectedSubject $subject } catch { $rejected = $true }
if (-not $rejected) { throw 'Tampered signed executable was accepted' }
Write-Output 'Verified real Authenticode acceptance, publisher rejection, and PE tamper detection.'
