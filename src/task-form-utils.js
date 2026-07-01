import {
  applyMentionRefsToTaskForm,
  extractMentionRefsFromTiptapDoc,
  promptTextToTiptapDoc,
} from './prompt-editor-utils.js';

export const DEFAULT_CREATE_TASK_PRESET = '根据图片 @图片3    本生成写实邵氏兄弟港片风格视频，女主是 @女主厨师服 声音： @女主女主人声音  ，黑白猫猫声音 @黑白猫和灰猫奶牛猫声音     。灰猫声音 @黑白猫和灰猫英短声音  。脚本：';
export const TASK_PROMPT_MAX_LENGTH = 10000;

export function canSaveTaskDraft(form) {
  return Boolean(
    String(form?.prompt || '').trim()
      || (form?.role_ids || []).length
      || (form?.image_asset_ids || []).length
      || (form?.audio_asset_ids || []).length,
  );
}

export function buildSaveTaskDraftButtonState({
  canSaveDraft,
  isEditingTask,
  savingTaskDraft,
  savingTaskDraftPhase,
}) {
  if (savingTaskDraft) {
    return {
      disabled: true,
      icon: 'loader',
      label: savingTaskDraftPhase === 'title' ? '生成标题中…' : '保存中…',
    };
  }
  return {
    disabled: !canSaveDraft,
    icon: 'plus',
    label: isEditingTask ? '保存修改' : '保存任务',
  };
}

export function createEmptyTaskForm() {
  return {
    title: '',
    prompt: '',
    image_asset_ids: [],
    audio_asset_ids: [],
    role_ids: [],
    manual_mention_ids: [],
    auto_match_roles: true,
    scheduled_at: '',
    temp_image_paths: [],
    temp_image_asset_ids: [],
    prompt_doc: null,
    params: {
      model_version: 'seedance2.0',
      ratio: '9:16',
      duration: 15,
      video_resolution: '720p',
    },
  };
}

export function canSubmitCreateTask(form) {
  return Boolean(String(form?.prompt || '').trim() && (form?.image_asset_ids || []).length);
}

export function canApplyCreateTaskPreset(form) {
  const tempImageIds = new Set(form?.temp_image_asset_ids || []);
  const nonTempImageIds = (form?.image_asset_ids || []).filter((id) => !tempImageIds.has(id));
  return !String(form?.prompt || '').trim()
    && !(form?.role_ids || []).length
    && !(form?.manual_mention_ids || []).length
    && !nonTempImageIds.length
    && !(form?.audio_asset_ids || []).length
}

export function applyCreateTaskPreset(form, mentionItems = []) {
  const presetForm = {
    ...form,
    prompt: DEFAULT_CREATE_TASK_PRESET,
    role_ids: [],
    manual_mention_ids: [],
  };
  const doc = promptTextToTiptapDoc(DEFAULT_CREATE_TASK_PRESET, mentionItems);
  const refs = extractMentionRefsFromTiptapDoc(doc);
  return applyMentionRefsToTaskForm(presetForm, refs);
}

export function applyPromptMentionsToTaskForm(form, mentionItems = []) {
  const doc = promptTextToTiptapDoc(form?.prompt || '', mentionItems);
  const refs = extractMentionRefsFromTiptapDoc(doc);
  return applyMentionRefsToTaskForm(form, refs);
}

export function getRoleEditorMedia(mode, selectedRoleMedia, roleForm) {
  if (mode !== 'create') {
    return selectedRoleMedia || { images: [], audios: [] };
  }
  return {
    images: roleForm?.imagePath ? [{ id: 'pending-image', stored_path: roleForm.imagePath, name: '待导入参考图', pending: true }] : [],
    audios: roleForm?.audioPath ? [{ id: 'pending-audio', stored_path: roleForm.audioPath, name: '待导入音色', pending: true }] : [],
  };
}

export function createEmptyRoleForm() {
  return { id: '', name: '', aliases: '', tags: '', description: '', imagePath: '', audioPath: '' };
}

export function roleToEditorForm(role) {
  if (!role) return createEmptyRoleForm();
  return {
    id: role.id || '',
    name: role.name || '',
    aliases: role.aliases?.join('，') || '',
    tags: role.tags?.join('，') || '',
    description: role.description || '',
    imagePath: '',
    audioPath: '',
  };
}

export function createRoleEditor(mode, role = null) {
  return {
    mode,
    roleId: mode === 'edit' ? role?.id || '' : null,
    form: mode === 'edit' ? roleToEditorForm(role) : createEmptyRoleForm(),
  };
}

export function patchRoleEditorForm(editor, patch) {
  if (!editor) return editor;
  const nextPatch = typeof patch === 'function' ? patch(editor.form) : patch;
  return {
    ...editor,
    form: {
      ...editor.form,
      ...nextPatch,
    },
  };
}
