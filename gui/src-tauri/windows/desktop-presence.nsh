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
  ; This computer stays in the World from boot: the fastctx carried here is
  ; put in place and its node kept by a service, an administrator asked once.
  ; Declining leaves the installation standing; World's settings offer it.
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --keep-connected'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $DeleteAppDataCheckboxState 0
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ; /UPDATE belongs to the updater, never to a user's removal. A settings
  ; removal has already finished the kernel and hands only the package here.
  ${If} $UpdateMode != 1
    ${GetParameters} $R0
    ClearErrors
    ${GetOptions} $R0 "/FASTCTXKERNELDONE" $R1
    ${If} ${Errors}
      ClearErrors
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-world' $0
      ${If} $0 != 0
        Abort "World could not finish removing this computer's kernel (exit $0)."
      ${EndIf}
    ${EndIf}
    ClearErrors
    ${GetOptions} $R0 "/FASTCTXEVERYTHING" $R1
    ${IfNot} ${Errors}
      ClearErrors
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-finish' $0
      ${If} $0 != 0
        Abort "World's kernel removal has not completed (exit $0)."
      ${EndIf}
    ${EndIf}
  ${EndIf}
  ClearErrors
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-uninstall' $0
  ${If} ${Errors}
    Abort "World notification identity remover could not be started."
  ${EndIf}
  ${If} $0 != 0
    Abort "World notification identity removal failed (exit $0)."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode != 1
    ${GetParameters} $R0
    ClearErrors
    ${GetOptions} $R0 "/FASTCTXEVERYTHING" $R1
    ${IfNot} ${Errors}
      SetShellVarContext current
      DeleteRegKey HKCU "Software\${MANUFACTURER}\${PRODUCTNAME}"
    ${EndIf}
  ${EndIf}
!macroend
