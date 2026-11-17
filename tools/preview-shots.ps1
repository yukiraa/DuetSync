# Capture every guided-UI view via DUETSYNC_PREVIEW (navigation only, no fake data).
# Usage: tools\preview-shots.ps1 -Exe target\debug\DuetSync.exe -OutDir <folder>
param([string]$Exe = "target\debug\DuetSync.exe", [Parameter(Mandatory)][string]$OutDir, [int]$Delay = 3)
$root = Split-Path $PSScriptRoot -Parent
New-Item -ItemType Directory -Force $OutDir | Out-Null
$views = @("welcome") + (1..6 | ForEach-Object { "host:$_" }) + (1..5 | ForEach-Object { "guest:$_" })
foreach ($v in $views) {
  $env:DUETSYNC_PREVIEW = $v
  $p = Start-Process -FilePath (Join-Path $root $Exe) -PassThru
  Start-Sleep -Seconds $Delay
  $name = ($v -replace ':', '-') + ".png"
  powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "screenshot.ps1") -ProcId $p.Id -Out (Join-Path $OutDir $name) | Out-Null
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
}
Remove-Item Env:DUETSYNC_PREVIEW
Get-ChildItem $OutDir -Filter *.png | Select-Object Name, Length
