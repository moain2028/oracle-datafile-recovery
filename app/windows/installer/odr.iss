; ODR desktop installer (Inno Setup 6). Built by .github/workflows/app.yml on a Windows runner.
; Per-user install (no administrator rights): %LOCALAPPDATA%\Programs\ODR. Same AppId = a newer
; Setup installs over the old version in place; user outputs (Documents\ODR) are never touched.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{25A15D3D-E04A-4781-9BE9-23C0651DED56}
AppName=ODR - Oracle Datafile Recovery
AppVersion={#AppVersion}
AppVerName=ODR {#AppVersion}
AppPublisher=moain2028
AppCopyright=(c) 2026 moain2028
VersionInfoVersion={#AppVersion}
VersionInfoDescription=ODR - Oracle Datafile Recovery Setup
DefaultDirName={autopf}\ODR
DefaultGroupName=ODR
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=Output
OutputBaseFilename=ODR-Setup-{#AppVersion}
SetupIconFile=odr.ico
UninstallDisplayIcon={app}\ODR.exe
UninstallDisplayName=ODR - Oracle Datafile Recovery
LicenseFile=payload\LICENSE.txt
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=no
LanguageDetectionMethod=none
CloseApplications=yes

[Languages]
Name: "ar"; MessagesFile: "Arabic.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Types]
Name: "full"; Description: "كامل (البرنامج + ملفات العرض للتجربة)"
Name: "compact"; Description: "البرنامج فقط"

[Components]
Name: "main"; Description: "برنامج ODR"; Types: full compact; Flags: fixed
Name: "demo"; Description: "ملفات عرض للتجربة: قاعدة أوراكل حقيقية (نحو 500 MB)"; Types: full

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "payload\ODR.exe"; DestDir: "{app}"; Components: main; Flags: ignoreversion
Source: "payload\ODR.exe.config"; DestDir: "{app}"; Components: main; Flags: ignoreversion
Source: "payload\tool\*"; DestDir: "{app}\tool"; Components: main; Flags: ignoreversion recursesubdirs
Source: "payload\docs\*"; DestDir: "{app}\docs"; Components: main; Flags: ignoreversion recursesubdirs
Source: "payload\demo\*"; DestDir: "{app}\demo"; Components: demo; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ODR - استرجاع بيانات أوراكل"; Filename: "{app}\ODR.exe"
Name: "{autodesktop}\ODR - استرجاع بيانات أوراكل"; Filename: "{app}\ODR.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ODR.exe"; Description: "{cm:LaunchProgram,ODR}"; Flags: nowait postinstall skipifsilent
