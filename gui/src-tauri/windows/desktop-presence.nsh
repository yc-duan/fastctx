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
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ClearErrors
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --desktop-uninstall' $0
  ${If} ${Errors}
    Abort "World notification identity remover could not be started."
  ${EndIf}
  ${If} $0 != 0
    Abort "World notification identity removal failed (exit $0)."
  ${EndIf}
!macroend
