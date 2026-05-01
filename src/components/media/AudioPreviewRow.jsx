import React from 'react';
import { Play } from 'lucide-react';
import { Waveform } from './Waveform.jsx';

export function AudioPreviewRow({ asset, onClick }) {
  return (
    <div className={`audio-item${onClick ? ' audio-item-clickable' : ''}`} onClick={onClick}>
      <button type="button" className="audio-play-btn"><Play size={14} /></button>
      <span className="audio-name">{asset.name}</span>
      <Waveform />
      <em className="audio-duration">
        {asset.duration_seconds ? `${Math.round(asset.duration_seconds)}s` : '--'}
      </em>
    </div>
  );
}
