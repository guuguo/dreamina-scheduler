import React, { useState } from 'react';
import { Copy, Check } from 'lucide-react';

export function CopyableInput({ value, onChange, placeholder, readOnly }) {
  const [copied, setCopied] = useState(false);
  function copy() {
    navigator.clipboard.writeText(value || '').then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    });
  }
  return (
    <div className="copyable-input-wrap">
      <input
        value={value}
        onChange={onChange}
        placeholder={placeholder}
        readOnly={readOnly}
      />
      <button
        type="button"
        className="copyable-input-btn icon-ghost mini"
        tabIndex={-1}
        onClick={copy}
        title="复制"
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
    </div>
  );
}
