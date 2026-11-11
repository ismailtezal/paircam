<#
.SYNOPSIS
    Installs the PairCam virtual camera on this computer (Windows 11). Needs administrator
    rights once; asks for them itself.

.DESCRIPTION
    1. Copies target\vcam\paircam_vcam.dll (built by native\vcam\build.ps1) to
       C:\Program Files\PairCam\. Windows' camera service (Frame Server, running as Local
       Service) cannot read files under a user profile; that is the "Access denied" other
       virtual cameras hit.
    2. Grants read and execute on that folder to Local Service and Local System explicitly.
    3. Registers the DLL machine-wide with regsvr32 (Frame Server only looks in HKLM).

    Run it again after rebuilding the DLL to update the installed copy. If a camera app has the
    old DLL loaded, the old file is moved aside and removed on a later run.

.PARAMETER NoPause
    Do not wait for Enter before closing the elevated window.

.EXAMPLE
    .\scripts\register-vcam.ps1
#>
[CmdletBinding()]
param(
    [switch] $NoPause
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$clsid = '{81785F8A-A3B5-4F45-BC7B-F92794EB45BB}' # PCVC_CLSID_STRING in paircam_vcam.h
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$source = Join-Path $root 'target\vcam\paircam_vcam.dll'
$installDirectory = Join-Path $env:ProgramFiles 'PairCam'
$installed = Join-Path $installDirectory 'paircam_vcam.dll'

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Wait-BeforeClosing {
    if (-not $NoPause) {
        Write-Host ''
        Read-Host 'Press Enter to close' | Out-Null
    }
}

# Re-launch elevated. The elevated copy does the work in its own window; this one reports how
# it went.
if (-not (Test-Administrator)) {
    Write-Host 'Asking Windows for administrator rights to install the PairCam camera...'
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    if ($NoPause) {
        $arguments += '-NoPause'
    }
    try {
        $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs `
            -Wait -PassThru
    } catch {
        Write-Host 'Installation cancelled: administrator rights were not granted.' -ForegroundColor Red
        exit 1
    }
    if ($process.ExitCode -eq 0) {
        Write-Host 'The PairCam camera is installed.' -ForegroundColor Green
    } else {
        Write-Host "Installing the PairCam camera failed (exit code $($process.ExitCode))." -ForegroundColor Red
    }
    exit $process.ExitCode
}

try {
    if ([Environment]::OSVersion.Version.Build -lt 22000) {
        throw 'Virtual cameras need Windows 11 (build 22000 or later).'
    }
    if (-not (Test-Path $source)) {
        throw "$source was not found. Build it first with: .\native\vcam\build.ps1"
    }

    Write-Host "Installing the PairCam camera into $installDirectory"
    New-Item -ItemType Directory -Force -Path $installDirectory | Out-Null

    # Files left behind by earlier updates, now no longer loaded.
    Get-ChildItem -Path $installDirectory -Filter 'paircam_vcam.dll.old-*' -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }

    if (Test-Path $installed) {
        # A loaded DLL cannot be overwritten but can be renamed. Apps keep using the old copy
        # until they close the camera; new sessions load the new one.
        $aside = "$installed.old-$(Get-Date -Format 'yyyyMMddHHmmss')"
        try {
            Remove-Item -LiteralPath $installed -Force
        } catch {
            Move-Item -LiteralPath $installed -Destination $aside -Force
            Write-Host 'The previous camera DLL is in use; it was moved aside and will be removed next time.'
        }
    }
    Copy-Item -LiteralPath $source -Destination $installed -Force
    $pdb = [IO.Path]::ChangeExtension($source, '.pdb')
    if (Test-Path $pdb) {
        Copy-Item -LiteralPath $pdb -Destination $installDirectory -Force -ErrorAction SilentlyContinue
    }

    # Program Files already grants Users read access, but the camera services are named
    # explicitly so a locked-down machine cannot break the camera. S-1-5-19 is Local Service
    # (Frame Server), S-1-5-18 is Local System (Frame Server Monitor).
    foreach ($sid in @('*S-1-5-19', '*S-1-5-18')) {
        & icacls.exe $installDirectory /grant "${sid}:(OI)(CI)RX" /T /C /Q | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "icacls could not grant read access to $sid (exit code $LASTEXITCODE)."
        }
    }

    # The 64-bit regsvr32, even from a 32-bit PowerShell.
    $system = if ([Environment]::Is64BitProcess) { 'System32' } else { 'Sysnative' }
    $regsvr32 = Join-Path $env:SystemRoot "$system\regsvr32.exe"
    $process = Start-Process -FilePath $regsvr32 -ArgumentList @('/s', "`"$installed`"") -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "regsvr32 failed with exit code $($process.ExitCode)."
    }

    $key = "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32"
    $registered = (Get-ItemProperty -Path $key -ErrorAction SilentlyContinue).'(default)'
    if ($registered -ne $installed) {
        throw "The registration was not found in the registry after regsvr32 ($key)."
    }

    Write-Host ''
    Write-Host 'The PairCam camera is installed.' -ForegroundColor Green
    Write-Host 'Start PairCam; apps such as Camera, Teams, Zoom and Chrome then list "PairCam".'
    Wait-BeforeClosing
    exit 0
} catch {
    Write-Host ''
    Write-Host "Installing the PairCam camera failed: $($_.Exception.Message)" -ForegroundColor Red
    Wait-BeforeClosing
    exit 1
}
