[CmdletBinding()]
param([string]$Executable = "$PSScriptRoot/../dist/console-ui-win-x64/console-ui.exe")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms
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
    $element = Wait-For { $script:root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition) } $Name
    $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Value)
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
    $app = Start-Process (Resolve-Path $Executable) -ArgumentList @('--record', $capture) -PassThru
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
    $script:root.GetCurrentPattern([System.Windows.Automation.TransformPattern]::Pattern).Resize(1180, 780)
    $table.SetFocus()
    [System.Windows.Forms.SendKeys]::SendWait('{END}')
    Wait-For {
        $selected = $table.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).GetCurrentSelection()
        $selected.Count -eq 1 -and $selected[0].Current.Name.Split(',')[0] -ne $firstId -and -not $selected[0].Current.IsOffscreen
    } 'keyboard-selected final row visible after scrolling' | Out-Null
    Capture 'keyboard-selection'
    [System.Windows.Forms.SendKeys]::SendWait('{HOME}')
    Wait-For {
        $selected = $table.GetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern).GetCurrentSelection()
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
    $table = Wait-For { Find-Name 'Live telemetry table' } 'filtered task table'
    Wait-For { $table.FindAll([System.Windows.Automation.TreeScope]::Descendants, $itemCondition).Count -eq 1 } 'one filtered worker task' | Out-Null
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
} finally {
    if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force }
    if ($demo -and -not $demo.HasExited) { Stop-Process -Id $demo.Id -Force }
}
