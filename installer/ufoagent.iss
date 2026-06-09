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
AppPublisher=UFOAgent
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
Name: "{group}\Repair UFOAgent"; Filename: "{app}\ufoagent.exe"; Parameters: "repair"
; Launch the tray manager for every user at logon (replaces the old schtasks ONLOGON task —
; a Startup-folder shortcut is simpler and more reliable). Inno removes it on uninstall.
Name: "{commonstartup}\UFOAgent"; Filename: "{app}\ufoagent.exe"; Parameters: "tray"

[Registry]
; Add the install dir to the system PATH so `ufoagent` works from any prompt (idempotent via Check).
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
  ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
  Check: NeedsAddPath('{app}')

[Run]
; Install + start the Windows service (keeps LLM credentials fresh + heartbeats).
Filename: "{app}\ufoagent.exe"; Parameters: "service install"; Flags: runhidden
; Provision UFO2 + dependencies (one-time, large download). Run in a VISIBLE console via
; `cmd /c start` so the user sees progress and any failure stays on screen (`--pause`). `nowait`
; keeps the installer from blocking on the long download; the console lives on independently.
Filename: "{cmd}"; \
  Parameters: "/c start ""UFOAgent setup"" ""{app}\ufoagent.exe"" bootstrap --pause"; \
  Flags: nowait
; Launch the tray now so the 🛸 manager appears immediately (it FreeConsole()s its own window).
Filename: "{app}\ufoagent.exe"; Parameters: "tray"; Flags: nowait runhidden
; Offer to link now (opens a console showing a scannable QR — approve from your phone).
Filename: "{app}\ufoagent.exe"; Parameters: "link --pause"; \
  Description: "Link this machine to UFOAgent now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\ufoagent.exe"; Parameters: "service uninstall"; Flags: runhidden; RunOnceId: "SvcUninstall"

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
  // Auto-update / reinstall: the running service locks ufoagent.exe. `net stop` blocks until the
  // service has fully stopped (unlike `sc stop`), releasing the lock before [Files] copies the new
  // exe. The idempotent `service install` in [Run] restarts it. Errors (not installed/running) are
  // ignored — on a fresh install there's nothing to stop.
  Exec(ExpandConstant('{sys}\net.exe'), 'stop UFOAgent', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
