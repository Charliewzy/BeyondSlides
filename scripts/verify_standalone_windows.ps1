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
    $env:BEYOND_SLIDES_BROWSER_EXTRA_ARGS = "--headless=new --disable-gpu"
    $Process = Start-Process -FilePath $Launcher -PassThru
    $Deadline = [DateTime]::UtcNow.AddSeconds(30)
    $Ready = $false
    while ([DateTime]::UtcNow -lt $Deadline) {
        if ($Process.HasExited) {
            $Log = Join-Path $env:BEYOND_SLIDES_DATA_DIR "controller.log"
            $Diagnostics = if (Test-Path $Log) { Get-Content $Log -Raw } else { "no controller log" }
            throw "The standalone launcher exited with code $($Process.ExitCode): $Diagnostics"
        }
        try {
            $Probe = [Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
            $Probe.Dispose()
            $Ready = $true
            break
        } catch [Net.Sockets.SocketException] {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $Ready) {
        throw "The standalone controller did not become reachable within 30 seconds."
    }
    Start-Sleep -Seconds 1
    if ($Process.HasExited) {
        throw "The standalone launcher did not keep its application window alive."
    }
} finally {
    if ($null -ne $Process -and -not $Process.HasExited) {
        & taskkill.exe /PID "$($Process.Id)" /T /F | Out-Null
        $Process.WaitForExit()
    }
    Remove-Item Env:BEYOND_SLIDES_DATA_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:BEYOND_SLIDES_PORT -ErrorAction SilentlyContinue
    Remove-Item Env:BEYOND_SLIDES_BROWSER_EXTRA_ARGS -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}

$Deadline = [DateTime]::UtcNow.AddSeconds(10)
$ControllerStopped = $false
while ([DateTime]::UtcNow -lt $Deadline) {
    try {
        $Probe = [Net.Sockets.TcpClient]::new("127.0.0.1", $Port)
        $Probe.Dispose()
        Start-Sleep -Milliseconds 200
    } catch [Net.Sockets.SocketException] {
        $ControllerStopped = $true
        break
    }
}
if (-not $ControllerStopped) {
    throw "The launcher smoke test left its controller running."
}

Write-Host "Windows launcher, Chromium, FFmpeg, ffprobe, and PDFium smoke tests passed."
