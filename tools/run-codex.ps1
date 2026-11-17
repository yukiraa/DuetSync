# Run one non-interactive Codex task. Brief is read from a file and piped via stdin.
# Usage: tools\run-codex.ps1 -Model gpt-5.6-terra -Effort high -Sandbox workspace-write `
#          -Brief briefs\T2.md -Out logs\T2.last.md -Log logs\T2.log
param(
  [Parameter(Mandatory)][string]$Model,
  [string]$Effort = "high",
  [ValidateSet("read-only", "workspace-write")][string]$Sandbox = "read-only",
  [Parameter(Mandatory)][string]$Brief,
  [Parameter(Mandatory)][string]$Out,
  [Parameter(Mandatory)][string]$Log
)
$cx = (Get-ChildItem "$env:LOCALAPPDATA\OpenAI\Codex\bin\*\codex.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
if (-not $cx) { Write-Error "codex.exe not found (install the Codex app or CLI)"; exit 2 }
$root = Split-Path $PSScriptRoot -Parent
$start = Get-Date
Get-Content $Brief -Raw -Encoding UTF8 | & $cx exec -m $Model -c "model_reasoning_effort=`"$Effort`"" -s $Sandbox -C $root -o $Out - *> $Log
$code = $LASTEXITCODE
$tok = Select-String -Path $Log -Pattern "^tokens used" -Context 0, 1 | Select-Object -Last 1 | ForEach-Object { $_.Context.PostContext[0].Trim() }
"codex exit=$code model=$Model effort=$Effort sandbox=$Sandbox minutes=$([math]::Round(((Get-Date) - $start).TotalMinutes, 1)) tokens=$tok"
