import React, { useState } from 'react';
import { Copy, Check } from 'lucide-react';

export function InlineCopyButton({ text, title }) {
  const [copied, setCopied] = useState(false);
  function copy() {
    navigator.clipboard.writeText(text || '').then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    });
  }
  return (
    <button
      type="button"
      className="ui-inline-copy"
      onClick={copy}
      title={title || '复制'}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}
