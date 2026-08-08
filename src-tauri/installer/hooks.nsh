; HS Tracker reads the game's traffic through Npcap, so the installer checks
; for it and offers to fetch it.
;
; Npcap is NOT bundled: its free edition may not be redistributed inside another
; installer, and only the paid OEM edition has a silent mode. So this downloads
; the official installer straight from npcap.com and runs it, leaving its own
; wizard in charge — exactly what the user would do by hand, minus the hunting.

; the welcome page is generic by default, and it is the first thing anyone
; sees — say what the thing is and what it needs. Defined here because Tauri
; includes this file before it lays out the pages.
!define MUI_WELCOMEPAGE_TITLE "HS Tracker"
!define MUI_WELCOMEPAGE_TEXT "A session overlay for Hero Siege: gold, experience, kills and every drop worth hearing about, shown on top of the game.$\r$\n$\r$\nIt reads the game's network traffic through Npcap and never touches the game itself. If Npcap is missing, this installer offers to fetch it.$\r$\n$\r$\nNothing is sent anywhere — every number stays on this machine.$\r$\n$\r$\nClick Next to continue."

!define NPCAP_VERSION "1.88"
!define NPCAP_URL "https://npcap.com/dist/npcap-${NPCAP_VERSION}.exe"
!define NPCAP_PAGE "https://npcap.com/#download"

!macro NSIS_HOOK_POSTINSTALL
  ; the driver installs its DLL beside the system ones, or into its own folder
  ; when WinPcap compatibility is switched off
  ${IfNot} ${FileExists} "$SYSDIR\Npcap\wpcap.dll"
  ${AndIfNot} ${FileExists} "$SYSDIR\wpcap.dll"
    MessageBox MB_YESNO|MB_ICONQUESTION \
      "HS Tracker needs Npcap to read Hero Siege's network traffic, and it is not installed.$\r$\n$\r$\nDownload Npcap ${NPCAP_VERSION} and run its installer now? Its default options are fine — just click through it." \
      /SD IDNO IDNO npcap_skipped

    DetailPrint "Downloading Npcap ${NPCAP_VERSION} from npcap.com..."
    nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ProgressPreference=$\'SilentlyContinue$\'; try { Invoke-WebRequest -UseBasicParsing -Uri $\'${NPCAP_URL}$\' -OutFile $\'$TEMP\npcap-setup.exe$\' } catch { exit 1 }"'
    Pop $0

    ${If} $0 == 0
    ${AndIf} ${FileExists} "$TEMP\npcap-setup.exe"
      DetailPrint "Running the Npcap installer..."
      ExecWait '"$TEMP\npcap-setup.exe"'
      Delete "$TEMP\npcap-setup.exe"
    ${Else}
      DetailPrint "The download failed — opening the Npcap download page instead."
      ExecShell "open" "${NPCAP_PAGE}"
    ${EndIf}
  ${EndIf}
  npcap_skipped:
!macroend
