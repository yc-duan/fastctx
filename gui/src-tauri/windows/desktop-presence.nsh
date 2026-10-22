; World's own part of installing and removing, around Tauri's NSIS template.

; A removal's report is read in the details: each step World took, and what
; is left on this computer and why.
ShowUninstDetails show

; 1 when this removal is to take everything: the "Delete the application data"
; box, or /FASTCTXEVERYTHING on the command line of a silent removal.
Var FastctxEverything

; The uninstaller's own command line. The template's running-app check below
; uses $R0 and the like for itself, so it is kept in a variable of its own.
Var FastctxParameters

; Repair keeps /UPDATE semantics while restoring a missing WebView runtime.
; The bootstrapper is embedded in, and authenticated with, the Fastctx package.
!macro NSIS_HOOK_PREINSTALL
  ${GetParameters} $R8
  ClearErrors
  ${GetOptions} $R8 "/FASTCTXREPAIR" $R9
  ${IfNot} ${Errors}
    ; 2026-10-22: a broken runtime can leave its version registered. Let the
    ; embedded installer check it, then the repairer's real WebView probe decides.
      !if "${INSTALLWEBVIEW2MODE}" != "embedBootstrapper"
        !error "Fastctx repair requires an embedded WebView2 bootstrapper."
      !endif
      InitPluginsDir
      File "/oname=$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
      ClearErrors
      ExecWait '"$PLUGINSDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install' $R9
      ${If} ${Errors}
        Abort "Starting the embedded WebView2 installer failed."
      ${EndIf}
      ${If} $R9 != 0
        Abort "The embedded WebView2 installer failed (exit $R9)."
      ${EndIf}
  ${EndIf}
!macroend

; The identity is installed even when the optional shortcut task is off:
; unpackaged Windows notifications require this Start menu shortcut.
!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-install' $0
  ${If} ${Errors}
    Abort "Fastctx notification identity installer could not be started."
  ${EndIf}
  ${If} $0 != 0
    Abort "Fastctx notification identity installation failed (exit $0)."
  ${EndIf}
  ; The fastctx carried here put in place, and what a removal that kept the
  ; data took out put back; a computer already in a network gets back the
  ; service that keeps it there from boot. What goes wrong is said and the
  ; installation goes on; World offers what is missing. A computer in no
  ; network gets its service when it first joins or creates one.
  ${If} $UpdateMode != 1
    nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --after-install'
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; World's kernel removes the application data itself, keeping anything it
  ; did not write, so the template's own deletion of those folders stays off.
  StrCpy $FastctxEverything $DeleteAppDataCheckboxState
  StrCpy $DeleteAppDataCheckboxState 0
  ${GetParameters} $FastctxParameters
  ClearErrors
  ${GetOptions} $FastctxParameters "/FASTCTXEVERYTHING" $R1
  ${IfNot} ${Errors}
    StrCpy $FastctxEverything 1
  ${EndIf}
  ; A World open for this user, in this session or another (a removal run
  ; remotely while the user's desktop session has it open), is asked to exit
  ; and waited for; the template's own check below cannot close a program
  ; in another session and would abort.
  ${If} $UpdateMode != 1
    nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --quit-running'
    Pop $0
    ${If} $0 != 0
      Abort "Fastctx could not be closed (exit $0)."
    ${EndIf}
  ${EndIf}
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ; /UPDATE belongs to the updater, never to a user's removal. A removal from
  ; World's settings has already done the kernel's part (/FASTCTXKERNELDONE)
  ; and hands only the package here.
  ${If} $UpdateMode != 1
    ClearErrors
    ${GetOptions} $FastctxParameters "/FASTCTXKERNELDONE" $R1
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
          MessageBox MB_OK|MB_ICONINFORMATION "This computer is a world's server, so its data was kept. Removing everything is done inside Fastctx, where the network's name is typed to confirm."
        ${EndIf}
      ${ElseIf} $0 == 11
        ${IfNot} ${Silent}
          MessageBox MB_OK|MB_ICONINFORMATION "This computer is a managed machine. Its owner takes it off the network in Fastctx; Fastctx can be removed here after. An administrator of this computer can remove it now."
        ${EndIf}
        Abort "This computer is a managed machine its owner keeps; nothing was removed."
      ${ElseIf} $0 != 0
        Abort "Fastctx could not finish removing this computer's kernel (exit $0)."
      ${EndIf}
    ${ElseIf} $FastctxEverything = 1
      nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-finish'
      Pop $0
      ${If} $0 != 0
        Abort "Fastctx's kernel removal has not completed (exit $0)."
      ${EndIf}
    ${EndIf}
  ${EndIf}
  nsExec::ExecToLog '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-uninstall'
  Pop $0
  ${If} $0 != 0
    Abort "Fastctx notification identity removal failed (exit $0)."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    ; Where World was installed, which the template keeps unless its own
    ; data deletion runs; a new installation goes to the default place.
    SetShellVarContext current
    DeleteRegKey HKCU "Software\${MANUFACTURER}\${PRODUCTNAME}"
    DeleteRegKey /ifempty HKCU "Software\${MANUFACTURER}"
  ${EndIf}
!macroend

; NSIS runs an uninstaller from a copy in a folder of its own under the
; temporary folder, which it cannot delete while running. Once this process
; has exited, whether the removal finished or stopped, Windows' PowerShell
; removes that folder. Started with _?= the uninstaller runs in place and
; there is no copy.
!macro FASTCTX_RETIRE_UNINSTALLER_COPY
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
!macroend

Function un.onUninstSuccess
  !insertmacro FASTCTX_RETIRE_UNINSTALLER_COPY
FunctionEnd

Function un.onUninstFailed
  !insertmacro FASTCTX_RETIRE_UNINSTALLER_COPY
FunctionEnd
