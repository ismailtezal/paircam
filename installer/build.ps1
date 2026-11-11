<#
.SYNOPSIS
    Builds the PairCam Windows installer: target\installer\PairCam-Setup-<version>-x64.exe.

.DESCRIPTION
    Works from a clean checkout in a plain PowerShell window, without administrator rights:

      1. Fetches FFmpeg into third_party\ffmpeg if it is missing (scripts\fetch-ffmpeg.ps1).
      2. Builds paircam.exe in release mode (cargo build --release -p paircam-desktop).
      3. Builds the virtual camera DLL (native\vcam\build.ps1).
      4. Fetches Inno Setup into third_party\innosetup-<version> if it is missing. Inno Setup's
         own installer is run in portable, per-user mode: nothing is installed machine-wide, no
         registry entries are written and Windows does not ask for administrator rights.
      5. Stages the files that ship (target\installer\stage), including the licence texts, and
         compiles installer\paircam.iss with Inno Setup's command-line compiler.

    The version comes from the workspace Cargo.toml (through cargo metadata).

.PARAMETER SkipBuild
    Reuses the existing release build of paircam.exe and the camera DLL instead of rebuilding
    them. For iterating on the installer script only.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File installer\build.ps1
#>
[CmdletBinding()]
param(
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$thirdParty = Join-Path $root 'third_party'
$output = Join-Path $root 'target\installer'
$stage = Join-Path $output 'stage'

# Inno Setup, pinned. The SHA-256 is the digest GitHub publishes for the release asset
# (https://api.github.com/repos/jrsoftware/issrc/releases/tags/is-7_1_1). The installer is also
# Authenticode-signed by its publisher, which is checked below as a second, independent check.
$innoVersion = '7.1.1'
$innoUrl = "https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/innosetup-$innoVersion-x64.exe"
$innoSha256 = '8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0'
$innoSigner = 'Pyrsys B.V.'
$innoDirectory = Join-Path $thirdParty "innosetup-$innoVersion"

function Invoke-Checked {
    param([string] $Tool, [string[]] $Arguments)
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Tool $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Get-InnoSetupCompiler {
    $compiler = Join-Path $innoDirectory 'ISCC.exe'
    if (Test-Path $compiler) {
        return $compiler
    }

    $downloads = Join-Path $thirdParty 'downloads'
    New-Item -ItemType Directory -Force -Path $downloads | Out-Null
    $setup = Join-Path $downloads "innosetup-$innoVersion-x64.exe"

    Write-Host "Downloading Inno Setup $innoVersion"
    Invoke-WebRequest -Uri $innoUrl -OutFile $setup -UseBasicParsing

    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $setup).Hash.ToLowerInvariant()
    if ($actual -ne $innoSha256) {
        Remove-Item -LiteralPath $setup -Force
        throw "Checksum mismatch for Inno Setup. Expected $innoSha256 but got $actual."
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $setup
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch [regex]::Escape($innoSigner)) {
        Remove-Item -LiteralPath $setup -Force
        throw "Inno Setup's installer is not validly signed by $innoSigner ($($signature.Status))."
    }

    # /CURRENTUSER runs the installer without elevation (it allows that override), and
    # /PORTABLE=1 makes it a plain copy into /DIR: no uninstaller, registry entries, shortcuts or
    # file associations. Start-Process -Wait because the installer detaches from the console.
    Write-Host "Unpacking Inno Setup into $innoDirectory"
    $log = Join-Path $downloads "innosetup-$innoVersion-install.log"
    $arguments = @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOICONS', '/CURRENTUSER', '/PORTABLE=1',
        "/DIR=`"$innoDirectory`"", "/LOG=`"$log`""
    )
    $process = Start-Process -FilePath $setup -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0 -or -not (Test-Path $compiler)) {
        throw "Unpacking Inno Setup failed (exit code $($process.ExitCode)); see $log."
    }
    Remove-Item -LiteralPath $setup -Force
    return $compiler
}

function Get-Metadata {
    # Normal (shipping) dependencies for the Windows target only.
    $json = & cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc `
        --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE."
    }
    return ($json -join "`n") | ConvertFrom-Json
}

# The crates compiled into paircam.exe: everything reachable from paircam-desktop through normal
# dependencies. Build dependencies and procedural macros run only at compile time and do not ship.
function Get-ShippedPackages($Metadata) {
    $packages = @{}
    foreach ($package in $Metadata.packages) {
        $packages[$package.id] = $package
    }
    $nodes = @{}
    foreach ($node in $Metadata.resolve.nodes) {
        $nodes[$node.id] = $node
    }

    $start = ($Metadata.packages | Where-Object { $_.name -eq 'paircam-desktop' }).id
    $seen = @{ $start = $true }
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($start)
    while ($queue.Count -gt 0) {
        $node = $nodes[$queue.Dequeue()]
        foreach ($dependency in $node.deps) {
            $normal = @($dependency.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0
            if ($normal -and -not $seen.ContainsKey($dependency.pkg)) {
                $seen[$dependency.pkg] = $true
                $queue.Enqueue($dependency.pkg)
            }
        }
    }

    return $seen.Keys |
        ForEach-Object { $packages[$_] } |
        Where-Object { $_.source } | # crates from crates.io or git, not this workspace
        Where-Object { @($_.targets | Where-Object { $_.kind -notcontains 'proc-macro' }).Count -gt 0 } |
        Sort-Object name, version
}

function Get-LicenseFiles($Package) {
    $directory = Split-Path -Parent $Package.manifest_path
    $files = @(Get-ChildItem -LiteralPath $directory -File |
            Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT)' } |
            Sort-Object Name)
    if ($Package.license_file) {
        $declared = Join-Path $directory $Package.license_file
        if ((Test-Path -LiteralPath $declared) -and -not ($files.FullName -contains (Resolve-Path $declared).Path)) {
            $files += Get-Item -LiteralPath $declared
        }
    }
    return $files
}

# THIRD-PARTY-NOTICES.txt: every crate in paircam.exe with its licence and the licence texts it
# ships. Identical texts are printed once, followed by the crates they belong to.
function Write-RustNotices($Packages, [string] $Path) {
    $texts = [ordered]@{}
    $summary = New-Object System.Text.StringBuilder
    foreach ($package in $Packages) {
        $license = if ($package.license) { $package.license } else { 'see licence file' }
        [void]$summary.AppendLine("  $($package.name) $($package.version)  ($license)")
        foreach ($file in Get-LicenseFiles $package) {
            $text = (Get-Content -Raw -LiteralPath $file.FullName).Trim()
            if (-not $texts.Contains($text)) {
                $texts[$text] = New-Object System.Collections.Generic.List[string]
            }
            $texts[$text].Add("$($package.name) $($package.version) ($($file.Name))")
        }
    }

    $builder = New-Object System.Text.StringBuilder
    [void]$builder.AppendLine('PairCam includes the following Rust crates, linked into paircam.exe.')
    [void]$builder.AppendLine('Their licence texts follow the list.')
    [void]$builder.AppendLine()
    [void]$builder.Append($summary.ToString())
    $rule = '=' * 79
    foreach ($entry in $texts.GetEnumerator()) {
        [void]$builder.AppendLine()
        [void]$builder.AppendLine($rule)
        foreach ($owner in $entry.Value) {
            [void]$builder.AppendLine($owner)
        }
        [void]$builder.AppendLine($rule)
        [void]$builder.AppendLine()
        [void]$builder.AppendLine($entry.Key)
    }
    [IO.File]::WriteAllText($Path, $builder.ToString(), (New-Object Text.UTF8Encoding $false))
}

# Copies one crate's licence files into the licences folder under a readable name.
function Copy-CrateLicense($Packages, [string] $Name, [string] $Title, [string] $Destination) {
    $package = $Packages | Where-Object { $_.name -eq $Name } | Select-Object -First 1
    if (-not $package) {
        throw "$Name is not among paircam.exe's dependencies; update installer\build.ps1."
    }
    $files = @(Get-LicenseFiles $package)
    if ($files.Count -eq 0) {
        throw "$Name $($package.version) has no licence file in its package."
    }
    foreach ($file in $files) {
        $suffix = [IO.Path]::GetFileNameWithoutExtension($file.Name)
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $Destination "$Title-$suffix.txt")
    }
}

# The FFmpeg DLLs the engine loads, as listed in crates\engine\build.rs (one source of truth).
function Get-FfmpegRuntimeDlls {
    $source = Get-Content -Raw (Join-Path $root 'crates\engine\build.rs')
    $block = [regex]::Match($source, 'RUNTIME_DLLS[^=]*=\s*\[(?<list>[^\]]*)\]')
    if (-not $block.Success) {
        throw 'RUNTIME_DLLS was not found in crates\engine\build.rs.'
    }
    $dlls = [regex]::Matches($block.Groups['list'].Value, '"([^"]+\.dll)"') | ForEach-Object { $_.Groups[1].Value }
    if (@($dlls).Count -eq 0) {
        throw 'RUNTIME_DLLS in crates\engine\build.rs lists no DLLs.'
    }
    return $dlls
}

# Fails if a staged binary imports a DLL that is neither staged nor part of Windows. Needs
# dumpbin, which is on PATH after native\vcam\build.ps1 has loaded the Visual Studio environment.
function Test-StagedImports([string] $Directory) {
    if (-not (Get-Command dumpbin.exe -ErrorAction SilentlyContinue)) {
        Write-Warning 'dumpbin.exe is not on PATH; skipping the DLL dependency check.'
        return
    }
    $staged = @(Get-ChildItem -LiteralPath $Directory -File | ForEach-Object { $_.Name.ToLowerInvariant() })
    foreach ($binary in Get-ChildItem -LiteralPath $Directory -File -Include *.exe, *.dll -Recurse) {
        $lines = & dumpbin.exe /nologo /dependents $binary.FullName
        $imports = $lines | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim().ToLowerInvariant() }
        foreach ($import in $imports) {
            if ($staged -contains $import) {
                continue
            }
            $system = Join-Path $env:SystemRoot "System32\$import"
            if ((Test-Path $system) -or $import.StartsWith('api-ms-') -or $import.StartsWith('ext-ms-')) {
                continue
            }
            throw "$($binary.Name) needs $import, which is neither in the installer nor part of Windows."
        }
    }
}

# A short note on where the FFmpeg build and its source come from (LGPL section 6 asks for the
# source to be obtainable). The build is pinned in scripts\fetch-ffmpeg.ps1.
function Write-FfmpegNotice([string] $Path) {
    $fetch = Get-Content -Raw (Join-Path $root 'scripts\fetch-ffmpeg.ps1')
    $tag = [regex]::Match($fetch, '\$tag\s*=\s*"([^"]+)"').Groups[1].Value
    $archive = [regex]::Match($fetch, '\$archive\s*=\s*"([^"]+)"').Groups[1].Value
    $commit = [regex]::Match($archive, '-g([0-9a-f]{7,40})-').Groups[1].Value
    $text = @"
PairCam uses FFmpeg (https://ffmpeg.org) to decode video. FFmpeg is licensed under the GNU Lesser
General Public License (LGPL) version 2.1 or later; its licence is in FFmpeg-LICENSE.txt.

PairCam links to FFmpeg dynamically. The FFmpeg DLLs in this folder are unmodified builds from
BtbN/FFmpeg-Builds (the Windows builds linked from ffmpeg.org), LGPL "shared" variant:

  Build:   https://github.com/BtbN/FFmpeg-Builds/releases/tag/$tag
  Archive: $archive
  Source:  https://github.com/FFmpeg/FFmpeg/commit/$commit
  Build scripts: https://github.com/BtbN/FFmpeg-Builds

You may replace these DLLs with your own compatible build of FFmpeg.
"@
    [IO.File]::WriteAllText($Path, $text, (New-Object Text.UTF8Encoding $false))
}

# ---------------------------------------------------------------------------------------------
# 1-3. Build what ships
# ---------------------------------------------------------------------------------------------

if (-not (Test-Path (Join-Path $thirdParty 'ffmpeg\include\libavcodec\avcodec.h'))) {
    Write-Host 'Fetching FFmpeg'
    & (Join-Path $root 'scripts\fetch-ffmpeg.ps1')
}

if (-not $SkipBuild) {
    Write-Host 'Building paircam.exe (release)'
    Invoke-Checked cargo @('build', '--release', '--locked', '-p', 'paircam-desktop', '--manifest-path', (Join-Path $root 'Cargo.toml'))
}
# Building the camera DLL also loads the Visual Studio environment into this process, which
# makes dumpbin available for the dependency check below.
if (-not $SkipBuild -or -not (Test-Path (Join-Path $root 'target\vcam\paircam_vcam.dll'))) {
    Write-Host 'Building the camera DLL'
    & (Join-Path $root 'native\vcam\build.ps1')
}

$compiler = Get-InnoSetupCompiler

# ---------------------------------------------------------------------------------------------
# 4. Stage the files that ship
# ---------------------------------------------------------------------------------------------

$metadata = Get-Metadata
$desktop = $metadata.packages | Where-Object { $_.name -eq 'paircam-desktop' }
$version = $desktop.version
# Windows version resources take four numbers; a pre-release suffix stays in the string only.
$numericVersion = ($version -replace '[-+].*$', '') + '.0'
$copyright = (Get-Content (Join-Path $root 'LICENSE') | Where-Object { $_ -like 'Copyright*' } | Select-Object -First 1)
$repositoryUrl = if ($desktop.repository) { $desktop.repository } else { '' }

Write-Host "Staging PairCam $version"
if (Test-Path $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
$appStage = Join-Path $stage 'app'
$cameraStage = Join-Path $stage 'camera'
$licenses = Join-Path $appStage 'licenses'
New-Item -ItemType Directory -Force -Path $appStage, $cameraStage, $licenses | Out-Null

$release = Join-Path $root 'target\release'
Copy-Item -LiteralPath (Join-Path $release 'paircam.exe') -Destination $appStage
foreach ($dll in Get-FfmpegRuntimeDlls) {
    Copy-Item -LiteralPath (Join-Path $thirdParty "ffmpeg\bin\$dll") -Destination $appStage
}
Copy-Item -LiteralPath (Join-Path $root 'target\vcam\paircam_vcam.dll') -Destination $cameraStage

$packages = @(Get-ShippedPackages $metadata)
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $licenses 'LICENSE-PairCam.txt')
Copy-Item -LiteralPath (Join-Path $thirdParty 'ffmpeg\LICENSE.txt') -Destination (Join-Path $licenses 'FFmpeg-LICENSE.txt')
Write-FfmpegNotice (Join-Path $licenses 'FFmpeg-README.txt')
Copy-Item -LiteralPath (Join-Path $root 'crates\ui\assets\fonts\OFL.txt') -Destination (Join-Path $licenses 'Geist-OFL.txt')
Copy-CrateLicense $packages 'gpui-pre' 'GPUI' $licenses
# The gpui-kit crate itself ships no licence file; gpui-component, from the same repository
# (longbridge/gpui-kit) and under the same Apache-2.0 licence, does.
Copy-CrateLicense $packages 'gpui-component' 'GPUI-Kit' $licenses
Write-RustNotices $packages (Join-Path $licenses 'THIRD-PARTY-NOTICES.txt')

Test-StagedImports $appStage
Test-StagedImports $cameraStage

# ---------------------------------------------------------------------------------------------
# 5. Compile the installer
# ---------------------------------------------------------------------------------------------

Write-Host 'Compiling the installer'
New-Item -ItemType Directory -Force -Path $output | Out-Null
$setupName = "PairCam-Setup-$version-x64.exe"
$setupPath = Join-Path $output $setupName
if (Test-Path $setupPath) {
    Remove-Item -LiteralPath $setupPath -Force
}

# Inno Setup has no "warnings as errors" switch. --messages-jsonl prints every warning and error
# as a JSON line on stderr, so any warning fails the build here instead.
$arguments = @(
    '--messages-jsonl',
    "/DAppVersion=$version",
    "/DAppVersionNumeric=$numericVersion",
    "/DStageDir=$stage",
    "/DOutputDir=$output",
    "/DCopyright=$copyright",
    "/DRepositoryUrl=$repositoryUrl",
    (Join-Path $PSScriptRoot 'paircam.iss')
)
$previousPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue' # stderr lines from a native tool are not PowerShell errors
$lines = & $compiler @arguments 2>&1 | ForEach-Object { "$_" }
$exitCode = $LASTEXITCODE
$ErrorActionPreference = $previousPreference

$problems = @()
foreach ($line in $lines) {
    $message = $null
    if ($line.TrimStart().StartsWith('{')) {
        try {
            $message = $line | ConvertFrom-Json
        } catch {
            $message = $null
        }
    }
    if ($message) {
        $problems += $message
        Write-Host $line -ForegroundColor Yellow
    } else {
        Write-Host $line
    }
}
if ($exitCode -ne 0) {
    throw "The Inno Setup compiler failed with exit code $exitCode."
}
if ($problems.Count -gt 0) {
    Remove-Item -LiteralPath $setupPath -Force -ErrorAction SilentlyContinue
    throw "The Inno Setup compiler reported $($problems.Count) warning(s); fix them (warnings are errors)."
}

$size = (Get-Item -LiteralPath $setupPath).Length
Write-Host ''
Write-Host ("Built {0} ({1:N1} MB)" -f $setupPath, ($size / 1MB)) -ForegroundColor Green
