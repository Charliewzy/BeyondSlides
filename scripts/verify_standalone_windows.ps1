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
$EmbeddingModel = Join-Path $PackageDirectory "models\bge-small-zh-v1.5-modelscope-08d7186b7de51be7c12444137221ad96825593d6"
foreach ($Required in @($Launcher, $Backend, $Chromium, $RuntimeTools, $EmbeddingModel)) {
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
$EmbeddingAssets = @(
    @{ Name = "model.onnx"; Bytes = 94851877; Sha256 = "69a0b846f4f116b5e6aabf9546ea6754d02264f3211a13a1bd69b31b8040749a" },
    @{ Name = "tokenizer.json"; Bytes = 439125; Sha256 = "48cea5d44424912a6fd1ea647bf4fe50b55ab8b1e5879c3275f80e339e8fae26" },
    @{ Name = "config.json"; Bytes = 716; Sha256 = "d4193ead3a810fd694fa8a31d7fc72fbaebc0668b603e398734bf2f6538ff42f" },
    @{ Name = "special_tokens_map.json"; Bytes = 125; Sha256 = "b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3" },
    @{ Name = "tokenizer_config.json"; Bytes = 367; Sha256 = "e6f3b96db926a37d4039995fbf5ad17de158dfb8f6343d607e4dbaad18d75f5a" }
)
foreach ($Asset in $EmbeddingAssets) {
    $Path = Join-Path $EmbeddingModel $Asset.Name
    if (-not (Test-Path $Path)) {
        throw "The package is missing BGE embedding asset $($Asset.Name)."
    }
    if ((Get-Item $Path).Length -ne $Asset.Bytes) {
        throw "Unexpected packaged BGE embedding asset length: $($Asset.Name)."
    }
    $Hash = (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Hash -ne $Asset.Sha256) {
        throw "Unexpected packaged BGE embedding asset hash: $($Asset.Name): $Hash"
    }
}
& $Ffmpeg.FullName -version | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Packaged FFmpeg did not start." }
& $Ffprobe.FullName -version | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Packaged ffprobe did not start." }

function Get-FreeTcpPort {
    $Listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $Listener.Start()
    $SelectedPort = ([Net.IPEndPoint]$Listener.LocalEndpoint).Port
    $Listener.Stop()
    return $SelectedPort
}

$Port = Get-FreeTcpPort
$BrowserPort = Get-FreeTcpPort
while ($BrowserPort -eq $Port) { $BrowserPort = Get-FreeTcpPort }
$Temporary = Join-Path ([IO.Path]::GetTempPath()) "beyond-slides-windows-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $Temporary | Out-Null
try {
    $env:BEYOND_SLIDES_DATA_DIR = Join-Path $Temporary "data"
    $env:BEYOND_SLIDES_PORT = "$Port"
    $env:BEYOND_SLIDES_BROWSER_EXTRA_ARGS = "--headless=new --disable-gpu --remote-debugging-port=$BrowserPort"
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

    # Ask only the application Chromium to close through its private DevTools
    # endpoint. The launcher must then gracefully stop the controller; killing
    # the launcher's process tree would hide orphaned auxiliary Chromium.
    $BrowserDeadline = [DateTime]::UtcNow.AddSeconds(15)
    $BrowserMetadata = $null
    while ([DateTime]::UtcNow -lt $BrowserDeadline) {
        try {
            $BrowserMetadata = Invoke-RestMethod "http://127.0.0.1:$BrowserPort/json/version"
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if ($null -eq $BrowserMetadata.webSocketDebuggerUrl) {
        throw "The packaged application Chromium did not expose its test endpoint."
    }
    $Socket = [Net.WebSockets.ClientWebSocket]::new()
    try {
        $null = $Socket.ConnectAsync(
            [Uri]$BrowserMetadata.webSocketDebuggerUrl,
            [Threading.CancellationToken]::None
        ).GetAwaiter().GetResult()
        $Command = [Text.Encoding]::UTF8.GetBytes('{"id":1,"method":"Browser.close"}')
        $Segment = [ArraySegment[byte]]::new($Command)
        $null = $Socket.SendAsync(
            $Segment,
            [Net.WebSockets.WebSocketMessageType]::Text,
            $true,
            [Threading.CancellationToken]::None
        ).GetAwaiter().GetResult()
    } finally {
        $Socket.Dispose()
    }
    if (-not $Process.WaitForExit(30000)) {
        throw "The launcher did not exit after its application Chromium closed."
    }

    $Log = Join-Path $env:BEYOND_SLIDES_DATA_DIR "controller.log"
    $Diagnostics = if (Test-Path $Log) { Get-Content $Log -Raw } else { "" }
    if ($Diagnostics -notlike "*BeyondSlides application shutdown complete.*") {
        throw "The launcher did not let the controller shut down gracefully: $Diagnostics"
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

$ProcessDeadline = [DateTime]::UtcNow.AddSeconds(10)
do {
    $Survivors = @(Get-CimInstance Win32_Process | Where-Object {
        $_.CommandLine -like "*$Temporary*"
    })
    if ($Survivors.Count -eq 0) { break }
    Start-Sleep -Milliseconds 200
} while ([DateTime]::UtcNow -lt $ProcessDeadline)
if ($Survivors.Count -ne 0) {
    $Descriptions = $Survivors | ForEach-Object { "$($_.Name) ($($_.ProcessId))" }
    throw "The launcher smoke test left packaged processes running: $($Descriptions -join ', ')"
}

Write-Host "Windows launcher, Chromium, FFmpeg, ffprobe, PDFium, and BGE model smoke tests passed."
