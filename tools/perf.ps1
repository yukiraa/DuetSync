param([string]$Exe, [int]$Secs = 60)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class U { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c); }
"@
$cores = [Environment]::ProcessorCount
$p = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 8
function Measure-It($label) {
  $p.Refresh(); $c0 = $p.TotalProcessorTime.TotalMilliseconds; $t0 = Get-Date
  Start-Sleep -Seconds $Secs
  $p.Refresh(); $c1 = $p.TotalProcessorTime.TotalMilliseconds; $dt = ((Get-Date) - $t0).TotalMilliseconds
  $pct = ($c1 - $c0) / $dt / $cores * 100
  $pctOne = ($c1 - $c0) / $dt * 100
  $ctr = (Get-Counter "\Process($($p.ProcessName))\Working Set - Private" -ErrorAction SilentlyContinue).CounterSamples | Select-Object -First 1
  "{0}: CPU {1:N3} % of all {2} cores ({3:N2} % of one core), cpu-ms {4:N0} in {5:N0} ms, working set {6:N1} MB, private working set {7:N1} MB, private bytes {8:N1} MB, threads {9}" -f $label, $pct, $cores, $pctOne, ($c1-$c0), $dt, ($p.WorkingSet64/1MB), ($ctr.CookedValue/1MB), ($p.PrivateMemorySize64/1MB), $p.Threads.Count
}
Measure-It "GUI idle, window visible"
[U]::ShowWindow($p.MainWindowHandle, 6) | Out-Null  # SW_MINIMIZE
Start-Sleep -Seconds 3
Measure-It "GUI idle, minimized"
Stop-Process -Id $p.Id
