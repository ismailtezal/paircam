<#
.SYNOPSIS
    Draws the PairCam mark and writes the application icon and the installer's wizard images.

.DESCRIPTION
    The mark is a near-black rounded square (a superellipse, like Apple's continuous corners)
    holding two overlapping rings. Where the rings overlap they form a lens: a camera lens, and
    two devices paired. Colours come from design\tokens\tokens.json.

    Every size is drawn directly at its pixel size rather than scaled down from a large image, so
    strokes stay crisp at 16 and 24 pixels.

    Writes, next to this script:
      paircam.png              1024x1024 source image
      paircam.ico              16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 pixels
      wizard-small-<n>.png     the installer's top-right image (transparent corners)
      wizard-<n>.png           the installer's welcome and finish panel, light appearance
      wizard-dark-<n>.png      the same panel, dark appearance

    The outputs are committed; run this again only when the mark or the tokens change.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File installer\assets\make-icon.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Add-Type -AssemblyName System.Drawing

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$tokens = Get-Content -Raw (Join-Path $root 'design\tokens\tokens.json') | ConvertFrom-Json

function ConvertTo-Color([string] $Hex) {
    $hex = $Hex.TrimStart('#')
    $alpha = if ($hex.Length -eq 8) { [Convert]::ToInt32($hex.Substring(6, 2), 16) } else { 255 }
    return [System.Drawing.Color]::FromArgb(
        $alpha,
        [Convert]::ToInt32($hex.Substring(0, 2), 16),
        [Convert]::ToInt32($hex.Substring(2, 2), 16),
        [Convert]::ToInt32($hex.Substring(4, 2), 16))
}

$tileColor = ConvertTo-Color $tokens.color.dark.surface
$tileBorder = ConvertTo-Color $tokens.color.dark.borderStrong
$ringColor = ConvertTo-Color $tokens.color.dark.textMuted
$lensColor = ConvertTo-Color $tokens.color.dark.text
$panelLight = ConvertTo-Color $tokens.color.light.surface
$panelDark = ConvertTo-Color $tokens.color.dark.background

# A superellipse |x/a|^n + |y/b|^n = 1 as a closed path. n = 5 is close to Apple's squircle.
function New-SquirclePath([float] $X, [float] $Y, [float] $Size) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $half = $Size / 2
    $centerX = $X + $half
    $centerY = $Y + $half
    $exponent = 5.0
    $points = New-Object 'System.Collections.Generic.List[System.Drawing.PointF]'
    for ($step = 0; $step -lt 360; $step++) {
        $angle = $step * [Math]::PI / 180
        $cos = [Math]::Cos($angle)
        $sin = [Math]::Sin($angle)
        $px = [Math]::Sign($cos) * [Math]::Pow([Math]::Abs($cos), 2 / $exponent) * $half
        $py = [Math]::Sign($sin) * [Math]::Pow([Math]::Abs($sin), 2 / $exponent) * $half
        $points.Add((New-Object System.Drawing.PointF ([float]($centerX + $px)), ([float]($centerY + $py))))
    }
    $path.AddPolygon($points.ToArray())
    return $path
}

# Draws the mark (tile, rings, lens) into a square of $Size pixels at ($X, $Y).
function Draw-Mark([System.Drawing.Graphics] $Graphics, [float] $X, [float] $Y, [float] $Size) {
    # Small icons get a thinner margin so the tile stays as large as possible.
    $margin = [Math]::Max(0.5, $Size / 16)
    $tileSize = $Size - 2 * $margin
    $tile = New-SquirclePath ($X + $margin) ($Y + $margin) $tileSize

    $brush = New-Object System.Drawing.SolidBrush $tileColor
    $Graphics.FillPath($brush, $tile)
    $brush.Dispose()

    # A hairline keeps the dark tile visible on a dark taskbar or desktop.
    $hairline = [Math]::Max(1.0, $Size / 128)
    $pen = New-Object System.Drawing.Pen ($tileBorder, [float]$hairline)
    $Graphics.DrawPath($pen, $tile)
    $pen.Dispose()
    $tile.Dispose()

    $centerX = $X + $Size / 2
    $centerY = $Y + $Size / 2
    $radius = $tileSize * 0.235
    $offset = $radius * 0.5
    $stroke = [Math]::Max(1.25, $Size * 0.036)

    # The lens where the two circles overlap: an arc of each circle between their two
    # intersection points.
    $height = [Math]::Sqrt($radius * $radius - $offset * $offset)
    $theta = [Math]::Atan2($height, $offset) * 180 / [Math]::PI
    $left = New-Object System.Drawing.RectangleF ([float]($centerX - $offset - $radius)), ([float]($centerY - $radius)), ([float](2 * $radius)), ([float](2 * $radius))
    $right = New-Object System.Drawing.RectangleF ([float]($centerX + $offset - $radius)), ([float]($centerY - $radius)), ([float](2 * $radius)), ([float](2 * $radius))
    $lens = New-Object System.Drawing.Drawing2D.GraphicsPath
    $lens.AddArc($left, [float](-$theta), [float](2 * $theta))
    $lens.AddArc($right, [float](180 - $theta), [float](2 * $theta))
    $lens.CloseFigure()
    $brush = New-Object System.Drawing.SolidBrush $lensColor
    $Graphics.FillPath($brush, $lens)
    $brush.Dispose()
    $lens.Dispose()

    $pen = New-Object System.Drawing.Pen ($ringColor, [float]$stroke)
    $Graphics.DrawEllipse($pen, $left)
    $Graphics.DrawEllipse($pen, $right)
    $pen.Dispose()
}

function New-Canvas([int] $Width, [int] $Height) {
    $bitmap = New-Object System.Drawing.Bitmap ($Width, $Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $graphics.Clear([System.Drawing.Color]::Transparent)
    return @($bitmap, $graphics)
}

function New-MarkBitmap([int] $Size) {
    $bitmap, $graphics = New-Canvas $Size $Size
    Draw-Mark $graphics 0 0 $Size
    $graphics.Dispose()
    return $bitmap
}

function Get-PngBytes([System.Drawing.Bitmap] $Bitmap) {
    $stream = New-Object System.IO.MemoryStream
    $Bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $stream.ToArray()
}

# An icon image as a 32-bit DIB: BITMAPINFOHEADER (height doubled for the AND mask), BGRA rows
# bottom-up with straight alpha, then a 1-bit AND mask (all zero: alpha decides).
function Get-DibBytes([System.Drawing.Bitmap] $Bitmap) {
    $size = $Bitmap.Width
    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $stream
    $writer.Write([int32]40)
    $writer.Write([int32]$size)
    $writer.Write([int32]($size * 2))
    $writer.Write([int16]1)
    $writer.Write([int16]32)
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    $writer.Write([int32]0)
    for ($y = $size - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $size; $x++) {
            $pixel = $Bitmap.GetPixel($x, $y)
            $writer.Write([byte]$pixel.B)
            $writer.Write([byte]$pixel.G)
            $writer.Write([byte]$pixel.R)
            $writer.Write([byte]$pixel.A)
        }
    }
    $maskRow = [int]([Math]::Ceiling($size / 32.0) * 4)
    $writer.Write((New-Object byte[] ($maskRow * $size)))
    $writer.Flush()
    return , $stream.ToArray()
}

function Write-Icon([string] $Path, [int[]] $Sizes) {
    # 256 is stored as PNG (smaller, and what Windows expects at that size); the rest as DIBs,
    # which every consumer of .ico files reads, including the resource compiler.
    $images = foreach ($size in $Sizes) {
        $bitmap = New-MarkBitmap $size
        $bytes = if ($size -ge 256) { Get-PngBytes $bitmap } else { Get-DibBytes $bitmap }
        $bitmap.Dispose()
        , @($size, $bytes)
    }

    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $stream
    $writer.Write([int16]0)
    $writer.Write([int16]1)
    $writer.Write([int16]$images.Count)
    $offset = 6 + 16 * $images.Count
    foreach ($image in $images) {
        $size = $image[0]
        $dimension = if ($size -ge 256) { 0 } else { $size } # 0 means 256 in an icon directory
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([int16]1)
        $writer.Write([int16]32)
        $writer.Write([int32]$image[1].Length)
        $writer.Write([int32]$offset)
        $offset += $image[1].Length
    }
    foreach ($image in $images) {
        $writer.Write([byte[]]$image[1])
    }
    $writer.Flush()
    [IO.File]::WriteAllBytes($Path, $stream.ToArray())
}

function Save-Png([System.Drawing.Bitmap] $Bitmap, [string] $Path) {
    $Bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
}

$assets = $PSScriptRoot

$source = New-MarkBitmap 1024
Save-Png $source (Join-Path $assets 'paircam.png')
$source.Dispose()

Write-Icon (Join-Path $assets 'paircam.ico') @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256)

# Inno Setup picks the best match among several sizes (its image areas at 100%, 150% and 250%).
foreach ($size in 58, 97, 159) {
    $bitmap = New-MarkBitmap $size
    Save-Png $bitmap (Join-Path $assets "wizard-small-$size.png")
    $bitmap.Dispose()
}

# The welcome and finish panel (aspect ratio 164:314): the mark, centred a little above the
# middle, on the page colour of each appearance.
foreach ($panel in @(@('', $panelLight), @('-dark', $panelDark))) {
    foreach ($width in 202, 336, 534) {
        $height = [int][Math]::Round($width * 314 / 164)
        $bitmap, $graphics = New-Canvas $width $height
        $graphics.Clear($panel[1])
        $mark = [Math]::Round($width * 0.42)
        Draw-Mark $graphics (($width - $mark) / 2) ($height * 0.42 - $mark / 2) $mark
        $graphics.Dispose()
        Save-Png $bitmap (Join-Path $assets "wizard$($panel[0])-$width.png")
        $bitmap.Dispose()
    }
}

Write-Host "Wrote the icon and wizard images to $assets"
