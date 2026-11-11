[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64')][string]$Architecture = 'x64',
    [switch]$StageOnly,
    [switch]$ArchiveOnly,
    [switch]$RequireSigned
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($StageOnly -and $ArchiveOnly) { throw 'StageOnly and ArchiveOnly are mutually exclusive' }
if ($RequireSigned -and -not $ArchiveOnly) { throw 'Sign the staged application, then use ArchiveOnly with RequireSigned' }
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    $triple = if ($Architecture -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
    $buildRoot = Join-Path $projectRoot "target/package-$Architecture"
    $packageRoot = Join-Path $projectRoot "dist/console-ui-win-$Architecture"
    if (-not $ArchiveOnly) {
        # A dedicated clean output avoids accidentally shipping stale runtime DLLs or another binary.
        & cargo clean --target-dir $buildRoot
        if ($LASTEXITCODE -ne 0) { throw 'cargo clean failed' }
        & cargo build --locked --release --bin console-ui --target $triple --target-dir $buildRoot
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
        $profileRoot = Join-Path $buildRoot "$triple/release"
        if (Test-Path $packageRoot) { Remove-Item -Recurse -Force $packageRoot }
        New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
        $excluded = @('build', 'deps', 'examples', 'incremental', '.fingerprint')
        Get-ChildItem $profileRoot -Force | Where-Object {
            $_.Name -notin $excluded -and $_.Name -notlike '.*' -and $_.Extension -notin @('.d', '.pdb', '.rlib', '.rmeta')
        } | Copy-Item -Destination $packageRoot -Recurse -Force

        # WinUI's native DLLs also depend on the Visual C++ runtime. Ship its app-local DLLs.
        $crtRoot = $env:VCToolsRedistDir
        if (-not $crtRoot) {
            $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
            if (-not (Test-Path $vswhere)) { throw 'Visual Studio C++ tools are required to locate redistributable CRT DLLs' }
            $installation = & $vswhere -latest -products '*' -property installationPath
            if (-not $installation) { throw 'Visual Studio installation not found' }
            $crtRoot = (Get-ChildItem (Join-Path $installation 'VC/Redist/MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1).FullName
        }
        $crt = Get-ChildItem (Join-Path $crtRoot $Architecture) -Directory -Filter 'Microsoft.VC*.CRT' | Select-Object -First 1
        if (-not $crt) { throw "No $Architecture redistributable CRT found under $crtRoot" }
        Copy-Item (Join-Path $crt.FullName '*.dll') -Destination $packageRoot
        Copy-Item README.md, LICENSE, THIRD_PARTY.md -Destination $packageRoot
    }
    foreach ($required in @('console-ui.exe', 'Microsoft.ui.xaml.dll', 'Microsoft.WindowsAppRuntime.dll', 'Microsoft.ui.pri', 'msvcp140.dll', 'vcruntime140.dll')) {
        if (-not (Test-Path (Join-Path $packageRoot $required))) { throw "Self-contained payload is missing $required" }
    }
    if ($StageOnly) { Write-Output "Staged self-contained payload: $packageRoot"; return }
    if ($RequireSigned) {
        & "$PSScriptRoot/verify-windows-signatures.ps1" -PackageRoot $packageRoot -ExpectedSubject $env:WINDOWS_SIGNER_SUBJECT -OutputJson (Join-Path $projectRoot 'dist/signing-evidence/windows.json')
    }
    $zip = "$packageRoot.zip"
    Compress-Archive -Path "$packageRoot/*" -DestinationPath $zip -Force
    Get-FileHash $zip -Algorithm SHA256 | ForEach-Object { "$($_.Hash.ToLower())  $(Split-Path $zip -Leaf)" } | Set-Content "$zip.sha256" -Encoding ascii
    Write-Output "Self-contained package: $zip"
} finally { Pop-Location }
