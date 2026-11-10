; peerPrint — Inno Setup 6 script.
; Build the release binary first:  cargo build --release
; Then compile:                    iscc installer\peerprint.iss
;
; Setup asks for the install mode with Inno's built-in dialog:
;   All users  -> Program Files, machine-wide Windows service (starts before sign-in)
;   Just me    -> %LocalAppData%\Programs, tray app at sign-in (no admin rights)
; A wizard page then asks what this PC should do: host, client, or both.

#define MyAppName "peerPrint"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "peerPrint"
#define MyAppExeName "peerprint.exe"

[Setup]
AppId={{7C4D6B2E-5B61-4F7E-9B7A-5C3E9D2A6F14}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\peerPrint
DefaultGroupName=peerPrint
DisableProgramGroupPage=yes
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=peerPrint-{#MyAppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
CloseApplications=yes
AppMutex=Local\peerPrint-{username}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\peerPrint"; Filename: "{app}\{#MyAppExeName}"; Parameters: "{code:GetLaunchParams}"
Name: "{autodesktop}\peerPrint"; Filename: "{app}\{#MyAppExeName}"; Parameters: "{code:GetLaunchParams}"; Tasks: desktopicon

[Run]
; Store the chosen role first: machine-wide state for an all-users install,
; the current user's profile for a single-user install.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--set-role {code:GetRole} {code:GetScopeFlag}"; Flags: runhidden waituntilterminated
; All users: register the boot-start Windows service that runs the engine.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--install-service"; Flags: runhidden waituntilterminated; Check: IsAdminInstallMode
; All users: launch the tray UI as the signed-in user, not elevated.
Filename: "{app}\{#MyAppExeName}"; Parameters: "{code:GetLaunchParams}"; Description: "Launch peerPrint"; Flags: nowait postinstall skipifsilent runasoriginaluser; Check: IsAdminInstallMode
; Just me: launch the self-contained tray app.
Filename: "{app}\{#MyAppExeName}"; Description: "Launch peerPrint"; Flags: nowait postinstall skipifsilent; Check: not IsAdminInstallMode

[UninstallRun]
; All users: stop and delete the service before removing its queues.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--uninstall-service"; Flags: runhidden waituntilterminated; RunOnceId: "RemovePeerPrintService"; Check: IsAdminInstallMode
Filename: "{app}\{#MyAppExeName}"; Parameters: "--uninstall-all {code:GetScopeFlag}"; Flags: runhidden waituntilterminated; RunOnceId: "RemovePeerPrintQueues"

[Code]
var
  RolePage: TInputOptionWizardPage;

procedure InitializeWizard;
begin
  RolePage := CreateInputOptionPage(wpSelectTasks,
    'What should this PC do?',
    'Choose how peerPrint is used on this PC.',
    'You can change this later from the peerPrint window.',
    True, False);
  RolePage.Add('Host only - share printers attached to this PC');
  RolePage.Add('Client only - use printers shared by another PC');
  RolePage.Add('Both - share and use printers (recommended)');
  RolePage.Values[2] := True;
end;

function GetRole(Param: String): String;
begin
  if RolePage.Values[0] then
    Result := 'host'
  else if RolePage.Values[1] then
    Result := 'client'
  else
    Result := 'both';
end;

function GetScopeFlag(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := '--machine-wide'
  else
    Result := '';
end;

function GetLaunchParams(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := '--machine-wide'
  else
    Result := '';
end;
