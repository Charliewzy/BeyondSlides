#ifndef AppVersion
  #error AppVersion must be supplied by the packaging script.
#endif
#ifndef SourceDir
  #error SourceDir must be supplied by the packaging script.
#endif
#ifndef OutputDir
  #error OutputDir must be supplied by the packaging script.
#endif

[Setup]
AppId={{ED572A50-2F36-4DD0-9718-61A7C7E4043B}
AppName=BeyondSlides
AppVersion={#AppVersion}
AppPublisher=BeyondSlides contributors
AppPublisherURL=https://github.com/Charliewzy/BeyondSlides
AppSupportURL=https://github.com/Charliewzy/BeyondSlides/issues
AppUpdatesURL=https://github.com/Charliewzy/BeyondSlides/releases
DefaultDirName={localappdata}\Programs\BeyondSlides
DefaultGroupName=BeyondSlides
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=BeyondSlides-Setup-x86_64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile={#SourceDir}\LICENSE.beyond-slides.txt
UninstallDisplayIcon={app}\BeyondSlides.exe
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\BeyondSlides"; Filename: "{app}\BeyondSlides.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\BeyondSlides"; Filename: "{app}\BeyondSlides.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\BeyondSlides.exe"; Description: "Launch BeyondSlides"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
