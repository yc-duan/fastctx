; World's own part of installing and removing, around Tauri's NSIS template.

; A removal's report is read in the details: each step World took, and what
; is left on this computer and why.
ShowUninstDetails show

; 1 when this removal is to take everything: the "Delete the application data"
; box, or /FASTCTXEVERYTHING on the command line of a silent removal.
Var FastctxEverything

; The identity is installed even when the optional shortcut task is off:
; unpackaged Windows notifications require this Start menu shortcut.
!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-install' $0
  ${If} ${Errors}
    Abort "World notification identity installer could not be started."
  ${EndIf}
  ${If} $0 != 0
    Abort "World notification identity installation failed (exit $0)."
  ${EndIf}
  ; The fastctx carried here put in place, and what a removal that kept the
  ; data took out put back. What goes wrong is said and the installation
  ; goes on; World offers what is missing. The service that keeps this
  ; computer in its network from boot comes with joining or creating one.
  nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --after-install'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; World's kernel removes the application data itself, keeping anything it
  ; did not write, so the template's own deletion of those folders stays off.
  StrCpy $FastctxEverything $DeleteAppDataCheckboxState
  StrCpy $DeleteAppDataCheckboxState 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/FASTCTXEVERYTHING" $R1
  ${IfNot} ${Errors}
    StrCpy $FastctxEverything 1
  ${EndIf}
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ; /UPDATE belongs to the updater, never to a user's removal. A removal from
  ; World's settings has already done the kernel's part (/FASTCTXKERNELDONE)
  ; and hands only the package here.
  ${If} $UpdateMode != 1
    ClearErrors
    ${GetOptions} $R0 "/FASTCTXKERNELDONE" $R1
    ${If} ${Errors}
      ${If} $FastctxEverything = 1
        nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-world --everything'
      ${Else}
        nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-world'
      ${EndIf}
      Pop $0
      ${If} $0 == 10
        StrCpy $FastctxEverything 0
        ${IfNot} ${Silent}
          MessageBox MB_OK|MB_ICONINFORMATION "This computer is a world's server, so its data was kept. Removing everything is done inside World, where the world's name is typed to confirm."
        ${EndIf}
      ${ElseIf} $0 != 0
        Abort "World could not finish removing this computer's kernel (exit $0)."
      ${EndIf}
    ${ElseIf} $FastctxEverything = 1
      nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-finish'
      Pop $0
      ${If} $0 != 0
        Abort "World's kernel removal has not completed (exit $0)."
      ${EndIf}
    ${EndIf}
  ${EndIf}
  nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-uninstall'
  Pop $0
  ${If} $0 != 0
    Abort "World notification identity removal failed (exit $0)."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    ; Where World was installed, which the template keeps unless its own
    ; data deletion runs; a new installation goes to the default place.
    SetShellVarContext current
    DeleteRegKey HKCU "Software\${MANUFACTURER}\${PRODUCTNAME}"
    DeleteRegKey /ifempty HKCU "Software\${MANUFACTURER}"
    ; NSIS runs an uninstaller from a copy in a folder of its own under the
    ; temporary folder, which it cannot delete while running. Once this
    ; process has exited, Windows' PowerShell removes that folder. Started
    ; with _?= the uninstaller runs in place and there is no copy.
    ${GetParent} $EXEDIR $R2
    ${GetFileName} $EXEDIR $R3
    StrCpy $R4 $R3 4
    ${If} $R2 == $TEMP
    ${AndIf} $R4 == "~nsu"
      System::Call 'kernel32::GetCurrentProcessId() i .R5'
      ${WordReplace} $EXEDIR "'" "''" "+" $R6
      ${WordReplace} $EXEPATH "'" "''" "+" $R7
      ExecShell "" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" "-NoProfile -NonInteractive -WindowStyle Hidden -Command $\"$$p = Get-Process -Id $R5 -ErrorAction SilentlyContinue; if ($$p -and $$p.Path -eq '$R7') { $$p.WaitForExit() }; Remove-Item -LiteralPath '$R6' -Recurse -Force$\"" SW_HIDE
    ${EndIf}
  ${EndIf}
!macroend
