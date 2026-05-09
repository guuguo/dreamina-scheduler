import React from 'react';

export function FilterSelect({ value, onChange, options, label }) {
  return (
    <select
      className="ui-filter-select"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      title={label}
    >
      {options.map((opt) => (
        <option key={opt.key} value={opt.key}>{opt.label}</option>
      ))}
    </select>
  );
}
