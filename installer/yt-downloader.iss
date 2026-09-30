; Instalador do YT Downloader (Inno Setup 6).
; Gere com installer\build.ps1, que compila o app e passa a versão do Cargo.toml.

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
; Instala só para o usuário atual: não pede admin e o winget instala no mesmo perfil.
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
Name: "ptbr"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "deps"; Description: "Instalar/atualizar dependências via winget (yt-dlp, FFmpeg e Deno)"; GroupDescription: "Dependências:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Configurações salvas pelo app. As dependências do winget são mantidas,
; pois podem ser usadas por outros programas.
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
  WizardForm.StatusLabel.Caption := 'Instalando ' + Name + ' (winget)... isso pode levar alguns minutos.';
  WizardForm.Refresh;
  // "install" também atualiza se já existir uma versão antiga.
  Code := RunHidden('winget install -e --id ' + Id +
    ' --accept-package-agreements --accept-source-agreements --silent --disable-interactivity');
  Log(Format('winget install %s terminou com código %d', [Id, Code]));
end;

procedure InstallDependencies();
begin
  if not WingetAvailable() then
  begin
    SuppressibleMsgBox(
      'O winget (Gerenciador de Pacotes do Windows) não foi encontrado.' + #13#10 +
      'Instale o "Instalador de Aplicativo" pela Microsoft Store e depois use o botão ' +
      '"Instalar yt-dlp" dentro do YT Downloader.', mbInformation, MB_OK, IDOK);
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
      'Não foi possível confirmar a instalação do yt-dlp.' + #13#10 +
      'Se o app avisar que ele não foi encontrado, use o botão "Instalar yt-dlp" dentro do programa.',
      mbInformation, MB_OK, IDOK);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('deps') then
    InstallDependencies();
end;
