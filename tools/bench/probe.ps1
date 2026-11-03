# Native barcode probe (no browser needed), same encoding as probe.html:
# [red] 32 bits of epoch ms + 8-bit checksum [green]. Used on hosts without a browser.
param([int]$X = 40, [int]$Y = 40)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$CELL = 20; $H = 64; $PAD = 8; $BITS = 40
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Tether probe'
$form.FormBorderStyle = 'FixedToolWindow'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point($X, $Y)
$form.ClientSize = New-Object System.Drawing.Size(($PAD * 2 + $CELL * ($BITS + 2)), ($H + $PAD * 2))
$form.TopMost = $true
$form.BackColor = [System.Drawing.Color]::Gray
$prop = $form.GetType().GetProperty('DoubleBuffered', [System.Reflection.BindingFlags]'Instance,NonPublic')
$prop.SetValue($form, $true, $null)

$form.Add_Paint({
  param($s, $e)
  $g = $e.Graphics
  # Int64 modulo: 0xFFFFFFFF is Int32 -1 in PowerShell, and casting the full ms count overflows.
  $v = [uint32]([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() % [int64]4294967296)
  $sum = (($v -band 255) + (($v -shr 8) -band 255) + (($v -shr 16) -band 255) + (($v -shr 24) -band 255) + 0x5A) -band 255
  $g.FillRectangle([System.Drawing.Brushes]::Red, $PAD, $PAD, $CELL, $H)
  for ($i = 0; $i -lt 32; $i++) {
    $b = ($v -shr (31 - $i)) -band 1
    $g.FillRectangle($(if ($b) { [System.Drawing.Brushes]::Black } else { [System.Drawing.Brushes]::White }), $PAD + $CELL * ($i + 1), $PAD, $CELL, $H)
  }
  for ($i = 0; $i -lt 8; $i++) {
    $b = ($sum -shr (7 - $i)) -band 1
    $g.FillRectangle($(if ($b) { [System.Drawing.Brushes]::Black } else { [System.Drawing.Brushes]::White }), $PAD + $CELL * ($i + 33), $PAD, $CELL, $H)
  }
  $g.FillRectangle([System.Drawing.Brushes]::Lime, $PAD + $CELL * ($BITS + 1), $PAD, $CELL, $H)
})
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 1
$timer.Add_Tick({ $form.Invalidate() })
$timer.Start()
[System.Windows.Forms.Application]::Run($form)
