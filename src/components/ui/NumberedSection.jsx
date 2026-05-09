import React from 'react';

export function NumberedSection({ number, title, subtitle, children, className }) {
  return (
    <div className={`numbered-section${className ? ` ${className}` : ''}`}>
      <div className="numbered-section-head">
        <span className="numbered-section-badge">{number}</span>
        <h3 className="numbered-section-title">{title}</h3>
        {subtitle ? <span className="numbered-section-subtitle">{subtitle}</span> : null}
      </div>
      <div className="numbered-section-body">{children}</div>
    </div>
  );
}
