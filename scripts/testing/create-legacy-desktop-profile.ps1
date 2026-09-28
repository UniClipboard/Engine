# 用官方发布的 Desktop 0.19.x CLI 生成升级矩阵使用的合成旧版资料快照。
#
# 只使用便携模式：资料与文件钥匙串都写在 CLI 所在目录的 data\ 下，不接触系统凭据管理器或
# 已安装应用。历史内容写入当前 SSH/控制台会话自己的剪贴板，写入前先清空，写入后逐条核对。
# 生成结果与步骤见 tests/upgrade-matrix/fixtures/README.md。
param(
  # 含 cli\uniclip.exe 与 cli\uniclipd.exe（已按发布 SHA256SUMS 校验）的专属工作目录。
  [Parameter(Mandatory = $true)][string]$Root,
  # 为空时使用默认 profile，与多数真实用户的目录布局一致。
  [string]$Profile = '',
  # 与连接测试宿主的固定口令一致，使需要手动解锁的路径同样可测。
  [string]$Passphrase = 'connection-recovery-synthetic-passphrase',
  [int]$Entries = 3,
  # 命名 profile 的端口由 0.19.x 按名称的 FNV-1a 哈希推导；默认 profile 为 42715。
  [int]$Port = 0
)
$ErrorActionPreference = 'Stop'
Set-Location $Root
$env:UC_PORTABLE = '1'
if ($Profile) {
  $env:UC_PROFILE = $Profile
  $appDir = "app.uniclipboard.desktop-$Profile"
} else {
  Remove-Item Env:UC_PROFILE -ErrorAction SilentlyContinue
  $appDir = 'app.uniclipboard.desktop'
}
$data = Join-Path $Root "cli\data\$appDir"
if (Test-Path $data) { throw "profile already exists: $data" }
$port = if ($Port) { $Port } elseif (-not $Profile) { 42715 } else { throw 'pass -Port for a named profile' }
$base = "http://127.0.0.1:$port"
Add-Type -AssemblyName System.Windows.Forms

function Invoke-Cli([string[]]$Arguments, [string]$Log) {
  $process = Start-Process -FilePath "$Root\cli\uniclip.exe" -ArgumentList $Arguments -PassThru -NoNewWindow `
    -RedirectStandardOutput "$Log.out" -RedirectStandardError "$Log.err"
  # 先取句柄，否则进程结束后 ExitCode 为空。
  $null = $process.Handle
  if (-not $process.WaitForExit(120000)) { Stop-Process -Id $process.Id -Force; throw "$Log timed out" }
  return $process.ExitCode
}

function Set-SessionClipboard([string]$Text) {
  for ($attempt = 0; $attempt -lt 5; $attempt++) {
    try { [System.Windows.Forms.Clipboard]::SetDataObject($Text, $true, 10, 200); return } catch { Start-Sleep 1 }
  }
  throw 'session clipboard is unavailable'
}

# oneshot daemon 在最后一个控制连接断开后自行正常退出；等它退出后才能开始下一步。
function Wait-OwnDaemonExit {
  for ($i = 0; $i -lt 60; $i++) {
    if (-not (Get-Process uniclipd -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$Root\*" })) { return }
    Start-Sleep 1
  }
  throw 'fixture daemon did not exit'
}

$init = Invoke-Cli @('init', '--passphrase', $Passphrase, '--device-name', 'LegacyFixture') "$Root\init"
if ($init -ne 0) { throw "init failed with exit code $init" }
Wait-OwnDaemonExit
[System.Windows.Forms.Clipboard]::Clear()
# `watch` 持有 oneshot daemon 的控制租约；结束它即让 daemon 正常关闭，得到与用户退出应用一致的资料。
$watch = Start-Process -FilePath "$Root\cli\uniclip.exe" -ArgumentList @('watch') -PassThru -NoNewWindow `
  -RedirectStandardOutput "$Root\watch.out" -RedirectStandardError "$Root\watch.err"
$null = $watch.Handle
for ($i = 0; $i -lt 60 -and -not (Test-Path (Join-Path $data '.daemon-token')); $i++) { Start-Sleep 1 }
for ($i = 0; $i -lt 30; $i++) { try { Invoke-WebRequest -UseBasicParsing "$base/health" -TimeoutSec 2 | Out-Null; break } catch { Start-Sleep 1 } }
$secret = (Get-Content (Join-Path $data '.daemon-token') -Raw).Trim()
$body = @{ pid = $PID; clientType = 'gui' } | ConvertTo-Json
$session = (Invoke-RestMethod -Method Post "$base/auth/connect" -Headers @{ Authorization = "Bearer $secret" } `
  -ContentType 'application/json' -Body $body).data.sessionToken
$headers = @{ Authorization = "Session $session" }
Invoke-RestMethod -Method Post "$base/lifecycle/ready" -Headers $headers | Out-Null
Start-Sleep 3
for ($i = 1; $i -le $Entries; $i++) {
  Set-SessionClipboard "legacy fixture text $i"
  Start-Sleep 2
}
Start-Sleep 2
$history = Invoke-RestMethod "$base/search/query?query=&limit=100" -Headers $headers
$expected = 1..$Entries | ForEach-Object { "legacy fixture text $_" }
$actual = @($history.data.items | ForEach-Object { $_.textPreview }) | Sort-Object
if ($history.data.total -ne $Entries -or (Compare-Object $expected $actual)) {
  Stop-Process -Id $watch.Id -Force
  throw "history does not contain exactly the synthetic entries"
}
"entries=$($history.data.total)"
Stop-Process -Id $watch.Id -Force
Wait-OwnDaemonExit
$log = Get-ChildItem (Join-Path $Root 'cli\data\logs') -Filter 'uniclipboard-daemon.json.*' |
  Sort-Object LastWriteTime | Select-Object -Last 1
# init 与 watch 各启动一次 oneshot daemon，两次都必须正常关闭。
$stops = @(Select-String -Path $log.FullName -Pattern 'uniclipboard-daemon stopped').Count
if ($stops -ne 2) { throw "expected two clean daemon stops, found $stops" }
"profile=$data"
