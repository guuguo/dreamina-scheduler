export function buildMentionItems({ roles = [], assetById = new Map(), tempImagePaths = [], tempImageAssetIds = [] }) {
  const items = [];
  for (const role of roles) {
    items.push({
      key: `role:${role.id}`,
      label: role.name,
      type: 'role',
      roleId: role.id,
      roleName: role.name,
      aliases: role.aliases || [],
    });
    const media = getRoleMediaForMentions(role, assetById);
    media.images.forEach((asset, index) => {
      const assetName = asset.name || `${role.name}图${index + 1}`;
      items.push({
        key: `img:${asset.id}`,
        label: `${role.name}${assetName}`,
        type: 'image',
        roleId: role.id,
        roleName: role.name,
        assetId: asset.id,
        assetName,
        storedPath: asset.stored_path,
        mime: asset.mime || '',
        createdAt: asset.created_at || '',
        sourceHint: `角色库 / ${role.name}`,
      });
    });
    media.audios.forEach((asset, index) => {
      const assetName = asset.name || `${role.name}音频${index + 1}`;
      items.push({
        key: `aud:${asset.id}`,
        label: `${role.name}${assetName}`,
        type: 'audio',
        roleId: role.id,
        roleName: role.name,
        assetId: asset.id,
        assetName,
        storedPath: asset.stored_path,
        duration_seconds: asset.duration_seconds || null,
        mime: asset.mime || '',
        createdAt: asset.created_at || '',
        sourceHint: `角色库 / ${role.name}`,
      });
    });
  }
  tempImagePaths.forEach((path, index) => {
    items.push({
      key: `temp:${tempImageAssetIds[index] || index}`,
      label: `分镜图${index + 1}`,
      type: 'temp_image',
      assetId: tempImageAssetIds[index] || '',
      path,
      storedPath: path,
    });
  });

  const currentTempIds = new Set(tempImageAssetIds.filter(Boolean));
  const TEN_DAYS_MS = 10 * 24 * 60 * 60 * 1000;
  const THREE_DAYS_MS = 3 * 24 * 60 * 60 * 1000;
  const now = Date.now();

  assetById.forEach((asset, id) => {
    if (
      asset.kind === 'image' &&
      Array.isArray(asset.tags) &&
      asset.tags.includes('temp_image') &&
      !currentTempIds.has(id)
    ) {
      const createdMs = asset.created_at ? new Date(asset.created_at).getTime() : 0;
      if (now - createdMs <= TEN_DAYS_MS) {
        items.push({
          key: `temp:${asset.id}`,
          label: asset.name || `临时图_${asset.id.slice(-6)}`,
          type: 'temp_image',
          assetId: asset.id,
          path: asset.stored_path,
          storedPath: asset.stored_path,
          isRecent: now - createdMs <= THREE_DAYS_MS,
          mime: asset.mime || '',
          createdAt: asset.created_at || '',
          sourceHint: '临时图库',
        });
      }
    }
  });

  return items;
}

export function applyMentionSelection({ form, item, atQuery }) {
  const prompt = form.prompt || '';
  const start = atQuery?.start ?? prompt.length;
  const end = findMentionEnd(prompt, start);
  const insert = `@${item.label} `;
  const next = {
    ...form,
    prompt: `${prompt.slice(0, start)}${insert}${prompt.slice(end)}`,
  };
  if (item.type === 'role' && item.roleId) {
    next.role_ids = uniqueValues([...(form.role_ids || []), item.roleId]);
    next.manual_mention_ids = uniqueValues([...(form.manual_mention_ids || []), item.roleId]);
  }
  if ((item.type === 'image' || item.type === 'temp_image') && item.assetId) {
    next.image_asset_ids = uniqueValues([...(form.image_asset_ids || []), item.assetId]);
  }
  if (item.type === 'audio' && item.assetId) {
    next.audio_asset_ids = uniqueValues([...(form.audio_asset_ids || []), item.assetId]);
  }
  return next;
}

export function collectPromptMentions(prompt, mentionItems) {
  const mentions = [];
  const regex = /@([^\s@]+)/g;
  let match;
  while ((match = regex.exec(prompt || '')) !== null) {
    const name = match[1];
    const found = mentionItems.find((item) => item.label === name);
    mentions.push({ text: match[0], name, matched: !!found, type: found?.type || 'unknown' });
  }
  return mentions;
}

export function highlightPromptMentions(prompt, mentionItems) {
  const parts = [];
  const regex = /@([^\s@]+)/g;
  let match;
  let lastIndex = 0;
  while ((match = regex.exec(prompt || '')) !== null) {
    if (match.index > lastIndex) {
      parts.push({ type: 'text', text: prompt.slice(lastIndex, match.index) });
    }
    const found = mentionItems.find((item) => item.label === match[1]);
    parts.push({
      type: 'mention',
      text: match[0],
      matched: !!found,
      mentionType: found?.type || 'unknown',
    });
    lastIndex = match.index + match[0].length;
  }
  if (lastIndex < (prompt || '').length) {
    parts.push({ type: 'text', text: prompt.slice(lastIndex) });
  }
  return parts;
}

function findMentionEnd(prompt, start) {
  const typedMention = prompt.slice(start).match(/^@[^\s@]*/);
  if (typedMention) return start + typedMention[0].length;
  const nextSpace = prompt.indexOf(' ', start);
  return nextSpace === start ? start + 1 : nextSpace === -1 ? start : nextSpace;
}

function getRoleMediaForMentions(role, assetById) {
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
    images: all.filter((asset) => asset.kind === 'image'),
    audios: all.filter((asset) => asset.kind === 'audio'),
  };
}

function uniqueValues(values) {
  return Array.from(new Set((values || []).filter(Boolean)));
}
