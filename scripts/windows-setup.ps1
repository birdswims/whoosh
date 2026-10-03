# Build Whoosh.exe, the transfer engine, and dist\WhooshSetup.exe.
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path

function Import-VcVars {
    if (Get-Command link.exe -ErrorAction SilentlyContinue) {
        return
    }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) {
        throw "Visual C++ build tools are required to build the Whoosh engine."
    }
    $vcvars = Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat"
    cmd /c "`"$vcvars`" >nul && set" | ForEach-Object {
        if ($_ -match "^(.*?)=(.*)$") {
            Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
        }
    }
}

function New-WhooshIcon([string]$Path) {
    Add-Type -AssemblyName System.Drawing
    $dir = Split-Path $Path
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    $sizes = @(16, 24, 32, 48, 64, 256)
    $images = foreach ($size in $sizes) {
        $bitmap = New-Object System.Drawing.Bitmap $size, $size
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.Clear([System.Drawing.Color]::FromArgb(255, 19, 22, 20))
        foreach ($arc in @(
            @{ Radius = 0.40; Alpha = 90; Width = 0.072 },
            @{ Radius = 0.285; Alpha = 170; Width = 0.078 },
            @{ Radius = 0.165; Alpha = 255; Width = 0.086 }
        )) {
            $radius = $size * $arc.Radius
            $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb($arc.Alpha, 241, 246, 243)), ($size * $arc.Width)
            $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
            $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
            $cx = $size * 0.50
            $cy = $size * 0.62
            $graphics.DrawArc($pen, [single]($cx - $radius), [single]($cy - $radius), [single]($radius * 2), [single]($radius * 2), 206, 132)
            $pen.Dispose()
        }
        $graphics.Dispose()
        $stream = New-Object System.IO.MemoryStream
        $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        $bitmap.Dispose()
        , $stream.ToArray()
    }
    $file = [System.IO.File]::Open($Path, "Create")
    $writer = New-Object System.IO.BinaryWriter $file
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$images.Count)
    $offset = 6 + (16 * $images.Count)
    for ($index = 0; $index -lt $images.Count; $index++) {
        $dimension = if ($sizes[$index] -ge 256) { [byte]0 } else { [byte]$sizes[$index] }
        $writer.Write($dimension)
        $writer.Write($dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$images[$index].Length)
        $writer.Write([uint32]$offset)
        $offset += $images[$index].Length
    }
    foreach ($image in $images) {
        $writer.Write($image)
    }
    $writer.Dispose()
}

Write-Host "Drawing the icon"
New-WhooshIcon (Join-Path $Root "windows\Whoosh\Assets\AppIcon.ico")

Write-Host "Building the Whoosh engine"
Import-VcVars
cargo build --release --locked

Write-Host "Building the Windows app"
$stage = Join-Path $Root "dist\stage-win"
if (Test-Path $stage) {
    Remove-Item -Recurse -Force $stage
}
dotnet publish (Join-Path $Root "windows\Whoosh\Whoosh.csproj") -c Release -r win-x64 --self-contained true -o $stage -p:Platform=x64 -p:WindowsAppSDKSelfContained=true
Copy-Item (Join-Path $Root "target\release\whoosh.exe") (Join-Path $stage "whoosh-core.exe") -Force

Write-Host "Packing the installer"
$zip = Join-Path $Root "windows\Setup\payload.zip"
if (Test-Path $zip) {
    Remove-Item -Force $zip
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip)
$setupStage = Join-Path $Root "dist\setup-publish"
if (Test-Path $setupStage) {
    Remove-Item -Recurse -Force $setupStage
}
dotnet publish (Join-Path $Root "windows\Setup\Setup.csproj") -c Release -r win-x64 --self-contained true -o $setupStage
New-Item -ItemType Directory -Force -Path (Join-Path $Root "dist") | Out-Null
Copy-Item (Join-Path $setupStage "WhooshSetup.exe") (Join-Path $Root "dist\WhooshSetup.exe") -Force
Write-Host "Wrote dist\WhooshSetup.exe"
