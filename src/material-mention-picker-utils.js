/**
 * material-mention-picker-utils.js
 * 素材候选归一化、分类过滤、角色 chip 派生、键盘网格移动等纯函数。
 */

export const CATEGORIES = ['recent', 'role_image', 'role_audio', 'temp_image'];

export const CATEGORY_LABELS = {
  recent: '最近使用',
  role_image: '角色图片',
  role_audio: '角色音频',
  temp_image: '临时图片',
};

/**
 * 把 buildMentionItems() 返回的 mentionItems 归一化为 picker items。
 * 过滤掉 type==='role'（角色本身不可直接插入）。
 */
export function normalizeMentionItems(mentionItems = []) {
  return mentionItems
    .filter((item) => item.type !== 'role')
    .map((item) => {
      const displayType =
        item.type === 'image' ? 'role_image'
          : item.type === 'audio' ? 'role_audio'
            : 'temp_image';
      const searchText = [
        item.label,
        item.assetName,
        item.roleName,
        item.type === 'audio' ? '音频' : item.type === 'image' ? '图片' : '临时图',
      ].filter(Boolean).join(' ').toLowerCase();
      return {
        key: item.key,
        label: item.label,
        type: item.type,
        displayType,
        roleId: item.roleId || '',
        roleName: item.roleName || '',
        assetId: item.assetId || '',
        storedPath: item.storedPath || item.path || '',
        durationSeconds: item.duration_seconds || null,
        insertText: `@${item.label}`,
        assetName: item.assetName || item.label,
        searchText,
        isRecent: item.isRecent || false,
        lastUsedAt: item.lastUsedAt || '',
        mime: item.mime || '',
        createdAt: item.createdAt || '',
        sourceHint: item.sourceHint || '',
      };
    });
}

/**
 * 从 normalizedItems 派生角色筛选 chips。
 * 返回 [{id:'all', label:'全部角色'}, ...角色 chips, 可能有 {id:'other', label:'其他'}]
 */
export function deriveRoleChips(normalizedItems) {
  const roleMap = new Map();
  for (const item of normalizedItems) {
    if (item.roleId && !roleMap.has(item.roleId)) {
      roleMap.set(item.roleId, item.roleName || item.roleId);
    }
  }
  const chips = [{ id: 'all', label: '全部角色' }];
  for (const [id, label] of roleMap) {
    chips.push({ id, label });
  }
  const hasNoRole = normalizedItems.some(
    (item) => !item.roleId && item.displayType !== 'temp_image',
  );
  if (hasNoRole) chips.push({ id: 'other', label: '其他' });
  return chips;
}

/**
 * 过滤素材候选。
 * @param {Array} normalizedItems
 * @param {{ query?: string, category?: string, roleId?: string }} opts
 */
export function filterItems(normalizedItems, { query = '', category = 'all', roleId = 'all' } = {}) {
  let result = normalizedItems;

  if (category === 'role_image') {
    result = result.filter((i) => i.displayType === 'role_image');
  } else if (category === 'role_audio') {
    result = result.filter((i) => i.displayType === 'role_audio');
  } else if (category === 'temp_image') {
    result = result.filter((i) => i.displayType === 'temp_image');
  }
  // 'recent' shows all items sorted by frecency (handled separately via sortByFrecency)

  if (roleId === 'other') {
    result = result.filter((i) => !i.roleId);
  } else if (roleId !== 'all') {
    result = result.filter((i) => i.roleId === roleId);
  }

  if (query.trim()) {
    const q = query.trim().toLowerCase();
    result = result.filter((i) => i.searchText.includes(q));
  }

  return result;
}

/**
 * 键盘网格焦点移动。
 * @param {number} currentIndex
 * @param {'ArrowUp'|'ArrowDown'|'ArrowLeft'|'ArrowRight'} direction
 * @param {number} cols  每行列数
 * @param {number} total 总项目数
 * @returns {number} 新索引
 */
export function moveGridFocus(currentIndex, direction, cols, total) {
  if (total === 0) return currentIndex;
  switch (direction) {
    case 'ArrowRight':
      return Math.min(currentIndex + 1, total - 1);
    case 'ArrowLeft':
      return Math.max(currentIndex - 1, 0);
    case 'ArrowDown':
      return Math.min(currentIndex + cols, total - 1);
    case 'ArrowUp':
      return Math.max(currentIndex - cols, 0);
    default:
      return currentIndex;
  }
}

/**
 * 基于 frecency（频率+新近度）排序：优先按 last_used_at 倒序，
 * 从未使用过的回落 created_at；再按 isRecent 标志微调。
 */
export function sortByFrecency(normalizedItems) {
  return [...normalizedItems].sort((a, b) => {
    const aLast = a.lastUsedAt ? new Date(a.lastUsedAt).getTime() : 0;
    const bLast = b.lastUsedAt ? new Date(b.lastUsedAt).getTime() : 0;
    // 优先：最近使用时间（倒序）
    if (aLast !== bLast) return bLast - aLast;
    // 次优：创建时间（倒序）
    const aCreated = a.createdAt ? new Date(a.createdAt).getTime() : 0;
    const bCreated = b.createdAt ? new Date(b.createdAt).getTime() : 0;
    return bCreated - aCreated;
  });
}

/**
 * 把最近使用的素材排到前面（基于 assetId 列表）。保留向后兼容。
 */
export function sortByRecent(normalizedItems, recentAssetIds = []) {
  if (!recentAssetIds.length) return normalizedItems;
  const recentSet = new Set(recentAssetIds);
  return [
    ...normalizedItems
      .filter((i) => recentSet.has(i.assetId))
      .map((i) => ({ ...i, isRecent: true })),
    ...normalizedItems.filter((i) => !recentSet.has(i.assetId)),
  ];
}
