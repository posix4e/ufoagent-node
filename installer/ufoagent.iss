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

[Files]
Source: "..\target\release\ufoagent.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Link this machine"; Filename: "{app}\ufoagent.exe"; Parameters: "link"
Name: "{group}\Repair UFOAgent"; Filename: "{app}\ufoagent.exe"; Parameters: "repair"

[Run]
; Install + start the Windows service (keeps LLM credentials fresh + heartbeats).
Filename: "{app}\ufoagent.exe"; Parameters: "service install"; Flags: runhidden
; Provision UFO2 + dependencies in the background (one-time, large download).
Filename: "{app}\ufoagent.exe"; Parameters: "bootstrap"; Flags: runhidden nowait
; Run the tray/manager in the user's interactive session at every logon.
Filename: "schtasks"; \
  Parameters: "/Create /TN ""UFOAgent Tray"" /TR ""\""{app}\ufoagent.exe\"" tray"" /SC ONLOGON /F"; \
  Flags: runhidden
; Launch the tray now so the 🛸 manager appears immediately (it FreeConsole()s its own window).
Filename: "{app}\ufoagent.exe"; Parameters: "tray"; Flags: nowait runhidden
; Offer to link now (opens a console showing a scannable QR — approve from your phone).
Filename: "{app}\ufoagent.exe"; Parameters: "link --pause"; \
  Description: "Link this machine to UFOAgent now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\ufoagent.exe"; Parameters: "service uninstall"; Flags: runhidden; RunOnceId: "SvcUninstall"
Filename: "schtasks"; Parameters: "/Delete /TN ""UFOAgent Tray"" /F"; Flags: runhidden; RunOnceId: "TrayTask"

[Code]
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
