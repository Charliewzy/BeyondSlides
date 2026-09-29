param(
    [Parameter(Mandatory = $true)]
    [string]$Installer
)

$ErrorActionPreference = "Stop"
$Repository = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Installer = (Resolve-Path $Installer).Path
$Temporary = Join-Path ([IO.Path]::GetTempPath()) "beyond-slides-installer-$([Guid]::NewGuid().ToString('N'))"
$InstallDirectory = Join-Path $Temporary "program"
$SetupLog = Join-Path $Temporary "setup.log"
New-Item -ItemType Directory $Temporary | Out-Null

try {
    $Setup = Start-Process -FilePath $Installer -Wait -PassThru -ArgumentList @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/SP-",
        "/DIR=`"$InstallDirectory`"",
        "/LOG=`"$SetupLog`""
    )
    if ($Setup.ExitCode -ne 0) {
        $Diagnostics = if (Test-Path $SetupLog) { Get-Content $SetupLog -Raw } else { "no setup log" }
        throw "The installer exited with code $($Setup.ExitCode): $Diagnostics"
    }

    & (Join-Path $Repository "scripts\verify_standalone_windows.ps1") $InstallDirectory
    if ($LASTEXITCODE -ne 0) {
        throw "The installed application failed standalone verification."
    }

    $Uninstaller = Join-Path $InstallDirectory "unins000.exe"
    if (-not (Test-Path $Uninstaller)) {
        throw "The installation did not create an uninstaller."
    }
    $Uninstall = Start-Process -FilePath $Uninstaller -Wait -PassThru -ArgumentList @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART"
    )
    if ($Uninstall.ExitCode -ne 0) {
        throw "The uninstaller exited with code $($Uninstall.ExitCode)."
    }
    Start-Sleep -Milliseconds 500
    if (Test-Path (Join-Path $InstallDirectory "BeyondSlides.exe")) {
        throw "The uninstaller left application files behind."
    }
} finally {
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}

Write-Host "Windows installer install, application, and uninstall smoke tests passed."
