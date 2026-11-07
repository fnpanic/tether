# Quality bench (docs/performance.md, "Image quality vs AnyDesk"): grab the VM's own screen (console session) as ground truth -> C:\bench\perf\qref.png
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public static class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }'
[Dpi]::SetProcessDPIAware() | Out-Null
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save('C:\bench\perf\qref.png', [System.Drawing.Imaging.ImageFormat]::Png)
"$($b.Width)x$($b.Height)" | Set-Content C:\bench\perf\qref.txt
