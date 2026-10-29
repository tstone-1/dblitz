; NSIS hooks for the dblitz installer (bundle.windows.nsis.installerHooks).
;
; The uninstaller removes the per-user "Open with" entries for CSV and JSON
; files that dblitz's Settings can add (src-tauri/src/platform/open_with.rs).
; An update runs the previous version's uninstaller with /UPDATE first, so the
; entries are kept then. A test in open_with.rs checks this list against the
; extensions the app writes.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Classes\.csv\OpenWithProgids" "dblitz.csv"
    DeleteRegKey HKCU "Software\Classes\dblitz.csv"
    DeleteRegValue HKCU "Software\Classes\.tsv\OpenWithProgids" "dblitz.tsv"
    DeleteRegKey HKCU "Software\Classes\dblitz.tsv"
    DeleteRegValue HKCU "Software\Classes\.json\OpenWithProgids" "dblitz.json"
    DeleteRegKey HKCU "Software\Classes\dblitz.json"
    DeleteRegValue HKCU "Software\Classes\.jsonl\OpenWithProgids" "dblitz.jsonl"
    DeleteRegKey HKCU "Software\Classes\dblitz.jsonl"
    DeleteRegValue HKCU "Software\Classes\.ndjson\OpenWithProgids" "dblitz.ndjson"
    DeleteRegKey HKCU "Software\Classes\dblitz.ndjson"
  ${EndIf}
!macroend
