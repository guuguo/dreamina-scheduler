import React from 'react';

export function Waveform({ active = false }) {
  const bars = [8, 18, 28, 16, 34, 22, 40, 14, 32, 24, 44, 18, 36, 26, 30, 16, 38, 20, 34, 12, 28, 22, 40, 18, 30, 14, 36, 24];
  return (
    <svg
      className={`waveform${active ? ' active' : ''}`}
      viewBox="0 0 196 48"
      preserveAspectRatio="xMidYMid slice"
      aria-hidden="true"
      focusable="false"
    >
      <rect className="waveform-track" x="0" y="0" width="196" height="48" rx="24" />
      <g className="waveform-bars">
        {bars.map((height, index) => {
          const x = 12 + index * 6.2;
          const y = (48 - height) / 2;
          return <rect key={`${height}-${index}`} x={x} y={y} width="2.8" height={height} rx="1.4" />;
        })}
      </g>
    </svg>
  );
}
