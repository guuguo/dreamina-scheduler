import React, { useState } from 'react';
import { Eye, EyeOff } from 'lucide-react';

export function PasswordInput({ value, onChange, placeholder, className }) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="password-input-wrap">
      <input
        type={visible ? 'text' : 'password'}
        value={value}
        onChange={onChange}
        placeholder={placeholder}
        className={className}
      />
      <button
        type="button"
        className="password-input-eye icon-ghost mini"
        tabIndex={-1}
        onClick={() => setVisible((v) => !v)}
      >
        {visible ? <EyeOff size={13} /> : <Eye size={13} />}
      </button>
    </div>
  );
}
