import React from 'react';
import { AlertCircle, AlertTriangle, Info, CheckCircle, List, FileText, Clock, Shield } from 'lucide-react';

const ICON_MAP = {
  AlertCircle, AlertTriangle, Info, CheckCircle, List, FileText, Clock, Shield,
};

export function StatCard({ icon, title, value, sub, tone }) {
  const Icon = ICON_MAP[icon] || Info;
  return (
    <div className={`ui-stat-card${tone ? ` ui-stat-card--${tone}` : ''}`}>
      <div className="ui-stat-card__icon">
        <Icon size={18} />
      </div>
      <div className="ui-stat-card__body">
        <span className="ui-stat-card__title">{title}</span>
        <strong className="ui-stat-card__value">{value}</strong>
        {sub ? <em className="ui-stat-card__sub">{sub}</em> : null}
      </div>
    </div>
  );
}
