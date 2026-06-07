; Inno Setup script — produces ufoagent-setup.exe.
; Compile:  ISCC /DAppVersion=0.1.0 installer\ufoagent.iss
; The output installer is Authenticode-signed in CI (Azure Trusted Signing), not here.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B1F4B6E2-UFO2-AGENT-CTRL-PLANE-0001}}
AppName=UFOAgent
AppVersion={#AppVersion}
AppPublisher=UFOAgent
DefaultDirName={autopf}\UFOAgent
DefaultGroupName=UFOAgent
DisableProgramGroupPage=yes
OutputBaseFilename=ufoagent-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
WizardStyle=modern

[Files]
Source: "..\dist\ufoagentd.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Link this machine"; Filename: "{app}\ufoagentd.exe"; Parameters: "link --control-plane https://ufoagent.xyz"

[Run]
; Register a background task that keeps credentials fresh + heartbeats.
Filename: "schtasks"; \
  Parameters: "/Create /TN ""UFOAgent Daemon"" /TR ""\""{app}\ufoagentd.exe\"" run-daemon"" /SC ONLOGON /RL HIGHEST /F"; \
  Flags: runhidden
; Offer to link immediately after install.
Filename: "{app}\ufoagentd.exe"; Parameters: "link --control-plane https://ufoagent.xyz"; \
  Description: "Link this machine to UFOAgent now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "schtasks"; Parameters: "/Delete /TN ""UFOAgent Daemon"" /F"; Flags: runhidden; RunOnceId: "DelTask"
