// Where a file the conversation names goes when it is pressed
// (`world-gui.md`, 对话的侧面板): one that reads as text opens in the side,
// a picture in the viewer, anything else with this device's program for
// its kind. Told by the name's ending alone: nothing is read to decide.

export type FileKind = "text" | "picture" | "outside";

/** The pictures the machines draw small for the viewer (`thumbnail.rs`). */
const PICTURES = new Set(["png", "jpg", "jpeg", "gif", "webp"]);

/** What only a program of its own shows: pages for a browser, documents,
 *  archives, media, fonts, databases, models and compiled code. */
const OUTSIDE = new Set([
  "html", "htm", "xhtml", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "pages", "numbers", "key",
  "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst", "lz4", "cab", "deb", "rpm", "dmg", "pkg", "msi", "msix", "appx", "apk", "aab", "ipa", "jar", "war", "iso", "img", "vhd", "vhdx", "qcow2",
  "exe", "dll", "so", "dylib", "bin", "o", "obj", "a", "lib", "pdb", "class", "pyc", "pyo", "wasm", "node",
  "mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "wma", "mid", "midi",
  "mp4", "m4v", "mov", "avi", "mkv", "webm", "wmv", "flv", "3gp",
  "bmp", "tif", "tiff", "ico", "icns", "heic", "heif", "avif", "svg", "psd", "ai", "eps", "sketch", "fig", "xcf", "raw", "cr2", "nef", "dng",
  "ttf", "otf", "woff", "woff2", "eot",
  "sqlite", "sqlite3", "db", "mdb", "accdb", "parquet", "arrow", "feather", "avro", "orc", "h5", "hdf5", "npy", "npz", "mat",
  "pt", "pth", "onnx", "safetensors", "ckpt", "pkl", "pickle", "gguf", "tflite", "pb",
  "blend", "fbx", "stl", "step", "stp", "dwg", "dxf",
]);

/** Where pressing a file named `path` takes it. */
export function fileKind(path: string): FileKind {
  const ending = endingOf(path);
  if (PICTURES.has(ending)) return "picture";
  if (OUTSIDE.has(ending)) return "outside";
  return "text";
}

/** Whether a text file is Markdown, set as prose rather than as lines. */
export function isMarkdown(path: string): boolean {
  const ending = endingOf(path);
  return ending === "md" || ending === "markdown";
}

/** A file's own name, after the last slash of either kind. */
export function leafOf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}

function endingOf(path: string): string {
  const leaf = leafOf(path);
  const dot = leaf.lastIndexOf(".");
  return dot <= 0 ? "" : leaf.slice(dot + 1).toLowerCase();
}
