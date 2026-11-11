<#
.SYNOPSIS
    Builds the PairCam virtual camera DLL and its test harness.

.DESCRIPTION
    Finds Visual Studio (2022 or later, with the C++ x64 tools) through vswhere, loads its x64
    build environment into this process, and compiles:

      target/vcam/paircam_vcam.dll        the Media Foundation camera source (release, x64,
                                          static C runtime so nothing extra loads inside the
                                          Frame Server service)
      target/vcam/paircam_vcam_test.exe   the test harness (no administrator rights needed)

    Works from a plain PowerShell window; no Developer prompt needed.

.PARAMETER Test
    Runs the test harness after a successful build.

.EXAMPLE
    ./native/vcam/build.ps1 -Test
#>
[CmdletBinding()]
param(
    [switch] $Test
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$source = $PSScriptRoot
$root = Resolve-Path (Join-Path $source '..\..')
$output = Join-Path $root 'target\vcam'
$objects = Join-Path $output 'obj'

function Import-VisualStudioEnvironment {
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
        return
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw 'vswhere.exe was not found. Install Visual Studio with "Desktop development with C++".'
    }
    $installation = & $vswhere -latest -products * -prerelease `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    if (-not $installation) {
        throw 'No Visual Studio installation with the x64 C++ tools was found.'
    }

    $vcvars = Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path $vcvars)) {
        throw "vcvars64.bat was not found at $vcvars."
    }

    # Run vcvars64 in cmd and copy the resulting environment into this PowerShell process.
    $lines = & cmd.exe /d /s /c "`"$vcvars`" >nul 2>&1 && set"
    if ($LASTEXITCODE -ne 0) {
        throw "vcvars64.bat failed with exit code $LASTEXITCODE."
    }
    foreach ($line in $lines) {
        $separator = $line.IndexOf('=')
        if ($separator -gt 0) {
            Set-Item -Path ("env:" + $line.Substring(0, $separator)) -Value $line.Substring($separator + 1)
        }
    }
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        throw 'cl.exe is still not on PATH after loading the Visual Studio environment.'
    }
}

function Invoke-Tool {
    param([string] $Tool, [string[]] $Arguments)
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Tool failed with exit code $LASTEXITCODE."
    }
}

Import-VisualStudioEnvironment
New-Item -ItemType Directory -Force -Path $output, (Join-Path $objects 'dll'), (Join-Path $objects 'test') | Out-Null

# Shared compiler switches: C++20, warnings at level 4 treated as errors, static CRT, Control
# Flow Guard and Spectre-safe code generation for a DLL that runs inside a Windows service.
$common = @(
    '/nologo', '/std:c++20', '/permissive-', '/EHsc', '/W4', '/WX', '/utf-8',
    '/O2', '/MT', '/Zi', '/guard:cf', '/Gy', '/Gw',
    '/DUNICODE', '/D_UNICODE', '/DWIN32_LEAN_AND_MEAN', '/DNOMINMAX', '/D_WIN32_WINNT=0x0A00',
    "/I$(Join-Path $source 'include')"
)
$linkCommon = @('/DEBUG', '/OPT:REF', '/OPT:ICF', '/DYNAMICBASE', '/NXCOMPAT', '/guard:cf')

Write-Host 'Building paircam_vcam.dll'
$dllSources = Get-ChildItem (Join-Path $source 'src\*.cpp') | ForEach-Object { $_.FullName }
Invoke-Tool cl.exe ($common + @(
    '/LD', "/Fo$(Join-Path $objects 'dll')\", "/Fd$(Join-Path $objects 'dll')\vc.pdb"
) + $dllSources + @(
    '/link'
) + $linkCommon + @(
    "/DEF:$(Join-Path $source 'src\paircam_vcam.def')",
    "/OUT:$(Join-Path $output 'paircam_vcam.dll')",
    "/PDB:$(Join-Path $output 'paircam_vcam.pdb')",
    "/IMPLIB:$(Join-Path $objects 'dll\paircam_vcam.lib')",
    'mfplat.lib', 'mfuuid.lib', 'mfsensorgroup.lib', 'ole32.lib', 'propsys.lib', 'advapi32.lib',
    'd2d1.lib', 'dwrite.lib', 'windowscodecs.lib'
))

Write-Host 'Building paircam_vcam_test.exe'
Invoke-Tool cl.exe ($common + @(
    "/Fo$(Join-Path $objects 'test')\", "/Fd$(Join-Path $objects 'test')\vc.pdb",
    (Join-Path $source 'test\vcam_test.cpp'),
    '/link'
) + $linkCommon + @(
    "/OUT:$(Join-Path $output 'paircam_vcam_test.exe')",
    "/PDB:$(Join-Path $output 'paircam_vcam_test.pdb')",
    'mfplat.lib', 'mfuuid.lib', 'ole32.lib', 'advapi32.lib', 'winmm.lib'
))

Write-Host "Built $(Join-Path $output 'paircam_vcam.dll')"

if ($Test) {
    & (Join-Path $output 'paircam_vcam_test.exe') (Join-Path $output 'paircam_vcam.dll')
    if ($LASTEXITCODE -ne 0) {
        throw "The virtual camera tests failed (exit code $LASTEXITCODE)."
    }
}
