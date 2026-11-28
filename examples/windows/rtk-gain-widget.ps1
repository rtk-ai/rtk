# Optional Windows notification-area widget for RTK gain statistics.
# Requires Windows PowerShell 5.1 and rtk on PATH.

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

function New-Color([int]$r, [int]$g, [int]$b) {
    [System.Drawing.Color]::FromArgb($r, $g, $b)
}

function Add-Text($parent, [string]$text, [int]$x, [int]$y, [int]$w, [int]$h, [float]$size, [System.Drawing.Color]$color, [bool]$bold = $false) {
    $style = if ($bold) { [System.Drawing.FontStyle]::Bold } else { [System.Drawing.FontStyle]::Regular }
    $control = New-Object System.Windows.Forms.Label
    $control.Text = $text
    $control.Location = New-Object System.Drawing.Point($x, $y)
    $control.Size = New-Object System.Drawing.Size($w, $h)
    $control.ForeColor = $color
    $control.BackColor = [System.Drawing.Color]::Transparent
    $control.Font = New-Object System.Drawing.Font('Segoe UI', $size, $style)
    $control.AutoEllipsis = $true
    $parent.Controls.Add($control)
    return $control
}

$background = New-Color 11 18 32
$headerColor = New-Color 16 27 45
$cardColor = New-Color 20 32 52
$cardAlt = New-Color 17 28 46
$white = New-Color 238 244 252
$muted = New-Color 142 158 180
$accent = New-Color 75 222 190
$blue = New-Color 111 168 255

$form = New-Object System.Windows.Forms.Form
$form.Text = 'RTK Gain'
$form.ClientSize = New-Object System.Drawing.Size(370, 344)
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(100, 100)
$form.TopMost = $false
$form.ShowInTaskbar = $false
$form.FormBorderStyle = 'FixedSingle'
$form.MaximizeBox = $false
$form.BackColor = $background
$form.ForeColor = $white
$form.Font = New-Object System.Drawing.Font('Segoe UI', 9)
$form.Icon = [System.Drawing.SystemIcons]::Information

$header = New-Object System.Windows.Forms.Panel
$header.Location = New-Object System.Drawing.Point(0, 0)
$header.Size = New-Object System.Drawing.Size(370, 68)
$header.BackColor = $headerColor
$form.Controls.Add($header)

$accentBar = New-Object System.Windows.Forms.Panel
$accentBar.Location = New-Object System.Drawing.Point(0, 0)
$accentBar.Size = New-Object System.Drawing.Size(4, 68)
$accentBar.BackColor = $accent
$header.Controls.Add($accentBar)

Add-Text $header 'RTK GAIN' 18 12 210 25 14 $white $true | Out-Null
Add-Text $header 'Token savings monitor' 19 37 220 18 9 $muted | Out-Null

$refreshButton = New-Object System.Windows.Forms.Button
$refreshButton.Text = 'Refresh'
$refreshButton.Location = New-Object System.Drawing.Point(284, 20)
$refreshButton.Size = New-Object System.Drawing.Size(70, 28)
$refreshButton.FlatStyle = 'Flat'
$refreshButton.FlatAppearance.BorderSize = 1
$refreshButton.FlatAppearance.BorderColor = (New-Color 48 68 91)
$refreshButton.BackColor = $cardColor
$refreshButton.ForeColor = $white
$refreshButton.Font = New-Object System.Drawing.Font('Segoe UI', 8)
$refreshButton.Cursor = [System.Windows.Forms.Cursors]::Hand
$header.Controls.Add($refreshButton)

$savedCard = New-Object System.Windows.Forms.Panel
$savedCard.Location = New-Object System.Drawing.Point(16, 84)
$savedCard.Size = New-Object System.Drawing.Size(338, 94)
$savedCard.BackColor = $cardColor
$form.Controls.Add($savedCard)
Add-Text $savedCard 'TOTAL TOKENS SAVED' 15 10 300 16 8 $muted $true | Out-Null
$savedValue = Add-Text $savedCard '0' 14 27 305 39 25 $accent $true
$savedCaption = Add-Text $savedCard 'Across 0 commands' 16 68 300 18 9 $muted

$commandsCard = New-Object System.Windows.Forms.Panel
$commandsCard.Location = New-Object System.Drawing.Point(16, 190)
$commandsCard.Size = New-Object System.Drawing.Size(164, 72)
$commandsCard.BackColor = $cardAlt
$form.Controls.Add($commandsCard)
Add-Text $commandsCard 'COMMANDS' 12 9 140 15 8 $muted $true | Out-Null
$commandsValue = Add-Text $commandsCard '0' 12 29 140 29 17 $white $true

$rateCard = New-Object System.Windows.Forms.Panel
$rateCard.Location = New-Object System.Drawing.Point(190, 190)
$rateCard.Size = New-Object System.Drawing.Size(164, 72)
$rateCard.BackColor = $cardAlt
$form.Controls.Add($rateCard)
Add-Text $rateCard 'AVG. SAVINGS' 12 9 140 15 8 $muted $true | Out-Null
$rateValue = Add-Text $rateCard '0.0%' 12 29 140 29 17 $blue $true

Add-Text $form 'SAVINGS RATE' 18 275 180 16 8 $muted $true | Out-Null
$ratePercent = Add-Text $form '0.0%' 294 275 55 16 8 $white $true
$barBack = New-Object System.Windows.Forms.Panel
$barBack.Location = New-Object System.Drawing.Point(18, 296)
$barBack.Size = New-Object System.Drawing.Size(336, 6)
$barBack.BackColor = (New-Color 37 51 70)
$form.Controls.Add($barBack)
$barFill = New-Object System.Windows.Forms.Panel
$barFill.Location = New-Object System.Drawing.Point(0, 0)
$barFill.Size = New-Object System.Drawing.Size(0, 6)
$barFill.BackColor = $accent
$barBack.Controls.Add($barFill)

$statusDot = New-Object System.Windows.Forms.Panel
$statusDot.Location = New-Object System.Drawing.Point(18, 320)
$statusDot.Size = New-Object System.Drawing.Size(7, 7)
$statusDot.BackColor = $accent
$form.Controls.Add($statusDot)
$updatedLabel = Add-Text $form 'Updated just now' 31 315 310 18 8 $muted

function Update-Stats {
    try {
        $output = & rtk gain --format json 2>$null
        if ($LASTEXITCODE -ne 0) { throw 'rtk gain returned an error.' }
        $json = ($output -join [Environment]::NewLine) | ConvertFrom-Json
        if ($null -eq $json.summary) { throw 'The RTK response did not contain summary data.' }

        $summary = $json.summary
        $saved = [double]$summary.total_saved
        $percent = [double]$summary.avg_savings_pct
        $commands = [double]$summary.total_commands
        $savedValue.Text = '{0:N0}' -f $saved
        $commandsValue.Text = '{0:N0}' -f $commands
        $rateValue.Text = '{0:N1}%' -f $percent
        $ratePercent.Text = '{0:N1}%' -f $percent
        $savedCaption.Text = 'Across {0:N0} commands' -f $commands
        $barWidth = [Math]::Max(0, [Math]::Min($barBack.Width, [int]($barBack.Width * ($percent / 100))))
        $barFill.Width = $barWidth
        $statusDot.BackColor = $accent
        $updatedLabel.Text = 'Updated ' + (Get-Date -Format 'HH:mm:ss')
    } catch {
        $savedValue.Text = 'Unavailable'
        $savedCaption.Text = "Check that 'rtk' is in PATH"
        $statusDot.BackColor = (New-Color 255 107 107)
        $updatedLabel.Text = 'Could not read RTK stats'
    }
}

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 300000
$timer.Add_Tick({ Update-Stats })
$refreshButton.Add_Click({ Update-Stats })

$script:exitRequested = $false
$appContext = New-Object System.Windows.Forms.ApplicationContext
$contextMenu = New-Object System.Windows.Forms.ContextMenuStrip
$openItem = $contextMenu.Items.Add('Open RTK Gain')
$refreshItem = $contextMenu.Items.Add('Refresh now')
[void]$contextMenu.Items.Add('-')
$exitItem = $contextMenu.Items.Add('Exit')

$notifyIcon = New-Object System.Windows.Forms.NotifyIcon
$notifyIcon.Text = 'RTK Gain - double-click to open'
$notifyIcon.Icon = [System.Drawing.SystemIcons]::Information
$notifyIcon.ContextMenuStrip = $contextMenu
$notifyIcon.Visible = $true

function Show-Widget {
    Update-Stats
    if (-not $form.Visible) { $form.Show() }
    if ($form.WindowState -eq [System.Windows.Forms.FormWindowState]::Minimized) {
        $form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
    }
    $form.Activate()
}

$openItem.Add_Click({ Show-Widget })
$refreshItem.Add_Click({ Update-Stats })
$notifyIcon.Add_MouseDoubleClick({ Show-Widget })
$exitItem.Add_Click({
    $script:exitRequested = $true
    $timer.Stop()
    $notifyIcon.Visible = $false
    $notifyIcon.Dispose()
    $form.Close()
    $appContext.ExitThread()
})
$form.Add_FormClosing({
    param($sender, $eventArgs)
    if (-not $script:exitRequested) {
        $eventArgs.Cancel = $true
        $form.Hide()
    } else {
        $timer.Stop()
        $notifyIcon.Visible = $false
    }
})

Update-Stats
$timer.Start()
[System.Windows.Forms.Application]::Run($appContext)
