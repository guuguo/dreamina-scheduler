import React from 'react';
import { X } from 'lucide-react';

export function ImageModal({ src, alt, onClose }) {
  if (!src) return null;
  return (
    <div className="modal-backdrop image-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <div className="image-modal-content" onMouseDown={(e) => e.stopPropagation()}>
        <button type="button" className="image-modal-close" onClick={onClose}>
          <X size={18} />
        </button>
        <img src={src} alt={alt || ''} className="image-modal-img" />
      </div>
    </div>
  );
}
