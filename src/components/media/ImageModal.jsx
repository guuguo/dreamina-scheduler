import React, { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { X, Copy, Check } from 'lucide-react';

export function ImageModal({ src, alt, onClose, onCopy }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!src) return;
    const handler = (e) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [src, onClose]);

  async function handleCopy() {
    if (onCopy) {
      await onCopy();
    } else {
      try {
        const res = await fetch(src);
        const blob = await res.blob();
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
      } catch { return; }
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  if (!src) return null;
  return createPortal(
    <div className="modal-backdrop image-modal-backdrop" role="presentation" onClick={onClose}>
      <div className="image-modal-content" onClick={(e) => e.stopPropagation()} onMouseDown={(e) => e.stopPropagation()}>
        <div className="image-modal-toolbar">
          <button type="button" className="image-modal-action" onClick={handleCopy} title="复制图片">
            {copied ? <Check size={16} /> : <Copy size={16} />}
            {copied ? '已复制' : '复制'}
          </button>
          <button type="button" className="image-modal-close" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
        <img src={src} alt={alt || ''} className="image-modal-img" />
      </div>
    </div>,
    document.body,
  );
}
