export function shouldAutoPlayAudioModal(asset) {
  return Boolean(String(asset?.stored_path || '').trim());
}
