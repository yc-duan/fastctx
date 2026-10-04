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
