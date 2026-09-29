param(
    [string]$PackageDirectory = "",
    [string]$OutputDirectory = "",
    [string]$InnoCompiler = ""
)

$ErrorActionPreference = "Stop"
$Repository = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($PackageDirectory)) {
    $PackageDirectory = Join-Path $Repository "dist\beyond-slides-windows-x86_64"
} elseif (-not [IO.Path]::IsPathRooted($PackageDirectory)) {
    $PackageDirectory = Join-Path $Repository $PackageDirectory
}
$PackageDirectory = (Resolve-Path $PackageDirectory).Path

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $Repository "dist"
} elseif (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $Repository $OutputDirectory
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null

foreach ($Required in @("BeyondSlides.exe", "beyond-slides.exe", "chromium", "runtime-tools")) {
    if (-not (Test-Path (Join-Path $PackageDirectory $Required))) {
        throw "The portable package is incomplete; missing $Required"
    }
}

if ([string]::IsNullOrWhiteSpace($InnoCompiler)) {
    $Command = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($null -ne $Command) {
        $InnoCompiler = $Command.Source
    } else {
        $Candidates = @(
            (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
            (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
            (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 7\ISCC.exe"),
            (Join-Path $env:ProgramFiles "Inno Setup 7\ISCC.exe")
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and (Test-Path $_) }
        $InnoCompiler = $Candidates | Select-Object -First 1
    }
}
if ([string]::IsNullOrWhiteSpace($InnoCompiler) -or -not (Test-Path $InnoCompiler)) {
    throw "Inno Setup 6.3 or later is required; pass its ISCC.exe with -InnoCompiler."
}

$CargoManifest = Get-Content (Join-Path $Repository "Cargo.toml") -Raw
$VersionMatch = [regex]::Match($CargoManifest, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $VersionMatch.Success) {
    throw "Could not read the package version from Cargo.toml."
}
$Version = $VersionMatch.Groups[1].Value
$Installer = Join-Path $OutputDirectory "BeyondSlides-Setup-x86_64.exe"
if (Test-Path $Installer) {
    throw "Refusing to replace existing installer output: $Installer"
}

$Script = Join-Path $Repository "packaging\windows\BeyondSlides.iss"
$Icon = Join-Path $Repository "packaging\windows\BeyondSlides.ico"
& $InnoCompiler `
    "/DAppVersion=$Version" `
    "/DSourceDir=$PackageDirectory" `
    "/DOutputDir=$OutputDirectory" `
    "/DIconFile=$Icon" `
    $Script
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path $Installer)) {
    throw "Inno Setup completed without producing $Installer"
}

Write-Host "Windows installer: $Installer"
Write-Host "Installer bytes:   $((Get-Item $Installer).Length)"
