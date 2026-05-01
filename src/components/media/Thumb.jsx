import React from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { Users } from 'lucide-react';

export function Thumb({ asset, label, onClick }) {
  const srcPath = asset?.stored_path || asset?.path;
  return (
    <div className={`thumb${onClick ? ' thumb-clickable' : ''}`} title={label} onClick={onClick}>
      {srcPath ? <img src={convertFileSrc(srcPath)} alt={label} /> : <Users size={22} />}
    </div>
  );
}
