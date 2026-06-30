/**
 * Pure helpers for Tiptap prompt serialization.
 *
 * These functions convert between Tiptap JSON doc, plain prompt text,
 * and structured mention refs, without depending on React or Tiptap runtime.
 */

/**
 * Convert a Tiptap JSON doc into plain prompt text.
 * Each mention node is rendered as @label, matching the existing prompt format.
 */
export function tiptapDocToPromptText(doc) {
  if (!doc || !doc.content) return '';
  return walkContent(doc.content);
}

export function getPromptTextFromTiptapEditor(editor) {
  if (!editor) return '';
  const doc = typeof editor.getJSON === 'function' ? editor.getJSON() : null;
  if (doc) return tiptapDocToPromptText(doc);
  return typeof editor.getText === 'function' ? editor.getText() : '';
}

function walkContent(content) {
  if (!content) return '';
  let text = '';
  for (const node of content) {
    if (node.type === 'text') {
      text += node.text || '';
    } else if (node.type === 'mention') {
      const label = node.attrs?.label || '';
      text += `@${label}`;
    } else if (node.type === 'paragraph') {
      if (text.length > 0) text += '\n';
      text += walkContent(node.content);
    } else if (node.type === 'hardBreak') {
      text += '\n';
    } else {
      text += walkContent(node.content);
    }
  }
  return text;
}

/**
 * Extract mention refs from a Tiptap JSON doc, grouped by type.
 * Returns { roleIds, imageAssetIds, audioAssetIds, tempImageAssetIds }.
 */
export function extractMentionRefsFromTiptapDoc(doc) {
  const refs = {
    roleIds: [],
    imageAssetIds: [],
    audioAssetIds: [],
    tempImageAssetIds: [],
  };
  if (!doc || !doc.content) return refs;
  collectMentionRefs(doc.content, refs);
  // deduplicate while preserving order
  refs.roleIds = uniqueInOrder(refs.roleIds);
  refs.imageAssetIds = uniqueInOrder(refs.imageAssetIds);
  refs.audioAssetIds = uniqueInOrder(refs.audioAssetIds);
  refs.tempImageAssetIds = uniqueInOrder(refs.tempImageAssetIds);
  return refs;
}

function collectMentionRefs(content, refs) {
  if (!content) return;
  for (const node of content) {
    if (node.type === 'mention') {
      const attrs = node.attrs || {};
      let { type, roleId, assetId } = attrs;
      const id = attrs.id || '';
      // Fallback: decode type+ids from key prefix if custom attrs were not persisted
      if (!type && id) {
        if (id.startsWith('img:')) { type = 'image'; assetId = assetId || id.slice(4); }
        else if (id.startsWith('temp:')) { type = 'temp_image'; assetId = assetId || id.slice(5); }
        else if (id.startsWith('role:')) { type = 'role'; roleId = roleId || id.slice(5); }
        else if (id.startsWith('audio:')) { type = 'audio'; assetId = assetId || id.slice(6); }
      }
      if (type === 'role' && roleId) {
        refs.roleIds.push(roleId);
      } else if ((type === 'image' || type === 'temp_image') && assetId) {
        refs.imageAssetIds.push(assetId);
        if (type === 'temp_image') refs.tempImageAssetIds.push(assetId);
      } else if (type === 'audio' && assetId) {
        refs.audioAssetIds.push(assetId);
      }
    }
    if (node.content) collectMentionRefs(node.content, refs);
  }
}

/**
 * Apply extracted mention refs to a taskForm object.
 *
 * role_ids = (roles added via role picker) ∪ (roles from @ mention nodes)
 * manual_mention_ids = only the mention-derived role ids
 *
 * This ensures that roles selected via the role picker are never wiped by
 * a mention rescan, while stale mention-derived roles are removed when their
 * mention nodes are deleted.
 */
export function applyMentionRefsToTaskForm(form, refs) {
  const nonMentionRoleIds = (form.role_ids || []).filter(
    (id) => !(form.manual_mention_ids || []).includes(id)
  );
  return {
    ...form,
    role_ids: uniqueInOrder([...nonMentionRoleIds, ...refs.roleIds]),
    manual_mention_ids: refs.roleIds,
    image_asset_ids: uniqueInOrder([...refs.imageAssetIds, ...(form.temp_image_asset_ids || [])]),
    temp_image_asset_ids: uniqueInOrder([...(form.temp_image_asset_ids || []), ...(refs.tempImageAssetIds || [])]),
    audio_asset_ids: refs.audioAssetIds,
  };
}

/**
 * Doc-first editor update: persist the Tiptap JSON doc as the source of truth
 * alongside the derived plain prompt text and mention-based bindings.
 *
 * prompt_doc is what the editor reloads from (stable highlight, no label-guessing),
 * while prompt + *_asset_ids stay derived for downstream (MCP, scheduler).
 */
export function applyEditorUpdate(form, { plainText, refs, doc }) {
  const withRefs = applyMentionRefsToTaskForm({ ...form, prompt: plainText }, refs);
  return { ...withRefs, prompt_doc: doc || null };
}

/**
 * Convert a plain prompt text + mentionItems into a Tiptap JSON doc.
 * Recognised @label patterns become mention nodes; everything else is plain text.
 */
export function promptTextToTiptapDoc(prompt, mentionItems) {
  if (!prompt) return { type: 'doc', content: [{ type: 'paragraph' }] };

  const paragraphs = prompt.split('\n');
  const content = paragraphs.map((line) => {
    const inline = parseInlineContent(line, mentionItems);
    return { type: 'paragraph', content: inline.length ? inline : undefined };
  });

  return { type: 'doc', content };
}

export function shouldBlockPromptTextInput({ maxLength, currentLength, selectedLength = 0, from, to, text }) {
  if (!maxLength) return false;
  const insertedLength = String(text || '').length;
  if (from !== to) {
    const replacedLength = selectedLength || Math.max(0, Number(to) - Number(from));
    const nextLength = currentLength - replacedLength + insertedLength;
    return nextLength > maxLength && insertedLength > replacedLength;
  }
  return currentLength + insertedLength > maxLength;
}

export function getAllowedPromptPasteText({ maxLength, currentLength, selectedLength = 0, text }) {
  const pasteText = String(text || '');
  if (!maxLength) return pasteText;
  const capacity = Math.max(0, Number(maxLength) - Math.max(0, Number(currentLength) - Number(selectedLength || 0)));
  return pasteText.slice(0, capacity);
}

export function shouldSyncExternalPromptValue({
  editorText = '',
  externalValue = '',
  lastExternalValue = '',
  isInternalUpdate = false,
} = {}) {
  if (isInternalUpdate) return false;
  if (externalValue === lastExternalValue) return false;
  return String(editorText || '') !== String(externalValue || '');
}

function parseInlineContent(text, mentionItems) {
  if (!text) return [];
  const parts = [];
  const regex = /@([^\s@]+)/g;
  let lastIndex = 0;
  let match;
  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push({ type: 'text', text: text.slice(lastIndex, match.index) });
    }
    const label = match[1];
    const found = mentionItems.find((item) => item.label === label);
    if (found) {
      parts.push({
        type: 'mention',
        attrs: {
          id: found.key,
          label: found.label,
          type: found.type,
          roleId: found.roleId || '',
          assetId: found.assetId || '',
        },
      });
    } else {
      parts.push({ type: 'text', text: match[0] });
    }
    lastIndex = match.index + match[0].length;
  }
  if (lastIndex < text.length) {
    parts.push({ type: 'text', text: text.slice(lastIndex) });
  }
  return parts;
}

function uniqueInOrder(arr) {
  const seen = new Set();
  return arr.filter((item) => {
    if (seen.has(item)) return false;
    seen.add(item);
    return true;
  });
}
