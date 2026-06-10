export interface WindowCopyGateOptions {
  hasSelection: boolean;
  targetTag?: string;
  isContentEditable: boolean;
  hasTextSelection: boolean;
  gridVisible: boolean;
}

export function shouldHandleWindowCopy({
  hasSelection,
  targetTag,
  isContentEditable,
  hasTextSelection,
  gridVisible,
}: WindowCopyGateOptions): boolean {
  if (!gridVisible) return false;
  if (!hasSelection) return false;
  if (targetTag === "INPUT" || targetTag === "TEXTAREA") return false;
  if (isContentEditable) return false;
  return !hasTextSelection;
}
