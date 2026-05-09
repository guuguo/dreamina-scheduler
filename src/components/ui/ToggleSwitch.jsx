import React from 'react';

export function ToggleSwitch({ checked, onChange, label, hint, disabled }) {
  return (
    <label className={`toggle-switch-row${disabled ? ' disabled' : ''}`}>
      <span className="toggle-switch-label">{label}</span>
      <span
        role="checkbox"
        aria-checked={checked}
        tabIndex={disabled ? -1 : 0}
        className={`toggle-track${checked ? ' checked' : ''}${disabled ? ' disabled' : ''}`}
        onClick={() => !disabled && onChange(!checked)}
        onKeyDown={(e) => { if (!disabled && (e.key === ' ' || e.key === 'Enter')) onChange(!checked); }}
      >
        <span className="toggle-thumb" />
      </span>
      {hint ? <small className="toggle-hint">{hint}</small> : null}
    </label>
  );
}
