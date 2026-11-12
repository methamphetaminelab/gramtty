#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Source
  #define Source "..\target\x86_64-pc-windows-msvc\release"
#endif

[Setup]
AppId={{B342C5A4-F19E-4FD2-9BE5-264BCB2FD336}
AppName=gramtty
AppVersion={#Version}
AppVerName=gramtty {#Version}
AppPublisher=methamphetaminelab
AppPublisherURL=https://github.com/methamphetaminelab/gramtty
AppSupportURL=https://github.com/methamphetaminelab/gramtty/issues
DefaultDirName={localappdata}\Programs\gramtty
DefaultGroupName=gramtty
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=gramtty-{#Version}-x86_64-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\gramtty.exe

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
en.PathTask=Add gramtty to PATH
ru.PathTask=Добавить gramtty в PATH
en.DepsTask=Install mpv, ffmpeg and Windows Terminal with winget (media, voice messages, images)
ru.DepsTask=Установить mpv, ffmpeg и Windows Terminal через winget (медиа, голосовые, картинки)
en.DepsStatus=Installing mpv, ffmpeg and Windows Terminal...
ru.DepsStatus=Устанавливаются mpv, ffmpeg и Windows Terminal...

[Tasks]
Name: "path"; Description: "{cm:PathTask}"
Name: "deps"; Description: "{cm:DepsTask}"

[Files]
Source: "{#Source}\gramtty.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\scripts\install-deps.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.ru.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\gramtty"; Filename: "{app}\gramtty.exe"

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\install-deps.ps1"""; StatusMsg: "{cm:DepsStatus}"; Tasks: deps
Filename: "{app}\gramtty.exe"; Description: "{cm:LaunchProgram,gramtty}"; Flags: postinstall nowait skipifsilent

[Code]
const
  EnvKey = 'Environment';

function UserPath(): string;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Result) then
    Result := '';
end;

procedure AddToPath(Dir: string);
var
  P: string;
begin
  P := UserPath();
  if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(P) + ';') > 0 then
    exit;
  if (P <> '') and (P[Length(P)] <> ';') then
    P := P + ';';
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', P + Dir);
end;

procedure RemoveFromPath(Dir: string);
var
  P: string;
  I: Integer;
begin
  P := ';' + UserPath() + ';';
  I := Pos(';' + Uppercase(Dir) + ';', Uppercase(P));
  if I = 0 then
    exit;
  Delete(P, I, Length(Dir) + 1);
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Copy(P, 2, Length(P) - 2));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('path') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
