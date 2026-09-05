!include "LogicLib.nsh"
!include "TextFunc.nsh"
!include "x64.nsh"

; Probe the same default distribution that the Windows app uses. Do not infer
; the active mode from .wslconfig: edits may not have been applied yet.
Function AITermCheckWslNetworking
  Push $0
  Push $1
  Push $2
  ${If} ${RunningX64}
    ${DisableX64FSRedirection}
  ${EndIf}
  nsExec::ExecToStack /TIMEOUT=15000 '"$SYSDIR\wsl.exe" --exec wslinfo --networking-mode'
  Pop $0
  Pop $1
  ${If} ${RunningX64}
    ${EnableX64FSRedirection}
  ${EndIf}
  ${TrimNewLines} "$1" $1

  ${If} $0 == "0"
  ${AndIf} $1 == "mirrored"
    DetailPrint "WSL mirrored networking is active."
  ${Else}
    StrCpy $2 "Setup could not confirm WSL's active networking mode."
    ${If} $0 == "0"
      ${If} $1 == "nat"
      ${OrIf} $1 == "virtioproxy"
      ${OrIf} $1 == "bridged"
      ${OrIf} $1 == "none"
        StrCpy $2 "WSL currently uses $1 networking."
      ${EndIf}
    ${EndIf}
    DetailPrint "$2 Mirrored networking is recommended for direct LAN connections."
    ; /SD keeps unattended installs noninteractive; there is no automatic
    ; configuration edit, distribution shutdown, or firewall change.
    MessageBox MB_OK|MB_ICONEXCLAMATION "$2$\r$\n$\r$\nFor direct phone connections over your local network, use mirrored networking.$\r$\n$\r$\nOpen WSL Settings > Networking and choose Mirrored. Save your work, then restart WSL or Windows to apply the change. This requires Windows 11 22H2 or later.$\r$\n$\r$\nYour phone must be able to reach the Windows PC, and the firewall must allow AITerm.$\r$\n$\r$\nYou can finish installing now and change this setting later." /SD IDOK
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  Call AITermCheckWslNetworking
!macroend
