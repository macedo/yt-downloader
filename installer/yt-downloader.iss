; YT Downloader installer (Inno Setup 6).
; Build it with installer\build.ps1, which compiles the app and passes the version from Cargo.toml.

#define AppName "YT Downloader"
#define AppExe "yt-downloader.exe"
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{3F85C58E-589A-4F2F-A7A1-98BF8E6C2EEF}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=macedo
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Per-user install: no admin prompt, and winget installs into the same profile.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=YT-Downloader-Setup-{#AppVersion}
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "deps"; Description: "Install/update dependencies via winget (yt-dlp, FFmpeg and Deno)"; GroupDescription: "Dependencies:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Settings saved by the app. The winget dependencies are kept, since other
; programs may use them.
Type: filesandordirs; Name: "{userappdata}\yt-downloader"

[Code]
function RunHidden(const Params: String): Integer;
begin
  if not Exec(ExpandConstant('{cmd}'), '/c ' + Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

function WingetAvailable(): Boolean;
begin
  Result := RunHidden('winget --version') = 0;
end;

function WingetPackageDirExists(const Id: String): Boolean;
var
  FindRec: TFindRec;
begin
  Result := FindFirst(ExpandConstant('{localappdata}\Microsoft\WinGet\Packages\') + Id + '_*', FindRec);
  if Result then
    FindClose(FindRec);
end;

function YtDlpInstalled(): Boolean;
begin
  Result := WingetPackageDirExists('yt-dlp.yt-dlp') or (RunHidden('where yt-dlp') = 0);
end;

procedure WingetInstall(const Id, Name: String);
var
  Code: Integer;
begin
  WizardForm.StatusLabel.Caption := 'Installing ' + Name + ' (winget)... this may take a few minutes.';
  WizardForm.Refresh;
  // "install" also upgrades an older version if one is present.
  Code := RunHidden('winget install -e --id ' + Id +
    ' --accept-package-agreements --accept-source-agreements --silent --disable-interactivity');
  Log(Format('winget install %s finished with code %d', [Id, Code]));
end;

procedure InstallDependencies();
begin
  if not WingetAvailable() then
  begin
    SuppressibleMsgBox(
      'winget (Windows Package Manager) was not found.' + #13#10 +
      'Install "App Installer" from the Microsoft Store, then use the ' +
      '"Install yt-dlp" button inside YT Downloader.', mbInformation, MB_OK, IDOK);
    Exit;
  end;

  WizardForm.ProgressGauge.Style := npbstMarquee;
  try
    WingetInstall('yt-dlp.yt-dlp', 'yt-dlp');
    WingetInstall('yt-dlp.FFmpeg', 'FFmpeg');
    WingetInstall('DenoLand.Deno', 'Deno');
  finally
    WizardForm.ProgressGauge.Style := npbstNormal;
  end;

  if not YtDlpInstalled() then
    SuppressibleMsgBox(
      'Could not confirm that yt-dlp was installed.' + #13#10 +
      'If the app says it cannot find it, use the "Install yt-dlp" button inside the program.',
      mbInformation, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('deps') then
    InstallDependencies();
end;
