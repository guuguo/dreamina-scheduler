import React, { useState, useEffect, useRef } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { X, Play, Loader2 } from 'lucide-react';
import { Waveform } from './Waveform.jsx';
import { shouldAutoPlayAudioModal } from '../../audio-modal-utils.js';

export function AudioAssetModal({ asset, onClose }) {
  const [isPlaying, setIsPlaying] = useState(false);
  const audioRef = useRef(null);

  useEffect(() => {
    setIsPlaying(false);
    if (!shouldAutoPlayAudioModal(asset)) return undefined;

    const audio = new Audio(convertFileSrc(asset.stored_path));
    let cancelled = false;
    audio.onended = () => {
      if (!cancelled) setIsPlaying(false);
    };
    audio.onerror = () => {
      if (!cancelled) setIsPlaying(false);
    };
    audioRef.current = audio;
    setIsPlaying(true);
    audio.play().catch(() => {
      if (!cancelled) setIsPlaying(false);
    });

    return () => {
      cancelled = true;
      audio.pause();
      if (audioRef.current === audio) {
        audioRef.current = null;
      }
    };
  }, [asset?.id, asset?.stored_path]);

  if (!asset) return null;

  const togglePlay = () => {
    if (!asset.stored_path) return;
    if (isPlaying) {
      audioRef.current?.pause();
      setIsPlaying(false);
      return;
    }
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.currentTime = 0;
    }
    const audio = new Audio(convertFileSrc(asset.stored_path));
    audio.onended = () => setIsPlaying(false);
    audio.onerror = () => setIsPlaying(false);
    audioRef.current = audio;
    audio.play().catch(() => setIsPlaying(false));
    setIsPlaying(true);
  };

  return (
    <div className="modal-backdrop audio-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section
        className="audio-modal-content"
        role="dialog"
        aria-modal="true"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="audio-modal-head">
          <div>
            <span>音频预览</span>
            <h3>{asset.name || asset.id?.slice(0, 8) || '未命名音频'}</h3>
          </div>
          <button type="button" className="image-modal-close audio-modal-close" onClick={onClose}>
            <X size={18} />
          </button>
        </header>
        <div className="role-detail-audio-row audio-modal-row">
          <button
            type="button"
            className={`play-round${isPlaying ? ' playing' : ''}`}
            onClick={togglePlay}
            disabled={!asset.stored_path}
          >
            {isPlaying ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
          </button>
          <strong>{asset.name || '音频素材'}</strong>
          <Waveform active={isPlaying} />
          <em>{asset.duration_seconds ? `${Math.round(asset.duration_seconds)}s` : '--'}</em>
        </div>
        {!asset.stored_path ? (
          <p className="audio-modal-empty">当前音频缺少本地文件路径，无法播放。</p>
        ) : null}
      </section>
    </div>
  );
}
