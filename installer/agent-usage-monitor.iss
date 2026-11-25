; Inno Setup script for Agent Usage Monitor (per-user install, no administrator rights).
; Build: iscc /DAppVersion=0.9.0 /DSourceExe=..\target\release\agent-usage-monitor.exe installer\agent-usage-monitor.iss
; The CI release workflow does this on a Windows runner.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\agent-usage-monitor.exe"
#endif

[Setup]
AppId={{441C2F18-5B58-4B7C-A95D-68D18A524F2C}
AppName=Agent Usage Monitor
AppVersion={#AppVersion}
AppPublisher=Agent Usage Monitor contributors
AppPublisherURL=https://github.com/VVVNL/agent-usage-monitor
DefaultDirName={localappdata}\Programs\AgentUsageMonitor
DefaultGroupName=Agent Usage Monitor
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=agent-usage-monitor-{#AppVersion}-setup
OutputDir=..\dist
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Agent Usage Monitor
LicenseFile=..\LICENSE
; Replace a running copy instead of failing on a locked file.
CloseApplications=force
RestartApplications=no

[Tasks]
Name: "autostart"; Description: "Start Agent Usage Monitor when I sign in to Windows"; Flags: unchecked
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "agent-usage-monitor.exe"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
; The AppUserModelID lets Windows attribute notifications to this app by name.
Name: "{userprograms}\Agent Usage Monitor"; Filename: "{app}\agent-usage-monitor.exe"; AppUserModelID: "AgentUsageMonitor.App"
Name: "{userdesktop}\Agent Usage Monitor"; Filename: "{app}\agent-usage-monitor.exe"; Tasks: desktopicon

[Registry]
; Same per-user value the app itself manages from Settings > Launch at login.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "AgentUsageMonitor"; ValueData: """{app}\agent-usage-monitor.exe"""; Tasks: autostart; Flags: uninsdeletevalue
; Remove the login entry on uninstall even if it was enabled later from Settings.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "AgentUsageMonitor"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\agent-usage-monitor.exe"; Description: "Start Agent Usage Monitor now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM agent-usage-monitor.exe /F"; Flags: runhidden skipifdoesntexist; RunOnceId: "StopApp"

; Settings and cached values live in %APPDATA%\AgentUsageMonitor and are kept on uninstall
; (they contain no credentials). Delete that folder to remove them.
