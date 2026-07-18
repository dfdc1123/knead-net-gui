/**
 * Turn paths from Tauri's native drag event into an existing folder import.
 * A KiCad file selects its matching project; a lone other path may be a folder.
 *
 * @param {readonly string[]} paths
 * @returns {{ folder: string, preferredProject: string | null } | null}
 */
export function projectTargetFromDrop(paths) {
  const kicadFile = paths.find((path) => /\.kicad_(?:sch|pcb)$/i.test(path));
  if (kicadFile) {
    const separator = Math.max(
      kicadFile.lastIndexOf("/"),
      kicadFile.lastIndexOf("\\"),
    );
    if (separator < 0) return null;

    const isWindowsRoot = separator === 2 && kicadFile[1] === ":";
    const folder = separator === 0 || isWindowsRoot
      ? kicadFile.slice(0, separator + 1)
      : kicadFile.slice(0, separator);
    const fileName = kicadFile.slice(separator + 1);
    const preferredProject = fileName.replace(/\.kicad_(?:sch|pcb)$/i, "");
    return { folder, preferredProject };
  }

  if (paths.length === 1 && paths[0]) {
    return { folder: paths[0], preferredProject: null };
  }
  return null;
}
