; The Pocketdesk installer for Windows (Inno Setup 6).
;
; Build it from the repository root after `cargo build --release`:
;   ISCC packaging\windows\pocketdesk.iss /DAppVersion=v0.7.0 /DNumericVersion=0.7.0
; /DSourceDir=<folder> picks the executables from elsewhere (default
; target\release), /DOutputDir=<folder> where setup.exe goes (default release).
;
; It installs to Program Files, adds Pocketdesk to the Start menu and to
; Apps & features, and can start it at sign-in (the logon task and the
; firewall rules `pocketdesk install` makes), put `pocketdesk` on PATH and
; install FFmpeg with winget. The uninstaller stops it, removes all of that,
; and asks before deleting the pairings and settings.

#ifndef AppVersion
  #define AppVersion "dev"
#endif
#ifndef NumericVersion
  #define NumericVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\release"
#endif
#define RepoRoot "..\.."

[Setup]
AppId={{8D3F6C2E-5B1A-4E7C-9A2F-3C6D1E8B4A70}
AppName=Pocketdesk
AppVersion={#AppVersion}
AppVerName=Pocketdesk {#AppVersion}
AppPublisher=Pocketdesk
AppPublisherURL=https://github.com/sreevarshan-xenoz/Pocketdesk
AppSupportURL=https://github.com/sreevarshan-xenoz/Pocketdesk/issues
AppUpdatesURL=https://github.com/sreevarshan-xenoz/Pocketdesk/releases
VersionInfoVersion={#NumericVersion}
DefaultDirName={autopf}\Pocketdesk
DefaultGroupName=Pocketdesk
DisableProgramGroupPage=yes
; Program Files (so an elevated logon task stays possible) and the firewall
; both need an administrator.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
; What Pocketdesk sends to whom, and how to turn it off, before anything is
; installed (SignPath Foundation's terms ask for it): docs/privacy.md in short.
InfoBeforeFile=privacy.txt
WizardImageFile=wizard-large.bmp,wizard-large-200.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-200.bmp
SetupIconFile={#RepoRoot}\assets\brand\pocketdesk.ico
UninstallDisplayIcon={app}\pocketdesk.ico
UninstallDisplayName=Pocketdesk
OutputDir={#OutputDir}
OutputBaseFilename=pocketdesk-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/ultra64
SolidCompression=yes
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
StartupGroup=Start-up:
AutostartTask=Start Pocketdesk when I sign in, and let phones on my network reach it (firewall)
CommandLineGroup=Command line:
PathTask=Add the pocketdesk command to PATH
VideoGroup=Video:
FFmpegTask=Install FFmpeg with winget (hardware video encoding needs it)
OpenApp=Open Pocketdesk
FirewallStatus=Opening Pocketdesk's firewall ports...
AutostartStatus=Registering Pocketdesk to start at sign-in...
FFmpegStatus=Installing FFmpeg with winget (this can take a minute)...
FirewallFailed=Pocketdesk's firewall ports could not be opened, so phones on your network may not reach it.%n%nOpen them later from an administrator terminal: pocketdesk firewall-add
AutostartFailed=Pocketdesk could not be set to start at sign-in.%n%nTry again later from a terminal: pocketdesk install
FFmpegFailed=FFmpeg could not be installed with winget. Hardware video encoding needs it.%n%nInstall it later: winget install Gyan.FFmpeg
RemoveData=Also delete your pairings, keys and settings?%n%nKeep them to pair nothing again when you reinstall Pocketdesk.

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; GroupDescription: "{cm:StartupGroup}"
Name: "addtopath"; Description: "{cm:PathTask}"; GroupDescription: "{cm:CommandLineGroup}"
Name: "ffmpeg"; Description: "{cm:FFmpegTask}"; GroupDescription: "{cm:VideoGroup}"; Check: CanInstallFFmpeg
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\pocketdesk.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\pocketdesk-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\pocketdesk-tray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\assets\brand\pocketdesk.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\config.toml.example"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\SECURITY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Pocketdesk"; Filename: "{app}\pocketdesk-app.exe"; IconFilename: "{app}\pocketdesk.ico"; Comment: "Your desktop, on your phone"
Name: "{autodesktop}\Pocketdesk"; Filename: "{app}\pocketdesk-app.exe"; IconFilename: "{app}\pocketdesk.ico"; Tasks: desktopicon

[Registry]
; The system PATH, so `pocketdesk` works in any terminal. The uninstaller
; takes it out again (RemoveFromPath).
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NotOnPath(ExpandConstant('{app}'))

[Run]
; FFmpeg, the firewall and the logon task run from [Code] (ssPostInstall),
; where a failure is told rather than lost.
Filename: "{app}\pocketdesk-app.exe"; Description: "{cm:OpenApp}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
; As before an upgrade: the logon task off first, so its restart-on-failure
; cannot start a killed server again; then a stop, and a kill for a server
; that has not exited by then (`stop` waits 10 s), since a running server
; holds pocketdesk.exe and the uninstaller could not delete it.
Filename: "{sys}\schtasks.exe"; Parameters: "/Change /TN ""Pocketdesk"" /DISABLE"; Flags: runhidden waituntilterminated; RunOnceId: "DisableTask"
Filename: "{app}\pocketdesk.exe"; Parameters: "stop"; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "StopServer"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM pocketdesk.exe"; Flags: runhidden waituntilterminated; RunOnceId: "KillServer"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM pocketdesk-tray.exe"; Flags: runhidden waituntilterminated; RunOnceId: "StopTray"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM pocketdesk-app.exe"; Flags: runhidden waituntilterminated; RunOnceId: "StopApp"
Filename: "{app}\pocketdesk.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "Unregister"

[Code]
const
  EnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

{ Whether Dir is missing from the system PATH. }
function NotOnPath(Dir: string): Boolean;
var
  Paths: string;
begin
  if not RegQueryStringValue(HKLM, EnvKey, 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') = 0;
end;

{ Take Dir out of the system PATH, wherever it sits in it. }
procedure RemoveFromPath(Dir: string);
var
  Paths: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKLM, EnvKey, 'Path', Paths) then
    exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then
    exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2);
  RegWriteExpandStringValue(HKLM, EnvKey, 'Path', Paths);
end;

{ The FFmpeg task is offered only when FFmpeg is not on PATH already and
  winget is there to install it. }
function CanInstallFFmpeg(): Boolean;
begin
  Result := (FileSearch('ffmpeg.exe', GetEnv('PATH')) = '') and
    FileExists(ExpandConstant('{localappdata}\Microsoft\WindowsApps\winget.exe'));
end;

{ An upgrade stops the running server (killing one that outlives `stop`),
  the tray and the app first, so their files can be replaced. The logon task is disabled first: a server that had
  to be killed would otherwise be restarted by it a minute later, mid-copy.
  `install` registers it again, enabled, when the start-up task is chosen. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Result := '';
  if FileExists(ExpandConstant('{app}\pocketdesk.exe')) then
  begin
    Exec(ExpandConstant('{sys}\schtasks.exe'), '/Change /TN "Pocketdesk" /DISABLE', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(ExpandConstant('{app}\pocketdesk.exe'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
    { A server that has not exited by then (`stop` waits 10 s) still holds
      pocketdesk.exe, which could then be replaced only at the next restart. }
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM pocketdesk.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM pocketdesk-tray.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM pocketdesk-app.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
end;

{ Run one step after the files are in, as the elevated installer or as the
  user who started it; a failure is logged and told (a silent install only
  logs it), and the install goes on. }
procedure RunStep(Status, Exe, Params: String; AsUser: Boolean; Failure: String);
var
  Code: Integer;
  Started: Boolean;
begin
  WizardForm.StatusLabel.Caption := Status;
  if AsUser then
    Started := ExecAsOriginalUser(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Code)
  else
    Started := Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Code);
  if not Started or (Code <> 0) then
  begin
    Log(Format('%s %s failed (started: %d, exit code %d)', [Exe, Params, Ord(Started), Code]));
    SuppressibleMsgBox(Failure, mbError, MB_OK, IDOK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Exe: String;
begin
  if CurStep <> ssPostInstall then
    exit;
  Exe := ExpandConstant('{app}\pocketdesk.exe');
  { FFmpeg first, so the server the logon task starts finds it on PATH. }
  if WizardIsTaskSelected('ffmpeg') then
    RunStep(CustomMessage('FFmpegStatus'), ExpandConstant('{cmd}'),
      '/c winget install --id Gyan.FFmpeg -e --silent --accept-package-agreements --accept-source-agreements',
      True, CustomMessage('FFmpegFailed'));
  { The firewall from here, already elevated; the logon task as the user who
    ran the installer, whose sign-in it starts at. }
  if WizardIsTaskSelected('autostart') then
  begin
    RunStep(CustomMessage('FirewallStatus'), Exe, 'firewall-add', False, CustomMessage('FirewallFailed'));
    RunStep(CustomMessage('AutostartStatus'), Exe, 'install --no-firewall', True, CustomMessage('AutostartFailed'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
  { The pairings, keys and settings stay unless asked; a silent uninstall
    keeps them. }
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox(CustomMessage('RemoveData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\pocketdesk'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\pocketdesk'), True, True, True);
    end;
end;
