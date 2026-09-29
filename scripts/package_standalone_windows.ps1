param(
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"
$Repository = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([Environment]::Is64BitOperatingSystem -ne $true) {
    throw "The standalone Windows package requires 64-bit Windows."
}
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $Repository "dist\beyond-slides-windows-x86_64"
} elseif (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $Repository $OutputDirectory
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$Archive = "$OutputDirectory.zip"
if ((Test-Path $OutputDirectory) -or (Test-Path $Archive)) {
    throw "Refusing to replace existing package output: $OutputDirectory"
}

$OutputParent = Split-Path $OutputDirectory -Parent
$OutputName = Split-Path $OutputDirectory -Leaf
New-Item -ItemType Directory -Force $OutputParent | Out-Null
$Staging = Join-Path $OutputParent ".$OutputName.$([Guid]::NewGuid().ToString('N'))"

try {
    Push-Location $Repository
    try {
        cargo build --release --locked --bin beyond-slides --bin beyond-slides-launcher
        if ($LASTEXITCODE -ne 0) {
            throw "The Rust release build failed."
        }
    } finally {
        Pop-Location
    }

    New-Item -ItemType Directory $Staging | Out-Null
    Copy-Item (Join-Path $Repository "target\release\beyond-slides.exe") $Staging
    Copy-Item (Join-Path $Repository "target\release\beyond-slides-launcher.exe") `
        (Join-Path $Staging "BeyondSlides.exe")
    Get-ChildItem (Join-Path $Repository "target\release") -Filter "*.dll" -File |
        Copy-Item -Destination $Staging
    Copy-Item (Join-Path $Repository "packaging\windows\README.txt") $Staging
    Copy-Item (Join-Path $Repository "LICENSE") `
        (Join-Path $Staging "LICENSE.beyond-slides.txt")
    Copy-Item (Join-Path $Repository "NOTICE") (Join-Path $Staging "NOTICE.txt")

    $Backend = Join-Path $Staging "beyond-slides.exe"
    & $Backend install-runtime-tools (Join-Path $Staging "runtime-tools")
    if ($LASTEXITCODE -ne 0) {
        throw "Could not install the managed FFmpeg and PDFium runtimes."
    }
    & $Backend install-chromium (Join-Path $Staging "chromium")
    if ($LASTEXITCODE -ne 0) {
        throw "Could not install the packaged Chromium browser."
    }

    Move-Item $Staging $OutputDirectory
    Compress-Archive -Path $OutputDirectory -DestinationPath $Archive -CompressionLevel Optimal
    Write-Host "Standalone directory: $OutputDirectory"
    Write-Host "Download archive:     $Archive"
    Write-Host "Archive bytes:        $((Get-Item $Archive).Length)"
} finally {
    if (Test-Path $Staging) {
        Remove-Item -Recurse -Force $Staging
    }
}
