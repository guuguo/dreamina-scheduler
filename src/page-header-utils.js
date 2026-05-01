export function buildSecondaryPageHeaderConfig(kind, { mode, name } = {}) {
  if (kind === 'role') {
    return {
      title: mode === 'create' ? '新建角色' : (name || '编辑角色'),
      backLabel: '返回角色列表',
    };
  }
  return {
    title: mode === 'edit'
      ? `编辑：${name || '未命名任务'}`
      : '新建 multimodal2video 任务',
    backLabel: '返回任务中心',
  };
}
