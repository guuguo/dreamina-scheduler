import React from 'react';
import { Users } from 'lucide-react';
import { resolveMediaSrc } from '../../media-src.js';

export function Thumb({ asset, label, onClick }) {
  const srcPath = asset?.stored_path || asset?.path;
  return (
    <div className={`thumb${onClick ? ' thumb-clickable' : ''}`} title={label} onClick={onClick}>
      {srcPath ? <img src={resolveMediaSrc(srcPath)} alt={label} /> : <Users size={22} />}
    </div>
  );
}
