# Regenerate checked-in PNG/ICO assets from the Canvas P SVG master.
# Uses Windows System.Drawing only; no additional tools are required.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$assetDirectory = Join-Path $PSScriptRoot '../assets'
[xml]$svg = Get-Content -LiteralPath (Join-Path $assetDirectory 'logo.svg')
$master = New-Object System.Drawing.Bitmap 1024, 1024
$graphics = [System.Drawing.Graphics]::FromImage($master)
$graphics.SmoothingMode = 'AntiAlias'
$graphics.ScaleTransform(4, 4)
# Chocolate plate with transparent outer corners, as in the approved concept.
$plate = New-Object System.Drawing.Drawing2D.GraphicsPath
foreach ($arc in @(@(16,16,56,56,180,90), @(184,16,56,56,270,90), @(184,184,56,56,0,90), @(16,184,56,56,90,90))) {
    $plate.AddArc($arc[0], $arc[1], $arc[2], $arc[3], $arc[4], $arc[5])
}
$plate.CloseFigure()
$brush = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#3D2B28'))
$graphics.FillPath($brush, $plate)
$brush.Dispose()
$plate.Dispose()
foreach ($element in $svg.svg.path) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $tokens = $element.d -split '\s+'
    $index = 0
    [single]$x = 0; [single]$y = 0
    while ($index -lt $tokens.Length) {
        $command = $tokens[$index++]
        switch ($command) {
            'M' { $x = [single]::Parse($tokens[$index++], [cultureinfo]::InvariantCulture); $y = [single]::Parse($tokens[$index++], [cultureinfo]::InvariantCulture); $path.StartFigure() }
            'L' {
                $nx = [single]::Parse($tokens[$index++], [cultureinfo]::InvariantCulture)
                $ny = [single]::Parse($tokens[$index++], [cultureinfo]::InvariantCulture)
                $path.AddLine($x, $y, $nx, $ny); $x = $nx; $y = $ny
            }
            'C' {
                $points = @(1..6 | ForEach-Object { [single]::Parse($tokens[$index++], [cultureinfo]::InvariantCulture) })
                $path.AddBezier($x, $y, $points[0], $points[1], $points[2], $points[3], $points[4], $points[5])
                $x = $points[4]; $y = $points[5]
            }
            'Z' { $path.CloseFigure() }
            default { throw "Unsupported SVG command: $command" }
        }
    }
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml($element.fill))
    $graphics.FillPath($brush, $path)
    $brush.Dispose(); $path.Dispose()
}
$graphics.Dispose()
$sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$frames = @()
foreach ($size in $sizes) {
    $bitmap = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $g.DrawImage($master, 0, 0, $size, $size)
    $g.Dispose()
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $frames += ,$stream.ToArray()
    if ($size -eq 256) { $bitmap.Save((Join-Path $assetDirectory 'icon.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
    $bitmap.Dispose(); $stream.Dispose()
}
$master.Dispose()
$file = [System.IO.File]::Create((Join-Path $assetDirectory 'icon.ico'))
$writer = New-Object System.IO.BinaryWriter $file
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Length)
    $offset = 6 + 16 * $sizes.Length
    for ($i = 0; $i -lt $sizes.Length; $i++) {
        $dimension = if ($sizes[$i] -eq 256) { 0 } else { $sizes[$i] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$i].Length); $writer.Write([uint32]$offset)
        $offset += $frames[$i].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose() }
Write-Output 'Generated assets/icon.png and assets/icon.ico from assets/logo.svg.'
