[CmdletBinding()]
param([string]$Executable = "$PSScriptRoot/../dist/console-ui-win-x64/console-ui.exe")
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Path "$PSScriptRoot/../dist" -Force | Out-Null
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NativeSmokeWindow {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
}
'@
$demo = $null
$app = $null
function Wait-For([scriptblock]$Check, [string]$Description) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $result = & $Check
        if ($result) { return $result }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out waiting for $Description"
}
function Find-Name([string]$Name) {
    $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    $script:root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}
function Invoke-Name([string]$Name) {
    $element = Wait-For {
        $candidate = Find-Name $Name
        if ($candidate -and $candidate.Current.IsEnabled) { $candidate }
    } $Name
    $pattern = $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $pattern.Invoke()
}
function Set-Text([string]$Name, [string]$Value) {
    $condition = [System.Windows.Automation.AndCondition]::new(
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name),
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit))
    # WinUI delivers TextChanged asynchronously. Let the controlled field survive
    # two Workbench refreshes before invoking a button that reads its model value.
    Wait-For {
        $element = $script:root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
        if (-not $element) { return $false }
        $pattern = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        if ($pattern.Current.Value -ne $Value) {
            $pattern.SetValue($Value)
            return $false
        }
        Start-Sleep -Milliseconds 600
        $element = $script:root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
        $element -and $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $Value
    } "$Name to retain its entered value" | Out-Null
}
function Capture([string]$Name) {
    $bounds = $script:root.Current.BoundingRectangle
    $bitmap = [System.Drawing.Bitmap]::new([int]$bounds.Width, [int]$bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen([int]$bounds.X, [int]$bounds.Y, 0, 0, $bitmap.Size)
        $bitmap.Save("$PSScriptRoot/../dist/smoke-$Name.png", [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
try {
    $demo = Start-Process "$PSScriptRoot/../target/debug/examples/instrumented.exe" -PassThru
    $capture = "$PSScriptRoot/../dist/smoke-native-recording.tcui"
    $env:CONSOLE_UI_TRACE_SCROLL = '1'
    $appError = "$PSScriptRoot/../dist/smoke-app-error.log"
    $app = Start-Process (Resolve-Path $Executable) -ArgumentList @('--record', $capture) -PassThru -RedirectStandardError $appError
    $script:root = Wait-For {
        $app.Refresh()
        if ($app.HasExited) { throw "App exited with $($app.ExitCode)" }
        if ($app.MainWindowHandle -ne 0 -and $app.MainWindowTitle -like '*Tokio diagnostics*') {
            [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
        }
    } 'native Reactor window'
    Wait-For { Find-Name 'CONNECTED' } 'telemetry connection' | Out-Null
    $table = Wait-For { Find-Name 'Live telemetry table' } 'task table'
    $itemCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::ListItem)
    $first = Wait-For { $table.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $itemCondition) } 'task row'
    $first.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
    $firstId = $first.Current.Name.Split(',')[0]
    if ($firstId -notmatch '^ID [0-9]+$') { throw "Task row is missing its accessible entity ID: $firstId" }
    $transform = $script:root.GetCurrentPattern([System.Windows.Automation.TransformPattern]::Pattern)
    $transform.Move(0, 0)
    $transform.Resize(1180, 780)
    Wait-For {
        [NativeSmokeWindow]::SetForegroundWindow($app.MainWindowHandle) | Out-Null
        [NativeSmokeWindow]::GetForegroundWindow() -eq $app.MainWindowHandle
    } 'foreground native window for keyboard input' | Out-Null
    Wait-For {
        try {
            $row = $table.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $itemCondition)
            $row.SetFocus()
            $row.Current.HasKeyboardFocus
        } catch { $false }
    } 'native row keyboard focus after layout' | Out-Null
    Start-Sleep -Milliseconds 200
    [System.Windows.Forms.SendKeys]::SendWait('{END}')
    Wait-For {
        $selected = $table.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()
        $selected.Count -eq 1 -and $selected[0].Current.Name.Split(',')[0] -ne $firstId -and -not $selected[0].Current.IsOffscreen
    } 'keyboard-selected final row visible after scrolling' | Out-Null
    Capture 'keyboard-selection'
    [System.Windows.Forms.SendKeys]::SendWait('{HOME}')
    Wait-For {
        $selected = $table.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()
        $selected.Count -eq 1 -and -not $selected[0].Current.IsOffscreen
    } 'keyboard-selected first row visible' | Out-Null
    Invoke-Name 'Inspect selected'
    Wait-For { Find-Name 'Waker activity' } 'task details' | Out-Null
    Capture 'task-details'
    Invoke-Name 'Back  /  Esc'
    Invoke-Name 'Resources'
    Wait-For { Find-Name 'Kind' } 'resource headers' | Out-Null
    $table = Wait-For { Find-Name 'Live telemetry table' } 'resource table'
    $first = Wait-For { $table.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $itemCondition) } 'resource row'
    $first.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
    Invoke-Name 'Inspect selected'
    Wait-For { Find-Name "Inspect operation's task" } 'resource details' | Out-Null
    Capture 'resource-details'
    Invoke-Name 'Pause'
    Wait-For { Find-Name 'PAUSED' } 'paused state' | Out-Null
    Invoke-Name 'Resume'
    Wait-For { Find-Name 'CONNECTED' } 'resumed state' | Out-Null
    Invoke-Name 'Filters & lookup'
    Set-Text 'Filter expression' 'name:worker-0'
    Invoke-Name 'Apply filter'
    Invoke-Name 'Monitor'
    Invoke-Name 'Tasks'
    Wait-For {
        $filter = Find-Name 'Filter telemetry'
        $filter -and $filter.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'name:worker-0'
    } 'applied worker filter in the monitor' | Out-Null
    $table = Wait-For { Find-Name 'Live telemetry table' } 'filtered task table'
    Wait-For {
        $rows = $table.FindAll([System.Windows.Automation.TreeScope]::Descendants, $itemCondition)
        $rows.Count -eq 1 -and $rows[0].Current.Name.Contains('worker-0')
    } 'one filtered worker task' | Out-Null
    Capture 'filtered-task'
    Invoke-Name 'Analysis'
    Invoke-Name 'Apply time window'
    Wait-For { Find-Name 'Warning thresholds' } 'analysis controls'
    Capture 'analysis'
    $script:root.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close()
    if (-not $app.WaitForExit(10000)) { throw 'Native window did not shut down cleanly' }
    if ($app.ExitCode -ne 0) { throw "Native shutdown failed with $($app.ExitCode)" }
    $replay = "$PSScriptRoot/../dist/smoke-native-replay.json"
    if (-not (Test-Path $capture)) { throw 'Native recording file was not created' }
    $replayError = "$PSScriptRoot/../dist/smoke-native-replay-error.log"
    $replayProcess = Start-Process (Resolve-Path $Executable) -ArgumentList @('--replay', $capture, 'dump', '--output', $replay) -Wait -PassThru -RedirectStandardError $replayError
    if ($replayProcess.ExitCode -ne 0) { throw "Shutdown recording could not be replayed ($($replayProcess.ExitCode)): $(Get-Content $replayError -Raw)" }
    $snapshot = Get-Content $replay -Raw | ConvertFrom-Json
    if ($snapshot.tasks.Count -lt 1) { throw 'Shutdown recording lost its task data' }
    Write-Output 'Packaged app inspected tasks/resources, filtered a task, rendered analysis, paused/resumed, and flushed a replayable capture on native window close.'
} catch {
    try {
        $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
        Write-Output "Keyboard focus: $($focused.Current.Name); process $($focused.Current.ProcessId); automation ID $($focused.Current.AutomationId)"
        foreach ($name in @('Filter expression', 'Filter telemetry')) {
            $filter = Find-Name $name
            if ($filter -and $filter.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit) {
                $value = $filter.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
                Write-Output "Filter field ${name}: $value"
            }
        }
        if ($table) {
            $chosen = $table.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).Current.GetSelection()
            foreach ($row in $chosen) { Write-Output "Selected row: $($row.Current.Name); offscreen $($row.Current.IsOffscreen); automation ID $($row.Current.AutomationId)" }
        }
        Capture 'failure'
    } catch { Write-Output "Unable to capture smoke failure: $_" }
    if ($appError -and (Test-Path $appError)) { Get-Content $appError -Tail 30 | Write-Output }
    throw
} finally {
    if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force }
    if ($demo -and -not $demo.HasExited) { Stop-Process -Id $demo.Id -Force }
}
