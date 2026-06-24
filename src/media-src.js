import { convertFileSrc } from '@tauri-apps/api/core';

function isDirectSrc(value) {
  return (
    typeof value === 'string'
    && (
      value.startsWith('data:')
      || value.startsWith('http://')
      || value.startsWith('https://')
      || value.startsWith('asset:')
      || value.startsWith('blob:')
    )
  );
}

export function resolveMediaSrc(path) {
  if (!path) return '';
  if (isDirectSrc(path)) return path;
  try {
    return convertFileSrc(path);
  } catch (_error) {
    return path;
  }
}
