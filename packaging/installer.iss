; Inno Setup script for a distributable SonicForge installer.
;
; Build the binaries first (`cargo build --release --workspace`), then compile
; this script with Inno Setup 6. The sf-setup.exe payload performs the COM
; registration and endpoint enablement, so both paths produce the same result.

#define AppName "SonicForge"
#define AppVersion "0.1.0"
#define AppPublisher "SonicForge contributors"
#define AppExe "sonicforge-editor.exe"

[Setup]
AppId={{8E3B1B64-5B2C-4E9E-9E4A-7C1D5A0F2B77}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=SonicForge-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\LICENSE

[Files]
Source: "..\target\release\sonicforge_apo.dll"; DestName: "SonicForgeAPO.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\sonicforge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\sonicforge-editor.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\sonicforge-tray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\sf-setup.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\SonicForge"; Filename: "{app}\{#AppExe}"
Name: "{group}\SonicForge tray"; Filename: "{app}\sonicforge-tray.exe"
Name: "{group}\Uninstall SonicForge"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\sf-setup.exe"; Parameters: "install --dir ""{app}"""; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "Open the SonicForge editor"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\sf-setup.exe"; Parameters: "uninstall --dir ""{app}"""; Flags: runhidden waituntilterminated; RunOnceId: "SonicForgeRestore"

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
end;
