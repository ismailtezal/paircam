; The PairCam installer (Inno Setup 7). Build it with installer\build.ps1, which stages the files
; and passes the defines below; compiling this script on its own is not supported.
;
; One elevated install puts the app, the FFmpeg runtime, the licence texts and the virtual camera
; DLL in C:\Program Files\PairCam, and registers the camera machine-wide. Windows' camera service
; (Frame Server, running as Local Service) loads that DLL, which is why it must live in a folder
; the service can read and why the whole install needs administrator rights.

#ifndef AppVersion
  #error AppVersion is not defined. Build the installer with installer\build.ps1.
#endif
#ifndef AppVersionNumeric
  #error AppVersionNumeric is not defined. Build the installer with installer\build.ps1.
#endif
#ifndef StageDir
  #error StageDir is not defined. Build the installer with installer\build.ps1.
#endif
#ifndef OutputDir
  #error OutputDir is not defined. Build the installer with installer\build.ps1.
#endif
#ifndef Copyright
  #define Copyright ""
#endif
#ifndef RepositoryUrl
  #define RepositoryUrl ""
#endif

#define AppName "PairCam"
#define AppExe "paircam.exe"
#define CameraDll "paircam_vcam.dll"
; PCVC_CLSID_STRING in native\vcam\include\paircam_vcam.h.
#define CameraClsid "{81785F8A-A3B5-4F45-BC7B-F92794EB45BB}"

[Setup]
; Never change AppId: Windows uses it to recognise an existing install when upgrading. The
; doubled brace escapes the opening brace of the GUID.
AppId={{AA285E5E-15B2-403C-A8EB-6A014ABA90E7}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppName}
AppCopyright={#Copyright}
#if RepositoryUrl != ""
AppPublisherURL={#RepositoryUrl}
AppSupportURL={#RepositoryUrl}/issues
AppUpdatesURL={#RepositoryUrl}/releases
#endif
VersionInfoVersion={#AppVersionNumeric}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany={#AppName}

; A 64-bit installer in 64-bit install mode: {autopf} is C:\Program Files and the camera DLL is
; registered with the 64-bit regsvr32. Only x64 Windows: on Arm64 Windows the camera service is
; native Arm64 and cannot load an x64 DLL. Virtual cameras need Windows 11 (build 22000).
SetupArchitecture=x64
ArchitecturesAllowed=x64os
MinVersion=10.0.22000

; Administrator rights once, for Program Files and the machine-wide camera registration.
PrivilegesRequired=admin
DefaultDirName={autopf}\{#AppName}
; The folder is fixed: a folder under a user profile would be unreadable for the camera service.
DisableDirPage=yes
DisableProgramGroupPage=yes
; Licence, then the desktop shortcut choice (whose Next button becomes Install), then done.
DisableReadyPage=yes
LicenseFile={#StageDir}\app\licenses\LICENSE-PairCam.txt
ShowLanguageDialog=no

WizardStyle=modern dynamic
WizardImageFile=assets\wizard-202.png,assets\wizard-336.png,assets\wizard-534.png
WizardImageFileDynamicDark=assets\wizard-dark-202.png,assets\wizard-dark-336.png,assets\wizard-dark-534.png
WizardSmallImageFile=assets\wizard-small-58.png,assets\wizard-small-97.png,assets\wizard-small-159.png
WizardSmallImageFileDynamicDark=assets\wizard-small-58.png,assets\wizard-small-97.png,assets\wizard-small-159.png
SetupIconFile=assets\paircam.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}

; Close a running PairCam before replacing its files (Windows Restart Manager). The camera DLL
; is excluded: it is loaded by the Frame Server service, which must not be stopped; that file is
; moved aside instead (see MoveAsideCameraDll). PairCam is not restarted by Restart Manager,
; which would start it elevated; the finish page offers to launch it as the user instead.
CloseApplications=yes
CloseApplicationsFilterExcludes={#CameraDll}
RestartApplications=no

OutputDir={#OutputDir}
OutputBaseFilename=PairCam-Setup-{#AppVersion}-x64
Compression=lzma2/max
SolidCompression=yes
; A log in %TEMP% (Setup Log *.txt) for support requests.
SetupLogging=yes
UninstallLogging=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "tr"; MessagesFile: "compiler:Languages\Turkish.isl"

[CustomMessages]
en.CameraNotRegistered=The PairCam camera could not be registered with Windows. PairCam will open, but other apps will not see its camera. Run Setup again to retry.
tr.CameraNotRegistered=PairCam kamerası Windows'a kaydedilemedi. PairCam açılır, ancak diğer uygulamalar kamerasını göremez. Yeniden denemek için kurulumu tekrar çalıştırın.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Dirs]
; Remove the folder at uninstall even when it existed before this install (for example, created
; by scripts\register-vcam.ps1 for a development build), as long as it is empty.
Name: "{app}"; Flags: uninsalwaysuninstall

[InstallDelete]
; Debug symbols left by scripts\register-vcam.ps1 from a development install.
Type: files; Name: "{app}\paircam_vcam.pdb"

[Files]
; The app, the FFmpeg runtime DLLs and the licence texts.
Source: "{#StageDir}\app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; The virtual camera. regserver: registered (DllRegisterServer, 64-bit regsvr32) after all files
; are installed, unregistered by the uninstaller before the file is removed.
; uninsrestartdelete: if the camera service still has it loaded at uninstall time, it is deleted
; at the next restart. No restartreplace: that flag would postpone every registration until a
; restart; an in-use DLL is renamed aside before copying instead.
Source: "{#StageDir}\camera\{#CameraDll}"; DestDir: "{app}"; Flags: ignoreversion regserver uninsrestartdelete; BeforeInstall: MoveAsideCameraDll

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; postinstall entries run as the user who started Setup, not elevated.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallDelete]
Type: files; Name: "{app}\paircam_vcam.pdb"

[Code]
const
  CameraClsid = '{#CameraClsid}';

var
  { Whether a file was queued for deletion at the next restart. }
  ScheduledRestartDelete: Boolean;

function CameraKey: String;
begin
  Result := 'Software\Classes\CLSID\' + CameraClsid;
end;

function CameraPath: String;
begin
  Result := ExpandConstant('{app}\{#CameraDll}');
end;

{ Deletes a file now or, when it is in use, at the next restart. }
procedure DeleteNowOrAtRestart(const Path: String);
begin
  if DeleteFile(Path) or RemoveDir(Path) then
    Exit;
  try
    RestartReplace(Path, '');
    ScheduledRestartDelete := True;
    Log('In use; will be deleted at the next restart: ' + Path);
  except
    Log('Could not delete or schedule the deletion of ' + Path + ': ' + GetExceptionMessage);
  end;
end;

{ Removes camera DLLs that earlier installs moved aside while they were in use. }
procedure RemoveOldCameraDlls;
var
  Directory: String;
  Found: TFindRec;
begin
  Directory := ExpandConstant('{app}');
  if FindFirst(Directory + '\{#CameraDll}.old-*', Found) then
  begin
    try
      repeat
        DeleteNowOrAtRestart(Directory + '\' + Found.Name);
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
end;

{ Runs before the camera DLL is copied. While an app uses the PairCam camera, Windows' camera
  service keeps the DLL loaded, and a loaded DLL cannot be overwritten or deleted, but it can be
  renamed. The service keeps using the renamed copy until the camera is closed; new sessions load
  the new DLL. The old copy is deleted at the next restart or by the next install. This mirrors
  scripts\register-vcam.ps1, and avoids both a restart and stopping the camera service. }
procedure MoveAsideCameraDll;
var
  Target, Aside: String;
begin
  Target := CameraPath;
  if not FileExists(Target) or DeleteFile(Target) then
    Exit;
  Aside := Target + '.old-' + GetDateTimeString('yyyymmddhhnnss', #0, #0);
  if RenameFile(Target, Aside) then
  begin
    Log('The camera DLL is in use; moved it aside to ' + Aside);
    DeleteNowOrAtRestart(Aside);
  end
  else
    Log('Could not move the camera DLL aside; Setup will try to replace it in place.');
end;

{ Program Files already lets every user read and run its files, but the camera service accounts
  are named explicitly so that a locked-down machine cannot break the camera, as
  scripts\register-vcam.ps1 does. S-1-5-19 is Local Service (Frame Server), S-1-5-18 is Local
  System (Frame Server Monitor). }
procedure GrantCameraServiceAccess;
var
  ResultCode: Integer;
  Parameters: String;
begin
  Parameters := AddQuotes(ExpandConstant('{app}')) +
    ' /grant *S-1-5-19:(OI)(CI)RX *S-1-5-18:(OI)(CI)RX /T /C /Q';
  if not Exec(ExpandConstant('{sys}\icacls.exe'), Parameters, '', SW_HIDE,
    ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
    Log('icacls could not grant the camera service access (exit code ' +
      IntToStr(ResultCode) + ').');
end;

{ Checks that regserver left the camera registered at this install's DLL, which is what the app
  looks for (paircam_engine::vcam::installed_path). }
procedure VerifyCameraRegistration;
var
  Registered: String;
begin
  if RegQueryStringValue(HKLM64, CameraKey + '\InprocServer32', '', Registered) and
    (CompareText(Registered, CameraPath) = 0) then
  begin
    Log('The camera is registered at ' + Registered);
    Exit;
  end;
  Log('The camera registration is missing or points elsewhere: "' + Registered + '"');
  SuppressibleMsgBox(CustomMessage('CameraNotRegistered'), mbError, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  case CurStep of
    ssInstall:
      RemoveOldCameraDlls;
    { File registration has already happened at this point (see "Installation Order"). }
    ssPostInstall:
      begin
        GrantCameraServiceAccess;
        VerifyCameraRegistration;
      end;
  end;
end;

{ ---- Uninstall ---- }

{ The process IDs of PairCam instances started from this install folder. }
function InstalledAppProcessIds: TArrayOfString;
var
  Locator, Service, Processes, Process: Variant;
  Path: String;
  Index: Integer;
begin
  SetArrayLength(Result, 0);
  Path := ExpandConstant('{app}\{#AppExe}');
  { WQL string literals escape backslashes and quotes. }
  StringChangeEx(Path, '\', '\\', True);
  StringChangeEx(Path, '''', '\''', True);
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Service := Locator.ConnectServer('.', 'root\CIMV2');
    Processes := Service.ExecQuery(
      'SELECT ProcessId FROM Win32_Process WHERE ExecutablePath = ''' + Path + '''');
    SetArrayLength(Result, Processes.Count);
    for Index := 0 to Processes.Count - 1 do
    begin
      Process := Processes.ItemIndex(Index);
      Result[Index] := IntToStr(Process.ProcessId);
    end;
  except
    Log('Could not list running processes: ' + GetExceptionMessage);
  end;
end;

procedure RunTaskkill(const Parameters: String);
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), Parameters, '', SW_HIDE, ewWaitUntilTerminated,
    ResultCode);
end;

{ A running PairCam holds its exe and DLLs open, so it is closed before its files are removed:
  first politely (taskkill without /F asks its window to close), then forcibly after five
  seconds. The user has already confirmed the uninstall at this point. }
procedure CloseRunningApp;
var
  Ids: TArrayOfString;
  Index, Waited: Integer;
begin
  Ids := InstalledAppProcessIds;
  if GetArrayLength(Ids) = 0 then
    Exit;
  Log('Closing the running PairCam.');
  for Index := 0 to GetArrayLength(Ids) - 1 do
    RunTaskkill('/PID ' + Ids[Index]);
  Waited := 0;
  while (GetArrayLength(InstalledAppProcessIds) > 0) and (Waited < 5000) do
  begin
    Sleep(250);
    Waited := Waited + 250;
  end;

  Ids := InstalledAppProcessIds;
  for Index := 0 to GetArrayLength(Ids) - 1 do
    RunTaskkill('/F /PID ' + Ids[Index]);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Registered, Directory: String;
begin
  case CurUninstallStep of
    usUninstall:
      begin
        CloseRunningApp;
        { Before the uninstaller removes its files and then the folder. }
        RemoveOldCameraDlls;
      end;
    usPostUninstall:
      begin
        { The uninstaller has already called DllUnregisterServer. If that failed, remove the
          registration directly, but only when it still points into this install folder: a
          development copy registered elsewhere is left alone. }
        if RegQueryStringValue(HKLM64, CameraKey + '\InprocServer32', '', Registered) and
          (CompareText(Registered, CameraPath) = 0) then
        begin
          Log('Removing the camera registration that DllUnregisterServer left behind.');
          RegDeleteKeyIncludingSubkeys(HKLM64, CameraKey);
        end;

        { A camera DLL still loaded by the camera service was queued for deletion at the next
          restart (uninsrestartdelete or above), so the folder cannot be removed yet: queue it
          too. Windows processes the queue in order and removes a directory only when empty. }
        Directory := ExpandConstant('{app}');
        if FileExists(CameraPath) or ScheduledRestartDelete then
          DeleteNowOrAtRestart(Directory);
      end;
  end;
end;