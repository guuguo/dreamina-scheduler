import React from 'react';
import { Search } from 'lucide-react';

export function SearchBox({ value, onChange, placeholder }) {
  return (
    <div className="ui-search-box">
      <Search size={14} className="ui-search-box__icon" />
      <input
        type="text"
        className="ui-search-box__input"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder || '搜索...'}
      />
    </div>
  );
}
