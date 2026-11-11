// Folder paths of the file transfer panes (Windows `\` or POSIX `/`, as the side's `sep` says).

export function join(dir: string, name: string, sep: string) {
  if (!dir) return name;
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

/** The folder above `dir`; "" above a Windows drive (the drive list). */
export function parent(dir: string, sep: string) {
  const t = dir.endsWith(sep) && dir.length > 1 ? dir.slice(0, -1) : dir;
  const i = t.lastIndexOf(sep);
  if (i <= 0) return sep === "/" ? "/" : t.length <= 3 ? "" : t.slice(0, i + 1);
  const up = t.slice(0, i) || sep;
  // A drive's root is `C:\`: a bare `C:` means "the current folder on C:" to Windows, which the
  // host cannot list (the request timed out and the pane stopped answering).
  return /^[A-Za-z]:$/.test(up) ? up + sep : up;
}
