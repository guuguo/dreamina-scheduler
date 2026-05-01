/**
 * 从 main.jsx 提取的高频 bug 区域纯函数。
 * 这些函数原先内联在 React 组件里，无法被单元测试覆盖，
 * 导致"测试全过但一用就出问题"。
 */

export function getRoleMedia(role, assetById) {
  const seen = new Set();
  const all = (role?.asset_ids || [])
    .map((id) => assetById.get(id))
    .filter(Boolean)
    .filter((asset) => {
      const key = `${asset.kind}:${asset.source_path || asset.stored_path || asset.id}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  return {
    all,
    images: all.filter((asset) => asset.kind === 'image'),
    audios: all.filter((asset) => asset.kind === 'audio'),
  };
}

export function buildQueueStats(tasks) {
  return {
    waiting: tasks.filter((task) => ['queued', 'scheduled', 'retry_wait'].includes(task.status)).length,
    running: tasks.filter((task) => ['submitting', 'querying'].includes(task.status)).length,
    done: tasks.filter((task) => task.status === 'succeeded').length,
    paused: tasks.filter((task) => task.status === 'paused').length,
  };
}

/**
 * 拖拽文件分发逻辑 — 这是历史高频 bug 区：
 * - 从 A 角色详情进入新建角色再拖入音频，不应修改 A 角色
 * - 从 A 角色详情进入 B 角色编辑再拖入音频，只应修改 B 角色
 * - 编辑模式下必须用 editor.roleId，不用 selectedRoleId
 */
export function resolveDropTarget(context) {
  const { activeView, roleEditor, selectedRoleId } = context;
  if (activeView === 'roles' && roleEditor?.mode === 'create') {
    return { type: 'create' };
  }
  if (activeView === 'roles' && roleEditor?.mode === 'edit' && roleEditor.roleId) {
    return { type: 'edit', roleId: roleEditor.roleId };
  }
  if (activeView === 'roles' && selectedRoleId) {
    return { type: 'detail', roleId: selectedRoleId };
  }
  return { type: 'task-form' };
}

/**
 * 移除临时分镜图 — 需同步移除 temp_image_asset_ids 和 image_asset_ids
 */
export function removeTempImageFromForm(form, index) {
  const removedAssetId = (form.temp_image_asset_ids || [])[index];
  return {
    ...form,
    temp_image_paths: (form.temp_image_paths || []).filter((_, i) => i !== index),
    temp_image_asset_ids: (form.temp_image_asset_ids || []).filter((_, i) => i !== index),
    image_asset_ids: (form.image_asset_ids || []).filter((id) => id !== removedAssetId),
  };
}

export function buildTaskFormFromTaskForEdit(task, assetById = new Map()) {
  const persistedTempIds = task?.temp_image_asset_ids || [];
  const inferredTempIds = persistedTempIds.length
    ? persistedTempIds
    : inferTempImageAssetIds(task, assetById);
  const tempImageAssetIds = inferredTempIds.filter((id) => assetById.has(id) || (task?.image_asset_ids || []).includes(id));
  const tempImagePaths = tempImageAssetIds
    .map((id, index) => {
      const asset = assetById.get(id);
      return asset?.stored_path || (task?.temp_image_paths || [])[index] || '';
    })
    .filter(Boolean);

  return {
    title: task?.title || '',
    prompt: task?.prompt || '',
    image_asset_ids: task?.image_asset_ids || [],
    audio_asset_ids: task?.audio_asset_ids || [],
    role_ids: task?.role_ids || [],
    manual_mention_ids: task?.manual_mention_ids || [],
    auto_match_roles: task?.auto_match_roles ?? true,
    scheduled_at: '',
    temp_image_paths: tempImagePaths,
    temp_image_asset_ids: tempImageAssetIds,
    params: {
      model_version: task?.params?.model_version || 'seedance2.0',
      ratio: task?.params?.ratio || '9:16',
      duration: task?.params?.duration || 15,
      video_resolution: task?.params?.video_resolution || '720p',
    },
  };
}

export function buildTaskFormFromTaskForDuplicate(task, assetById = new Map()) {
  const editForm = buildTaskFormFromTaskForEdit(task, assetById);
  return {
    ...editForm,
    title: '',
    image_asset_ids: [...(editForm.image_asset_ids || [])],
    audio_asset_ids: [...(editForm.audio_asset_ids || [])],
    role_ids: [...(editForm.role_ids || [])],
    manual_mention_ids: [...(editForm.manual_mention_ids || [])],
    temp_image_paths: [...(editForm.temp_image_paths || [])],
    temp_image_asset_ids: [...(editForm.temp_image_asset_ids || [])],
    params: { ...(editForm.params || {}) },
    scheduled_at: '',
  };
}

function inferTempImageAssetIds(task, assetById) {
  const prompt = task?.prompt || '';
  const hasStoryboardMention = /@分镜图\d*/.test(prompt);
  return (task?.image_asset_ids || []).filter((id) => {
    const asset = assetById.get(id);
    if (!asset || asset.kind !== 'image') return false;
    const name = asset.name || '';
    const tags = asset.tags || [];
    return hasStoryboardMention
      || name.includes('临时图片')
      || name.includes('粘贴图片')
      || tags.includes('temporary')
      || tags.includes('clipboard');
  });
}

/**
 * 角色保存逻辑 — 历史高频 bug：
 * - 编辑角色保存后，原有素材引用应保留，新导入素材应追加且去重
 * - 新建角色保存后，asset_ids 只包含本次新建表单中的待导入素材
 */
export function computeRoleAssetIdsOnSave({ mode, existingAssetIds = [], newAssetIds = [] }) {
  if (mode === 'create') {
    return [...new Set(newAssetIds)];
  }
  // edit: 保留原有 + 追加新导入，去重
  return [...new Set([...existingAssetIds, ...newAssetIds])];
}

/**
 * 删除角色素材时的上下文解析 — 历史高频 bug：
 * - 从角色详情页删除，用 selectedRoleId
 * - 从角色编辑页删除，用 roleEditor.roleId
 * - 两者不应混淆
 */
export function resolveRemoveMediaTarget(context) {
  const { roleEditor, selectedRoleId } = context;
  if (roleEditor?.mode === 'edit' && roleEditor.roleId) {
    return roleEditor.roleId;
  }
  return selectedRoleId || null;
}
