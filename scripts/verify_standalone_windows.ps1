param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDirectory
)

$ErrorActionPreference = "Stop"
$PackageDirectory = (Resolve-Path $PackageDirectory).Path
$Launcher = Join-Path $PackageDirectory "BeyondSlides.exe"
$Backend = Join-Path $PackageDirectory "beyond-slides.exe"
$Chromium = Join-Path $PackageDirectory "chromium\chrome.exe"
$RuntimeTools = Join-Path $PackageDirectory "runtime-tools"
foreach ($Required in @($Launcher, $Backend, $Chromium, $RuntimeTools)) {
    if (-not (Test-Path $Required)) {
        throw "Missing standalone component: $Required"
    }
}

$ChromiumHash = (Get-FileHash $Chromium -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ChromiumHash -ne "11ca7ce7021bdffcf102797f8a00c713f5295b5e7313f3f136efcd998422e6f2") {
    throw "Unexpected packaged Chromium executable hash: $ChromiumHash"
}
$Ffmpeg = Get-ChildItem $RuntimeTools -Filter "ffmpeg.exe" -Recurse -File | Select-Object -First 1
$Ffprobe = Get-ChildItem $RuntimeTools -Filter "ffprobe.exe" -Recurse -File | Select-Object -First 1
$Pdfium = Get-ChildItem $RuntimeTools -Filter "pdfium.dll" -Recurse -File | Select-Object -First 1
if ($null -in @($Ffmpeg, $Ffprobe, $Pdfium)) {
    throw "The package is missing FFmpeg, ffprobe, or PDFium."
}
& $Ffmpeg.FullName -version | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Packaged FFmpeg did not start." }
& $Ffprobe.FullName -version | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Packaged ffprobe did not start." }

$Listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$Listener.Start()
$Port = ([Net.IPEndPoint]$Listener.LocalEndpoint).Port
$Listener.Stop()
$Temporary = Join-Path ([IO.Path]::GetTempPath()) "beyond-slides-windows-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $Temporary | Out-Null
try {
    $env:BEYOND_SLIDES_DATA_DIR = Join-Path $Temporary "data"
    $env:BEYOND_SLIDES_PORT = "$Port"
    $env:BEYOND_SLIDES_BROWSER_EXTRA_ARGS = "--headless=new --dump-dom --disable-gpu"
    $Process = Start-Process -FilePath $Launcher -PassThru
    if (-not $Process.WaitForExit(30000)) {
        Stop-Process -Id $Process.Id -Force
        throw "The standalone launcher did not finish its headless smoke test."
    }
    if ($Process.ExitCode -ne 0) {
        $Log = Join-Path $env:BEYOND_SLIDES_DATA_DIR "controller.log"
        $Diagnostics = if (Test-Path $Log) { Get-Content $Log -Raw } else { "no controller log" }
        throw "The standalone launcher exited with code $($Process.ExitCode): $Diagnostics"
    }
    Start-Sleep -Milliseconds 300
    try {
        $Probe = [Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
        $Probe.Dispose()
        throw "The launcher left its controller running after Chromium exited."
    } catch [Net.Sockets.SocketException] {
        # Expected: closing the browser stops the local controller.
    }
} finally {
    Remove-Item Env:BEYOND_SLIDES_DATA_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:BEYOND_SLIDES_PORT -ErrorAction SilentlyContinue
    Remove-Item Env:BEYOND_SLIDES_BROWSER_EXTRA_ARGS -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}

Write-Host "Windows launcher, Chromium, FFmpeg, ffprobe, and PDFium smoke tests passed."
