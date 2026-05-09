import React from 'react';

export function Pager({ page, totalPages, total, pageSize, onPageChange, onPageSizeChange, pageSizeOptions }) {
  const sizes = pageSizeOptions || [10, 20, 50];
  return (
    <div className="ui-pager">
      <span className="ui-pager__total">共 {total} 条</span>
      <div className="ui-pager__controls">
        <button type="button" disabled={page <= 1} onClick={() => onPageChange(page - 1)}>上一页</button>
        <span className="ui-pager__page">{page} / {totalPages}</span>
        <button type="button" disabled={page >= totalPages} onClick={() => onPageChange(page + 1)}>下一页</button>
      </div>
      <select className="ui-pager__size" value={pageSize} onChange={(e) => onPageSizeChange(Number(e.target.value))}>
        {sizes.map((s) => <option key={s} value={s}>{s} 条/页</option>)}
      </select>
    </div>
  );
}
