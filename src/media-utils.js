export function fileExt(path) {
  return String(path || '').split('.').pop()?.toLowerCase() || '';
}

export function isImagePath(path) {
  return ['png', 'jpg', 'jpeg', 'webp'].includes(fileExt(path));
}

export function isAudioPath(path) {
  return ['mp3', 'wav', 'm4a', 'aac'].includes(fileExt(path));
}

export function isSupportedRoleMedia(path) {
  return isImagePath(path) || isAudioPath(path);
}

export function isVideoPath(path) {
  return ['mp4', 'mov', 'webm', 'mkv'].includes(fileExt(path));
}

export function normalizeFilePathKey(path) {
  return String(path || '').trim();
}

export function uniqueFilePaths(paths) {
  const seen = new Set();
  return (paths || []).filter((path) => {
    const key = normalizeFilePathKey(path);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function splitCsv(value) {
  return String(value || '').split(/[,，]/).map((item) => item.trim()).filter(Boolean);
}
