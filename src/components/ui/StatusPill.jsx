import React from 'react';

export function StatusPill({ variant = 'neutral', children, ...props }) {
  return (
    <span className={`status-pill status-pill--${variant}`} {...props}>{children}</span>
  );
}
