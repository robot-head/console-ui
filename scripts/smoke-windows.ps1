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
    $app = Start-Process (Resolve-Path $Executable) -PassThru
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
    Write-Output 'Packaged app connected, inspected tasks/resources, and paused/resumed successfully.'
} finally {
    if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force }
    if ($demo -and -not $demo.HasExited) { Stop-Process -Id $demo.Id -Force }
}
