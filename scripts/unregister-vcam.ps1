<#
.SYNOPSIS
    Removes the PairCam virtual camera from this computer. Needs administrator rights; asks for
    them itself.

.DESCRIPTION
    Reverses register-vcam.ps1: unregisters the DLL (removing its HKLM registration), deletes
    C:\Program Files\PairCam\paircam_vcam.dll and removes the folder when it is empty. A DLL that
    a camera app still has loaded is deleted at the next restart instead.

.PARAMETER NoPause
    Do not wait for Enter before closing the elevated window.

.EXAMPLE
    .\scripts\unregister-vcam.ps1
#>
[CmdletBinding()]
param(
    [switch] $NoPause
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$clsid = '{81785F8A-A3B5-4F45-BC7B-F92794EB45BB}' # PCVC_CLSID_STRING in paircam_vcam.h
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

if (-not (Test-Administrator)) {
    Write-Host 'Asking Windows for administrator rights to remove the PairCam camera...'
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    if ($NoPause) {
        $arguments += '-NoPause'
    }
    try {
        $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs `
            -Wait -PassThru
    } catch {
        Write-Host 'Cancelled: administrator rights were not granted.' -ForegroundColor Red
        exit 1
    }
    if ($process.ExitCode -eq 0) {
        Write-Host 'The PairCam camera is removed.' -ForegroundColor Green
    } else {
        Write-Host "Removing the PairCam camera failed (exit code $($process.ExitCode))." -ForegroundColor Red
    }
    exit $process.ExitCode
}

# MoveFileEx with MOVEFILE_DELAY_UNTIL_REBOOT deletes a file that is in use at the next restart.
Add-Type -Namespace PairCam -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool MoveFileEx(string existing, string replacement, int flags);
'@

function Remove-OrScheduleDelete([string] $Path) {
    try {
        Remove-Item -LiteralPath $Path -Force
    } catch {
        if (-not [PairCam.Native]::MoveFileEx($Path, $null, 4)) {
            throw "Cannot delete $Path and cannot schedule it for deletion."
        }
        Write-Host "$Path is in use; it will be deleted when Windows restarts."
    }
}

try {
    $key = "HKLM:\SOFTWARE\Classes\CLSID\$clsid"
    if (Test-Path $installed) {
        $system = if ([Environment]::Is64BitProcess) { 'System32' } else { 'Sysnative' }
        $regsvr32 = Join-Path $env:SystemRoot "$system\regsvr32.exe"
        $process = Start-Process -FilePath $regsvr32 -ArgumentList @('/u', '/s', "`"$installed`"") `
            -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            Write-Host "regsvr32 /u returned $($process.ExitCode); removing the registration directly."
        }
    }
    # Also covers a DLL that was deleted by hand while still registered.
    if (Test-Path $key) {
        Remove-Item -Path $key -Recurse -Force
    }

    if (Test-Path $installDirectory) {
        Get-ChildItem -LiteralPath $installDirectory -File |
            Where-Object { $_.Name -like 'paircam_vcam*' } |
            ForEach-Object { Remove-OrScheduleDelete $_.FullName }
        if (-not (Get-ChildItem -LiteralPath $installDirectory -Force)) {
            Remove-Item -LiteralPath $installDirectory -Force
        }
    }

    Write-Host ''
    Write-Host 'The PairCam camera is removed.' -ForegroundColor Green
    Wait-BeforeClosing
    exit 0
} catch {
    Write-Host ''
    Write-Host "Removing the PairCam camera failed: $($_.Exception.Message)" -ForegroundColor Red
    Wait-BeforeClosing
    exit 1
}
