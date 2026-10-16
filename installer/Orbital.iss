; Orbital Inno Setup Script
; Requires Inno Setup 6: https://jrsoftware.org/isdl.php
;
; Rust port: the single release binary replaces the C# self-contained publish
; output. `Source` points at `{#ReleaseDir}` (default `..\target\release`, i.e.
; `cargo build --release`), so the installer is built by
; scripts/Build-Installer.ps1 -> cargo build --release -> ISCC.exe. The single
; binary is `Orbital.exe` (Cargo.toml [[bin]] name); locales and assets are
; embedded in it, so no other files are packaged.

#define MyAppName "Orbital"
; Allow version to be overridden via /DMyAppVersion=x.y.z on the command line
#ifndef MyAppVersion
  #define MyAppVersion "1.0.0"
#endif
#define MyAppPublisher "CrowKing63"
#define MyAppURL "https://github.com/CrowKing63/Orbital"
#define MyAppExeName "Orbital.exe"
; Directory holding the cargo release binary. Defaults to the crate's own target
; dir; override on the ISCC command line (/DReleaseDir=<path>) when
; CARGO_TARGET_DIR points elsewhere, as scripts/Build-Installer.ps1 does.
#ifndef ReleaseDir
  #define ReleaseDir "..\target\release"
#endif
#define MyAppGUID "{{A3F2B5C1-8D4E-4F2A-9B6C-7E3D1A5F8C2B}"

[Setup]
AppId={#MyAppGUID}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
; Compression
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64os
ArchitecturesAllowed=x64os
; Output
OutputDir=..\dist
OutputBaseFilename=Orbital-{#MyAppVersion}-Setup
; Uninstaller
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\{#MyAppExeName}
; Privilege
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; The Rust release build (single natively-compiled exe).
Source: "{#ReleaseDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Kill running instance before uninstall
Filename: "taskkill.exe"; Parameters: "/f /im {#MyAppExeName}"; Flags: runhidden; RunOnceId: "KillOrbital"

[Code]
// Ask whether to remove AppData on uninstall
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  AppDataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    AppDataDir := ExpandConstant('{userappdata}\Orbital');
    if DirExists(AppDataDir) then
    begin
      if MsgBox(
        'Do you want to remove Orbital''s settings and data?' + #13#10 +
        '(' + AppDataDir + ')' + #13#10 + #13#10 +
        'Select No to keep your settings for a future reinstall.',
        mbConfirmation, MB_YESNO) = IDYES then
      begin
        DelTree(AppDataDir, True, True, True);
      end;
    end;
  end;
end;
