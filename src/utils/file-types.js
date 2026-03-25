/**
 * File type detection by extension.
 * Returns a category that determines how the file is rendered.
 */

const IMAGE_EXTENSIONS = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp', 'tiff', 'tif', 'avif',
]);

const VIDEO_EXTENSIONS = new Set([
  'mp4', 'webm', 'ogv', 'mov',
]);

const AUDIO_EXTENSIONS = new Set([
  'mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a', 'wma', 'opus',
]);

const PDF_EXTENSIONS = new Set(['pdf']);

const DOCX_EXTENSIONS = new Set(['docx']);

const XLSX_EXTENSIONS = new Set(['xlsx', 'xls']);

const PPTX_EXTENSIONS = new Set(['pptx']);

const MARKDOWN_EXTENSIONS = new Set(['md', 'markdown', 'mdx']);

const BINARY_EXTENSIONS = new Set([
  'exe', 'dll', 'so', 'dylib', 'bin', 'dat', 'o', 'obj', 'a', 'lib',
  'class', 'pyc', 'pyo', 'wasm',
  'zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar',
  'ttf', 'otf', 'woff', 'woff2', 'eot',
]);

/**
 * Get the file type category for a given file path.
 * @param {string} filePath
 * @returns {'code'|'image'|'video'|'audio'|'pdf'|'docx'|'xlsx'|'pptx'|'binary'}
 */
export function getFileType(filePath) {
  const ext = getExtension(filePath);
  if (!ext) return 'code';

  if (IMAGE_EXTENSIONS.has(ext)) return 'image';
  if (VIDEO_EXTENSIONS.has(ext)) return 'video';
  if (AUDIO_EXTENSIONS.has(ext)) return 'audio';
  if (PDF_EXTENSIONS.has(ext)) return 'pdf';
  if (MARKDOWN_EXTENSIONS.has(ext)) return 'markdown';
  if (DOCX_EXTENSIONS.has(ext)) return 'docx';
  if (XLSX_EXTENSIONS.has(ext)) return 'xlsx';
  if (PPTX_EXTENSIONS.has(ext)) return 'pptx';
  if (BINARY_EXTENSIONS.has(ext)) return 'binary';

  return 'code';
}

/**
 * Check if a file type is a preview type (not editable in the code editor).
 */
export function isPreviewType(fileType) {
  return fileType !== 'code';
}

/**
 * Get MIME type for a file extension.
 */
export function getMimeType(filePath) {
  const ext = getExtension(filePath);
  const mimeMap = {
    // Images
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
    gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
    ico: 'image/x-icon', bmp: 'image/bmp', tiff: 'image/tiff',
    tif: 'image/tiff', avif: 'image/avif',
    // Video
    mp4: 'video/mp4', webm: 'video/webm', ogv: 'video/ogg', mov: 'video/quicktime',
    // Audio
    mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg',
    flac: 'audio/flac', aac: 'audio/aac', m4a: 'audio/mp4',
    wma: 'audio/x-ms-wma', opus: 'audio/opus',
    // Documents
    pdf: 'application/pdf',
    docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  };
  return mimeMap[ext] || 'application/octet-stream';
}

/**
 * Get a human-readable label for a file type.
 */
export function getFileTypeLabel(fileType) {
  const labels = {
    code: 'Text',
    image: 'Image',
    video: 'Video',
    audio: 'Audio',
    pdf: 'PDF',
    markdown: 'Markdown',
    docx: 'Word Document',
    xlsx: 'Spreadsheet',
    pptx: 'Presentation',
    binary: 'Binary',
  };
  return labels[fileType] || 'File';
}

function getExtension(filePath) {
  const parts = filePath.split(/[/\\]/);
  const fileName = parts[parts.length - 1];
  const dotIndex = fileName.lastIndexOf('.');
  if (dotIndex <= 0) return '';
  return fileName.substring(dotIndex + 1).toLowerCase();
}
