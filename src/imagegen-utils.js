// 生图模块表单与 mention 工具
// 与 taskForm 结构平行但仅含图片子集，避免互相污染。

export const IMAGEGEN_MAX_REFERENCES = 9;

export function createEmptyImageGenForm() {
  return {
    prompt: '',
    size: '1024x1024',
    temp_image_paths: [],
    temp_image_asset_ids: [],
    image_asset_ids: [],
  };
}

// Picker 仅展示对生图有意义的条目：role / image / temp_image。
// 角色本身不直接做参考图，但保留入口便于一次性把角色挂载图全选。
export function filterMentionItemsForImageGen(items) {
  return (items || []).filter((item) => item.type === 'role' || item.type === 'image' || item.type === 'temp_image');
}

function uniq(list) {
  const seen = new Set();
  const out = [];
  for (const v of list || []) {
    if (v == null) continue;
    if (seen.has(v)) continue;
    seen.add(v);
    out.push(v);
  }
  return out;
}

// 追加一个素材作为参考图，超过上限则原样返回。
export function addReferenceAsset(form, asset) {
  if (!asset || !asset.id) return form;
  const ids = form.image_asset_ids || [];
  if (ids.includes(asset.id)) return form;
  if (ids.length >= IMAGEGEN_MAX_REFERENCES) return form;

  const isTemp = !!asset.tags && Array.isArray(asset.tags) && asset.tags.includes('temp_image');
  return {
    ...form,
    image_asset_ids: uniq([...ids, asset.id]),
    temp_image_asset_ids: isTemp
      ? uniq([...(form.temp_image_asset_ids || []), asset.id])
      : (form.temp_image_asset_ids || []),
    temp_image_paths: isTemp && asset.stored_path
      ? uniq([...(form.temp_image_paths || []), asset.stored_path])
      : (form.temp_image_paths || []),
  };
}

export function removeReferenceAsset(form, assetId, assetById) {
  if (!assetId) return form;
  const newTempAssetIds = (form.temp_image_asset_ids || []).filter((id) => id !== assetId);
  const newTempPaths = assetById
    ? newTempAssetIds.map((id) => assetById.get(id)?.stored_path).filter(Boolean)
    : (form.temp_image_paths || []);
  return {
    ...form,
    image_asset_ids: (form.image_asset_ids || []).filter((id) => id !== assetId),
    temp_image_asset_ids: newTempAssetIds,
    temp_image_paths: newTempPaths,
  };
}

// 将 PromptMentionEditor 抽出的 refs 应用到生图表单。
// 仅处理图片，role_ids / audio 一律忽略。
// preserveAssetIds：来自手动添加（按钮 / 粘贴）的素材 id 集合，避免被 mention 重扫覆盖。
export function applyMentionRefsToImageGenForm(form, refs, preserveAssetIds = []) {
  const fromMentions = refs?.imageAssetIds || [];
  return {
    ...form,
    image_asset_ids: uniq([...preserveAssetIds, ...fromMentions]).slice(0, IMAGEGEN_MAX_REFERENCES),
  };
}

export function shouldSuppressImageContextMenu(target) {
  const tagName = target?.tagName || target?.nodeName;
  return typeof tagName === 'string' && tagName.toLowerCase() === 'img';
}
