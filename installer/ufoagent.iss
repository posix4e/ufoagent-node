; Inno Setup script — produces ufoagent-setup.exe (wraps the native Rust ufoagent.exe).
; Compile:  ISCC /DAppVersion=0.2.0 installer\ufoagent.iss
; Authenticode-signed in CI (Azure Trusted Signing).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B1F4B6E2-UFO2-AGENT-CTRL-PLANE-0001}}
AppName=UFOAgent
AppVersion={#AppVersion}
AppPublisher=UFOAgent, Inc.
AppCopyright=Copyright (C) 2026 UFOAgent, Inc.
VersionInfoCompany=UFOAgent, Inc.
VersionInfoCopyright=Copyright (C) 2026 UFOAgent, Inc.
DefaultDirName={autopf}\UFOAgent
DisableProgramGroupPage=yes
OutputBaseFilename=ufoagent-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
WizardStyle=modern
; We append {app} to the system PATH below, so notify other processes of the env change.
ChangesEnvironment=yes

[Files]
Source: "..\target\release\ufoagent.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Link this machine"; Filename: "{app}\ufoagent.exe"; Parameters: "link"
Name: "{group}\Configure unattended GUI mode"; Filename: "{app}\ufoagent.exe"; Parameters: "autologon --pause"
Name: "{group}\Repair UFOAgent"; Filename: "{app}\ufoagent.exe"; Parameters: "repair"
; This is the one node runtime path: a login-session agent starts with the auto-logged-in desktop.
; It owns the WebSocket, credentials, updates, screenshots, and GUI task execution.
Name: "{commonstartup}\UFOAgent"; Filename: "{app}\ufoagent.exe"; Parameters: "tray"

[Registry]
; Add the install dir to the system PATH so `ufoagent` works from any prompt (idempotent via Check).
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
  ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
  Check: NeedsAddPath('{app}')

[Run]
; Provision UFO2 + dependencies (one-time, large download). Run in a VISIBLE console via
; `cmd /c start` so the user sees progress and any failure stays on screen (`--pause`). `nowait`
; keeps the installer from blocking on the long download; the console lives on independently.
Filename: "{cmd}"; \
  Parameters: "/c start ""UFOAgent setup"" ""{app}\ufoagent.exe"" bootstrap --pause"; \
  Flags: nowait
; Launch the tray now so the 🛸 manager appears immediately (it FreeConsole()s its own window).
Filename: "{app}\ufoagent.exe"; Parameters: "tray"; Flags: nowait runhidden
; UFOAgent is built for unattended GUI tasks: always prompt for Windows auto-logon during interactive
; install so the machine reboots into a usable desktop. Silent installs can run `ufoagent autologon`
; with explicit credentials before rebooting.
Filename: "{cmd}"; \
  Parameters: "/c start /wait ""UFOAgent unattended GUI"" ""{app}\ufoagent.exe"" autologon --pause"; \
  Flags: skipifsilent
; Offer to link now (opens a console showing a scannable QR — approve from your phone).
Filename: "{app}\ufoagent.exe"; Parameters: "link --pause"; \
  Description: "Link this machine to UFOAgent now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM ufoagent.exe /F /T"; Flags: runhidden; RunOnceId: "KillAgent"
Filename: "{sys}\sc.exe"; Parameters: "delete UFOAgent"; Flags: runhidden; RunOnceId: "DeleteOldSvc"

[Code]
// True when {app} is not already on the system PATH — keeps reinstalls from appending duplicates.
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_LOCAL_MACHINE,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(ExpandConstant(Param)) + ';',
                ';' + Uppercase(OrigPath) + ';') = 0;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := '';
  // Auto-update / reinstall: the login-session agent locks ufoagent.exe. Stop any old service from
  // pre-collapse builds, delete it so there is only one runtime path, and kill the old tray process
  // before [Files] copies the new exe. Errors (fresh install / not running) are ignored.
  Exec(ExpandConstant('{sys}\net.exe'), 'stop UFOAgent', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{sys}\sc.exe'), 'delete UFOAgent', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM ufoagent.exe /F /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
