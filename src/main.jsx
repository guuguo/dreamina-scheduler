import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  getRoleMedia,
  buildQueueStats,
  resolveDropTarget,
  removeTempImageFromForm,
  computeRoleAssetIdsOnSave,
  resolveRemoveMediaTarget,
  buildTaskFormFromTaskForDuplicate,
  buildTaskFormFromTaskForEdit,
} from './app-logic.js';
import { buildMentionItems } from './mention-utils.js';
import PromptMentionEditor from './components/PromptMentionEditor.jsx';
import {
  deriveCurrentExecutionRecord,
  deriveCurrentQueryRecords,
  deriveTaskHistory,
  historyItemLabel,
  isInterruptNotice,
} from './task-history-utils.js';
import { buildSecondaryPageHeaderConfig } from './page-header-utils.js';
import {
  applyCreateTaskPreset,
  canApplyCreateTaskPreset,
  canSaveTaskDraft,
  createEmptyTaskForm,
  createRoleEditor,
  getRoleEditorMedia,
  patchRoleEditorForm,
  roleToEditorForm,
  createEmptyRoleForm,
  TASK_PROMPT_MAX_LENGTH,
  applyPromptMentionsToTaskForm,
} from './task-form-utils.js';
import {
  fileExt,
  isImagePath,
  isAudioPath,
  isSupportedRoleMedia,
  uniqueFilePaths,
  splitCsv as splitCsvUtil,
} from './media-utils.js';
import {
  Activity,
  AlertCircle,
  ArrowLeft,
  Bell,
  CalendarClock,
  Camera,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ClipboardList,
  Clock3,
  Command,
  Copy,
  Download,
  ExternalLink,
  FileAudio,
  FolderOpen,
  Gauge,
  Grid,
  Home,
  Image,
  ImagePlus,
  LayoutList,
  ListChecks,
  Loader2,
  Logs,
  MoreHorizontal,
  Pencil,
  Play,
  Plus,
  RefreshCcw,
  Save,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  Star,
  Trash2,
  Upload,
  User,
  Coins,
  Users,
  X,
  Zap,
  ZoomIn,
} from 'lucide-react';
import {
  filterTasks,
  sortTasks,
  paginateTasks,
  formatPaginationLabel,
  deriveTaskFlowSteps,
  deriveTaskProgress,
  getModelOptions,
  deriveQueueStats,
  canDeleteTask,
  getTaskResultItems,
  getTaskHitResources,
  getCommandPreviewPresentation,
} from './queue-view-utils.js';
import {
  buildBatchSchedulePlan,
  canScheduleTask,
  formatSchedulePlanSummary,
  resolvePrepareGenerateOperation,
  resolveScheduleAt,
} from './schedule-utils.js';
import { Thumb } from './components/media/Thumb.jsx';
import { Waveform } from './components/media/Waveform.jsx';
import { ImageModal } from './components/media/ImageModal.jsx';
import { AudioPreviewRow } from './components/media/AudioPreviewRow.jsx';
import { AudioAssetModal } from './components/media/AudioAssetModal.jsx';
import { AiThinkingModal } from './components/AiThinkingModal.jsx';
import './styles.css';

const ratios = ['9:16', '16:9', '1:1', '3:4', '4:3', '21:9'];
const modelVersions = ['seedance2.0', 'seedance2.0fast'];
const durationOptions = Array.from({ length: 12 }, (_, index) => index + 4);
const defaultAiModelConfig = {
  id: 'default-openai',
  name: 'OpenAI 默认',
  api_mode: 'responses',
  api_key: '',
  base_url: 'https://api.openai.com/v1',
  model: 'gpt-5.4',
};
const views = [
  { id: 'dashboard', label: '仪表盘', icon: Home },
  { id: 'roles', label: '角色库', icon: User },
  { id: 'queue', label: '任务中心', icon: ListChecks },
  { id: 'logs', label: '日志', icon: Logs },
  { id: 'settings', label: '设置', icon: Settings },
];

const emptyState = {
  settings: {
    concurrency_limit_policy: 'SilentRetry',
    concurrency_retry_delay_seconds: 300,
    concurrency_retry_max_attempts: 8,
    auto_query_enabled: true,
    poll_interval_seconds: 60,
    log_retention_count: 500,
    mac_install_command: 'curl -fsSL https://jimeng.jianying.com/cli | bash',
    windows_install_command: '',
    ai_model_configs: [defaultAiModelConfig],
    active_ai_model_id: defaultAiModelConfig.id,
    prevent_sleep: true,
  },
  assets: [],
  roles: [],
  tasks: [],
  logs: [],
};

function App() {
  const [activeView, setActiveView] = useState('dashboard');
  const [state, setState] = useState(emptyState);
  const [cli, setCli] = useState({ available: false, path: '', message: '等待检测' });
  const [hostPlatform, setHostPlatform] = useState({ label: 'Desktop' });
  const [feedback, setFeedback] = useState('');
  useEffect(() => {
    if (!feedback) return;
    const t = setTimeout(() => setFeedback(''), 3000);
    return () => clearTimeout(t);
  }, [feedback]);
  const [pendingTaskOps, setPendingTaskOps] = useState({});
  const [pendingExecutionOps, setPendingExecutionOps] = useState({});
  const tickLockRef = useRef(false);
  const refreshStateRef = useRef(null);
  const [lastTickAt, setLastTickAt] = useState(null);
  const [selectedTaskId, setSelectedTaskId] = useState('');
  const [selectedRoleId, setSelectedRoleId] = useState('');
  const [roleSearchQuery, setRoleSearchQuery] = useState('');
  const [roleActiveTab, setRoleActiveTab] = useState('all');
  const [roleViewMode, setRoleViewMode] = useState('grid');
  const [roleEditor, setRoleEditor] = useState(null);
  const [dragActive, setDragActive] = useState(false);
  const [confirmModal, setConfirmModal] = useState(null);
  const [creditInfo, setCreditInfo] = useState({ available: false, total: '', used: '', remaining: '', raw_text: '' });
  const [creditModalOpen, setCreditModalOpen] = useState(false);
  const [settingsForm, setSettingsForm] = useState(emptyState.settings);
  const roleForm = roleEditor?.form || createEmptyRoleForm();
  const setRoleForm = (patch) => {
    setRoleEditor((current) => patchRoleEditorForm(current, patch));
  };
  const [taskForm, setTaskForm] = useState(() => createEmptyTaskForm());
  const [editingTaskId, setEditingTaskId] = useState(null);
  const dropContextRef = useRef({ activeView, selectedRoleId, roleEditor });

  async function refreshState() {
    try {
      const next = await invoke('get_app_state');
      setState(next);
      setSettingsForm(next.settings || emptyState.settings);
      if (!selectedTaskId && next.tasks?.[0]) setSelectedTaskId(next.tasks[0].id);
      invoke('sync_keep_awake_command').catch(() => {});
    } catch (error) {
      setFeedback(`读取状态失败：${String(error)}`);
    }
  }
  // 每次渲染都更新 ref，让 useEffect([]) 内的闭包始终拿到最新版本
  refreshStateRef.current = refreshState;

  async function checkCli() {
    const result = await invoke('check_dreamina_cli');
    setCli(result);
    if (result.available) {
      refreshCredit().catch(() => {});
    }
  }

  async function refreshCredit() {
    try {
      const info = await invoke('get_dreamina_credit');
      setCreditInfo(info);
    } catch (_e) {
      setCreditInfo({ available: false, total: '', used: '', remaining: '', raw_text: '' });
    }
  }

  async function checkHostPlatform() {
    const result = await invoke('get_host_platform');
    setHostPlatform(result);
  }

  useEffect(() => {
    refreshState();
    checkHostPlatform().catch(() => {});
    checkCli().catch((error) => setFeedback(`CLI 检测失败：${String(error)}`));
  }, []);

  useEffect(() => {
    dropContextRef.current = { activeView, selectedRoleId, roleEditor };
  }, [activeView, selectedRoleId, roleEditor]);

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'enter' || event.payload.type === 'over') {
        setDragActive(true);
        return;
      }
      if (event.payload.type === 'leave') {
        setDragActive(false);
        return;
      }
      if (event.payload.type === 'drop') {
        setDragActive(false);
        handleDroppedFiles(event.payload.paths || [], dropContextRef.current);
      }
    }).then((dispose) => {
      if (disposed) {
        dispose();
      } else {
        unlisten = dispose;
      }
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  // T003：应用启动后立即执行调度检查，随后每 30 秒轮询一次，无用户开关
  useEffect(() => {
    let cancelled = false;
    async function tick() {
      if (tickLockRef.current) return;
      tickLockRef.current = true;
      try {
        await invoke('process_queue_command');
        if (!cancelled) {
          setLastTickAt(new Date());
          await refreshStateRef.current?.();
        }
      } catch (_e) {
        // 内部调度静默处理，不干扰用户操作
      } finally {
        tickLockRef.current = false;
      }
    }
    tick();
    const timer = window.setInterval(tick, 30000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // T004：窗口重新获得焦点或系统休眠恢复后立即触发一次补偿检查
  useEffect(() => {
    async function onWake() {
      if (tickLockRef.current) return;
      tickLockRef.current = true;
      try {
        await invoke('process_queue_command');
        setLastTickAt(new Date());
        await refreshStateRef.current?.();
      } catch (_e) {
        // 静默处理
      } finally {
        tickLockRef.current = false;
      }
    }
    const handleFocus = () => onWake();
    const handleVisibility = () => { if (!document.hidden) onWake(); };
    window.addEventListener('focus', handleFocus);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      window.removeEventListener('focus', handleFocus);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, []);

  const assetById = useMemo(() => new Map(state.assets.map((asset) => [asset.id, asset])), [state.assets]);
  const selectedRole = useMemo(
    () => state.roles.find((role) => role.id === selectedRoleId) || null,
    [selectedRoleId, state.roles],
  );
  const selectedRoleMedia = useMemo(() => getRoleMedia(selectedRole, assetById), [assetById, selectedRole]);
  const queueStats = useMemo(() => buildQueueStats(state.tasks), [state.tasks]);

  useEffect(() => {
    if (activeView === 'roles' && !roleEditor && !selectedRoleId && state.roles.length) {
      setSelectedRoleId(state.roles[0].id);
    }
  }, [activeView, roleEditor, selectedRoleId, state.roles]);

  async function importRoleMedia(path, name) {
    if (!path.trim()) return null;
    return invoke('import_asset', { input: { path: path.trim(), name: name || null } });
  }

  async function importFilesToRole(roleId, paths) {
    if (!roleId) {
      setFeedback('请先选择一个角色，再导入图片或音频');
      return;
    }
    const uniquePaths = uniqueFilePaths(paths);
    if (!uniquePaths.length) return;
    await invoke('import_role_media_command', {
      input: {
        role_id: roleId,
        paths: uniquePaths,
      },
    });
    setFeedback(`已导入 ${uniquePaths.length} 个角色资源`);
    await refreshState();
  }

  async function importFilesToSelectedRole(paths) {
    await importFilesToRole(selectedRoleId, paths);
  }

  async function chooseFilesForSelectedRole() {
    try {
      const selected = await open({
        multiple: true,
        directory: false,
        title: '选择角色图片或音频',
        filters: [
          { name: '角色图片和音频', extensions: ['png', 'jpg', 'jpeg', 'webp', 'mp3', 'wav', 'm4a', 'aac'] },
        ],
      });
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (roleEditor?.mode === 'create') {
        const image = paths.find((path) => isImagePath(path));
        const audio = paths.find((path) => isAudioPath(path));
        setRoleForm((current) => ({
          ...current,
          imagePath: image || current.imagePath || '',
          audioPath: audio || current.audioPath || '',
        }));
        setFeedback('已填入新角色表单，保存后会复制到 App 内缓存');
        return;
      }
      if (roleEditor?.mode === 'edit' && roleEditor.roleId) {
        await importFilesToRole(roleEditor.roleId, paths);
        return;
      }
      await importFilesToSelectedRole(paths);
    } catch (error) {
      setFeedback(String(error));
    }
  }

  function askConfirm(config) {
    setConfirmModal({
      confirmText: '确认',
      cancelText: '取消',
      tone: 'danger',
      ...config,
    });
  }

  async function runConfirmedAction() {
    if (!confirmModal?.onConfirm) return;
    try {
      await confirmModal.onConfirm();
      setConfirmModal(null);
    } catch (error) {
      setConfirmModal(null);
      setFeedback(String(error));
    }
  }

  async function deleteRole(roleId) {
    await invoke('delete_role_command', { roleId });
    setFeedback('角色已删除');
    setSelectedRoleId('');
    await refreshState();
  }

  async function removeRoleMedia(assetId, _roleId) {
    const roleId = _roleId ?? resolveRemoveMediaTarget({ roleEditor, selectedRoleId });
    if (!roleId) {
      setFeedback('请先选择要移除资源的角色');
      return;
    }
    await invoke('remove_role_media_command', {
      input: {
        role_id: roleId,
        asset_id: assetId,
      },
    });
    setFeedback('角色资源已移除');
    await refreshState();
  }

  async function renameAsset(assetId, newName) {
    try {
      await invoke('rename_asset', { assetId, newName });
      setFeedback('素材已重命名');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function chooseInitialRoleFile(kind) {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: kind === 'image' ? '选择角色参考图' : '选择角色音频',
        filters: [
          kind === 'image'
            ? { name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp'] }
            : { name: '音频', extensions: ['mp3', 'wav', 'm4a', 'aac'] },
        ],
      });
      if (!selected || Array.isArray(selected)) return;
      setRoleForm((current) => ({
        ...current,
        [kind === 'image' ? 'imagePath' : 'audioPath']: selected,
      }));
    } catch (error) {
      setFeedback(String(error));
    }
  }

  function handleDroppedFiles(paths, context = dropContextRef.current) {
    const usable = uniqueFilePaths(paths).filter((path) => isSupportedRoleMedia(path));
    if (!usable.length) {
      setFeedback('只支持 png、jpg、jpeg、webp 图片，以及 mp3、wav、m4a、aac 音频');
      return;
    }
    const dropCtx = {
      activeView: context?.activeView ?? activeView,
      roleEditor: context?.roleEditor ?? roleEditor,
      selectedRoleId: context?.selectedRoleId ?? selectedRoleId,
    };
    const target = resolveDropTarget(dropCtx);
    if (target.type === 'create') {
      const image = usable.find((path) => isImagePath(path));
      const audio = usable.find((path) => isAudioPath(path));
      setRoleForm((current) => ({
        ...current,
        imagePath: image || current.imagePath || '',
        audioPath: audio || current.audioPath || '',
      }));
      setFeedback('已填入新角色表单，保存后会复制到 App 内缓存');
      return;
    }
    if (target.type === 'edit') {
      importFilesToRole(target.roleId, usable).catch((error) => setFeedback(String(error)));
      return;
    }
    if (target.type === 'detail') {
      importFilesToRole(target.roleId, usable).catch((error) => setFeedback(String(error)));
      return;
    }
    const image = usable.find((path) => isImagePath(path));
    const audio = usable.find((path) => isAudioPath(path));
    setRoleForm((current) => ({
      ...current,
      imagePath: current.imagePath || image || '',
      audioPath: current.audioPath || audio || '',
    }));
    setFeedback('已填入角色创建表单，保存后会复制到 App 内缓存');
  }

  async function createRole(event, formOverride = roleEditor?.form) {
    event.preventDefault();
    const form = formOverride || createEmptyRoleForm();
    try {
      if (!form.name.trim()) {
        setFeedback('请先填写角色名');
        return;
      }
      const currentRole = state.roles.find((role) => role.id === form.id);
      if (!form.imagePath.trim() && !currentRole?.asset_ids?.length) {
        setFeedback('MVP 要求角色至少有一张参考图');
        return;
      }
      const newAssetIds = [];
      const image = await importRoleMedia(form.imagePath, `${form.name.trim()}参考图`);
      if (image) newAssetIds.push(image.id);
      const audio = await importRoleMedia(form.audioPath, `${form.name.trim()}音色`);
      if (audio) newAssetIds.push(audio.id);

      const asset_ids = computeRoleAssetIdsOnSave({
        mode: form.id ? 'edit' : 'create',
        existingAssetIds: currentRole?.asset_ids || [],
        newAssetIds,
      });

      await invoke('create_role_command', {
        input: {
          id: form.id || null,
          name: form.name.trim(),
          aliases: splitCsv(form.aliases),
          tags: splitCsv(form.tags),
          description: form.description.trim(),
          asset_ids,
        },
      });
      setRoleEditor(null);
      setFeedback('角色已保存');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function saveTaskDraft(event) {
    event?.preventDefault?.();
    let draft = {
      ...taskForm,
      image_asset_ids: taskForm.image_asset_ids || [],
      audio_asset_ids: taskForm.audio_asset_ids || [],
      scheduled_at: null,
    };
    try {
      if (!String(draft.title || '').trim()) {
        const generatedTitle = await generateTaskTitle(draft.prompt);
        if (generatedTitle) draft = { ...draft, title: generatedTitle };
      }
      if (editingTaskId) {
        await invoke('update_task_draft_command', { taskId: editingTaskId, draft });
        setSelectedTaskId(editingTaskId);
      } else {
        await invoke('save_task_draft_command', { draft });
      }
      setTaskForm(() => createEmptyTaskForm());
      setEditingTaskId(null);
      setFeedback('任务已保存');
      setActiveView('queue');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function submitTask(taskId) {
    if (pendingTaskOps[taskId]?.submit) return;
    setPendingTaskOps((prev) => ({ ...prev, [taskId]: { ...prev[taskId], submit: true } }));
    try {
      await invoke('submit_task_command', { taskId });
      setFeedback('已执行一次提交');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingTaskOps((prev) => ({ ...prev, [taskId]: { ...prev[taskId], submit: false } }));
    }
  }

  async function queryTask(taskId, submitId = null) {
    if (pendingTaskOps[taskId]?.query) return;
    setPendingTaskOps((prev) => ({ ...prev, [taskId]: { ...prev[taskId], query: true } }));
    try {
      await invoke('query_task_command', { taskId, submitId });
      setFeedback('已查询任务结果');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingTaskOps((prev) => ({ ...prev, [taskId]: { ...prev[taskId], query: false } }));
    }
  }

  async function queryExecutionRecord(taskId, executionId, submitId) {
    if (pendingExecutionOps[executionId]?.query) return;
    setPendingExecutionOps((prev) => ({ ...prev, [executionId]: { ...prev[executionId], query: true } }));
    try {
      await invoke('query_task_command', { taskId, submitId });
      setFeedback('已查询执行记录结果');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingExecutionOps((prev) => ({ ...prev, [executionId]: { ...prev[executionId], query: false } }));
    }
  }

  const [installCliStatus, setInstallCliStatus] = useState('idle'); // idle | installing | success | failed
  const [loginCliStatus, setLoginCliStatus] = useState('idle'); // idle | logging | success | failed

  async function installCli() {
    setInstallCliStatus('installing');
    try {
      const msg = await invoke('install_dreamina_cli_command');
      setInstallCliStatus('success');
      setFeedback(msg);
      await refreshState();
    } catch (error) {
      setInstallCliStatus('failed');
      setFeedback(String(error));
    }
  }

  async function loginCli(headless = false) {
    setLoginCliStatus('logging');
    try {
      const msg = await invoke('login_dreamina_cli_command', { headless });
      setLoginCliStatus('success');
      setFeedback(msg);
      await refreshState();
    } catch (error) {
      setLoginCliStatus('failed');
      setFeedback(String(error));
    }
  }

  async function processQueueOnce() {
    if (tickLockRef.current) {
      setFeedback('调度正在运行中，请稍等');
      return;
    }
    tickLockRef.current = true;
    try {
      const result = await invoke('process_queue_command');
      setFeedback(result ? `任务已执行：${result.title} -> ${statusLabel(result.status)}` : '暂无到期任务');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    } finally {
      tickLockRef.current = false;
    }
  }

  async function pauseTask(taskId) {
    try {
      await invoke('pause_task_command', { taskId });
      setFeedback('任务已暂停');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function resumeTask(taskId, mode) {
    try {
      await invoke('resume_task_command', { taskId, mode });
      setFeedback('任务已恢复');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function rescheduleTask(taskId, newScheduledAt) {
    try {
      await invoke('reschedule_task_command', { taskId, newScheduledAt: newScheduledAt || '' });
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
      throw error;
    }
  }

  async function deleteTask(taskId) {
    try {
      await invoke('delete_task_command', { taskId });
      setFeedback('任务已删除');
      setSelectedTaskId('');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function clearLogs() {
    try {
      await invoke('clear_logs_command');
      setFeedback('日志已清空');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function addTempImage() {
    try {
      const selected = await open({
        multiple: true,
        filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      const imported = [];
      for (const path of paths) {
        // eslint-disable-next-line no-await-in-loop
        const name = path.split('/').pop().replace(/\.[^.]+$/, '') || '临时图片';
        const asset = await invoke('import_temp_image_command', { input: { path, name } });
        imported.push(asset);
      }
      setTaskForm((current) => ({
        ...current,
        temp_image_paths: [...current.temp_image_paths, ...imported.map((asset) => asset.stored_path)].slice(0, 9),
        temp_image_asset_ids: [...(current.temp_image_asset_ids || []), ...imported.map((asset) => asset.id)].slice(0, 9),
        image_asset_ids: uniqueValues([...(current.image_asset_ids || []), ...imported.map((asset) => asset.id)]).slice(0, 9),
      }));
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  function removeTempImage(index) {
    setTaskForm((current) => removeTempImageFromForm(current, index));
  }

  async function pasteClipboardImage(file) {
    const buffer = await file.arrayBuffer();
    const bytes = Array.from(new Uint8Array(buffer));
    const asset = await invoke('save_clipboard_image_command', {
      input: {
        file_name: file.name || 'clipboard.png',
        mime: file.type || 'image/png',
        bytes,
      },
    });
    await refreshState();
    return asset;
  }

  async function pasteSystemClipboardImage() {
    const asset = await invoke('paste_clipboard_image_command');
    await refreshState();
    return asset;
  }

  async function previewCommand() {
    try {
      const result = await invoke('preview_task_command', {
        draft: {
          ...taskForm,
          image_asset_ids: taskForm.image_asset_ids || [],
          audio_asset_ids: taskForm.audio_asset_ids || [],
          scheduled_at: null,
        },
      });
      setFeedback(result?.join(' \\\n  ') || '命令预览为空');
    } catch (error) {
      setFeedback(`预览失败：${String(error)}`);
    }
  }

  async function generateTaskTitle(prompt) {
    if (!String(prompt || '').trim()) return '';
    return await invoke('generate_task_title_command', { prompt });
  }

  async function saveSettings(event) {
    event.preventDefault();
    try {
      await invoke('update_settings_command', {
        input: {
          concurrency_limit_policy: settingsForm.concurrency_limit_policy,
          concurrency_retry_delay_seconds: Number(settingsForm.concurrency_retry_delay_seconds) || 300,
          concurrency_retry_max_attempts: Number(settingsForm.concurrency_retry_max_attempts) || 8,
          auto_query_enabled: settingsForm.auto_query_enabled ?? true,
          poll_interval_seconds: Number(settingsForm.poll_interval_seconds) || 60,
          log_retention_count: Number(settingsForm.log_retention_count) || 500,
          mac_install_command: settingsForm.mac_install_command || '',
          windows_install_command: settingsForm.windows_install_command || '',
          ai_model_configs: (settingsForm.ai_model_configs?.length ? settingsForm.ai_model_configs : [defaultAiModelConfig])
            .map((config) => ({
              ...config,
              api_mode: config.api_mode || 'responses',
              base_url: config.base_url || 'https://api.openai.com/v1',
            })),
          active_ai_model_id: settingsForm.active_ai_model_id || (settingsForm.ai_model_configs?.[0]?.id || defaultAiModelConfig.id),
          prevent_sleep: settingsForm.prevent_sleep ?? true,
        },
      });
      setFeedback('设置已保存');
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    }
  }

  function startWindowDrag(event) {
    if (event.button !== 0) return;
    if (event.target.closest('button, input, select, textarea, a')) return;
    getCurrentWindow().startDragging().catch(() => {});
  }

  return (
    <main className="desktop-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark" />
          <strong>Dreamina Scheduler</strong>
        </div>
        <nav>
          {views.map((item) => {
            const Icon = item.icon;
            return (
              <button
                key={item.id}
                type="button"
                className={activeView === item.id ? 'active' : ''}
                onClick={() => setActiveView(item.id)}
              >
                <Icon size={17} />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>
        <span className="version">v0.1.0</span>
      </aside>

      <section className="app-window">
        <header className="window-bar" data-tauri-drag-region onMouseDown={startWindowDrag}>
          <div className="window-title" data-tauri-drag-region onMouseDown={startWindowDrag}>
            <span className="traffic-spacer" data-tauri-drag-region />
            <div className="brand-mark small" />
            <strong>Dreamina Scheduler</strong>
            <span className={`cli-status ${cli.available ? 'ok' : 'bad'}`}>
              <CheckCircle2 size={13} />
              {cli.available ? 'dreamina CLI 已连接' : cli.message}
            </span>
            {cli.available ? (
              <span
                className={`cli-status credit-badge ${creditInfo.available ? 'ok' : 'neutral'}`}
                onClick={() => { refreshCredit(); setCreditModalOpen(true); }}
                title="点击查看额度详情"
              >
                <Coins size={13} />
                {creditInfo.available
                  ? (creditInfo.remaining ? `剩余 ${creditInfo.remaining}` : `总额 ${creditInfo.total}`)
                  : '额度未知'}
              </span>
            ) : null}
          </div>
          <div className="window-actions">
            <button type="button" title="重新检测 CLI" onClick={checkCli}><RefreshCcw size={16} /></button>
            <button type="button" title="通知"><Bell size={16} /></button>
            <button type="button" title="设置" onClick={() => setActiveView('settings')}><Settings size={16} /></button>
          </div>
        </header>

        {feedback ? (
          <section className="toast">
            <ShieldCheck size={16} />
            <span>{feedback}</span>
            <button type="button" onClick={() => setFeedback('')}><X size={14} /></button>
          </section>
        ) : null}

        {activeView === 'dashboard' ? (
          <Dashboard
            state={state}
            cli={cli}
            queueStats={queueStats}
            submitTask={submitTask}
            queryTask={queryTask}
            hostPlatform={hostPlatform}
            processQueueOnce={processQueueOnce}
            selectedTaskId={selectedTaskId}
            setSelectedTaskId={setSelectedTaskId}
            setActiveView={setActiveView}
          />
        ) : null}
        {activeView === 'create' ? (
          <CreateTaskView
            state={state}
            assetById={assetById}
            taskForm={taskForm}
            setTaskForm={setTaskForm}
            setActiveView={setActiveView}
            saveTaskDraft={saveTaskDraft}
            editingTaskId={editingTaskId}
            cli={cli}
            addTempImage={addTempImage}
            removeTempImage={removeTempImage}
            previewCommand={previewCommand}
            generateTaskTitle={generateTaskTitle}
            pasteClipboardImage={pasteClipboardImage}
            pasteSystemClipboardImage={pasteSystemClipboardImage}
          />
        ) : null}
        {activeView === 'roles' ? (
          <RolesView
            state={state}
            assetById={assetById}
            roleForm={roleForm}
            setRoleForm={setRoleForm}
            createRole={createRole}
            chooseInitialRoleFile={chooseInitialRoleFile}
            chooseFilesForSelectedRole={chooseFilesForSelectedRole}
            askConfirm={askConfirm}
            deleteRole={deleteRole}
            removeRoleMedia={removeRoleMedia}
            renameAsset={renameAsset}
            dragActive={dragActive}
            selectedRole={selectedRole}
            selectedRoleMedia={selectedRoleMedia}
            selectedRoleId={selectedRoleId}
            setSelectedRoleId={setSelectedRoleId}
            roleSearchQuery={roleSearchQuery}
            setRoleSearchQuery={setRoleSearchQuery}
            roleActiveTab={roleActiveTab}
            setRoleActiveTab={setRoleActiveTab}
            roleViewMode={roleViewMode}
            setRoleViewMode={setRoleViewMode}
            roleEditor={roleEditor}
            setRoleEditor={setRoleEditor}
          />
        ) : null}
        {activeView === 'queue' ? (
          <QueueView
            tasks={state.tasks}
            cli={cli}
            hostPlatform={hostPlatform}
            queueStats={queueStats}
            settings={state.settings}
            assetById={assetById}
            state={state}
            submitTask={submitTask}
            queryTask={queryTask}
            pendingTaskOps={pendingTaskOps}
            pendingExecutionOps={pendingExecutionOps}
            queryExecutionRecord={queryExecutionRecord}
            processQueueOnce={processQueueOnce}
            selectedTaskId={selectedTaskId}
            setSelectedTaskId={setSelectedTaskId}
            pauseTask={pauseTask}
            resumeTask={resumeTask}
            rescheduleTask={rescheduleTask}
            deleteTask={deleteTask}
            setActiveView={setActiveView}
            setTaskForm={setTaskForm}
            setEditingTaskId={setEditingTaskId}
            askConfirm={askConfirm}
            refreshState={refreshState}
            setFeedback={setFeedback}
            lastTickAt={lastTickAt}
          />
        ) : null}
        {activeView === 'logs' ? <LogsView logs={state.logs} clearLogs={clearLogs} /> : null}
        {activeView === 'settings' ? (
          <SettingsView
            cli={cli}
            settingsForm={settingsForm}
            setSettingsForm={setSettingsForm}
            checkCli={checkCli}
            saveSettings={saveSettings}
            installCli={installCli}
            installCliStatus={installCliStatus}
            loginCli={loginCli}
            loginCliStatus={loginCliStatus}
          />
        ) : null}
        <ConfirmDialog
          modal={confirmModal}
          onCancel={() => setConfirmModal(null)}
          onConfirm={runConfirmedAction}
        />
        {creditModalOpen ? (
          <CreditModal credit={creditInfo} onClose={() => setCreditModalOpen(false)} onRefresh={refreshCredit} />
        ) : null}
      </section>
    </main>
  );
}

function Dashboard({ state, cli, hostPlatform, queueStats, submitTask, queryTask, processQueueOnce, selectedTaskId, setSelectedTaskId, setActiveView }) {
  const selectedTask = state.tasks.find((task) => task.id === selectedTaskId) || state.tasks[0] || null;
  return (
    <div className="dashboard-view">
      <section className="metric-grid">
        <Metric title="平台" value={hostPlatform?.label || 'Desktop'} />
        <Metric title="CLI 检测" value={cli.available ? '检测正常' : '待处理'} tone={cli.available ? 'good' : 'warn'} />
        <Metric title="并发数" value="1" sub="固定并发，顺序执行" />
        <Metric title="默认模型" value="seedance2.0" />
        <div className="notice-card">
          <div>
            <strong>执行策略（并发上限检测）</strong>
            <p>检测到并发上限错误时，自动静默重试；任务状态全程可追溯。</p>
          </div>
          <ShieldCheck size={34} />
        </div>
      </section>

      <section className="dashboard-main">
        <div className="panel queue-table-panel">
          <PanelHeading
            title={`任务中心（并发数：1）`}
            action={(
              <div className="button-cluster">
                <button onClick={processQueueOnce}>运行一次</button>
                <button onClick={() => setActiveView('create')}>新建任务</button>
              </div>
            )}
          />
          <table className="queue-table">
            <thead>
              <tr>
                <th>任务名</th>
                <th>状态</th>
                <th>模型</th>
                <th>宽高比</th>
                <th>submit_id</th>
                <th>重试</th>
              </tr>
            </thead>
            <tbody>
              {state.tasks.map((task, index) => (
                <tr
                  key={task.id}
                  className={selectedTask?.id === task.id ? 'selected-row' : ''}
                  onClick={() => setSelectedTaskId(task.id)}
                >
                  <td>{index + 1}. {task.title || '未命名任务'}</td>
                  <td><StatusBadge status={task.status} task={task} /></td>
                  <td>{task.params?.model_version || 'seedance2.0'}</td>
                  <td>{task.params?.ratio || '9:16'}</td>
                  <td>{task.submit_id || '-'}</td>
                  <td>{task.concurrency_retry_count || 0}/8</td>
                </tr>
              ))}
              {!state.tasks.length ? (
                <tr><td colSpan="6" className="empty-cell">暂无任务，先创建一个 multimodal2video 任务。</td></tr>
              ) : null}
            </tbody>
          </table>
          <footer>共 {state.tasks.length} 条 · 等待 {queueStats.waiting} · 运行 {queueStats.running} · 成功 {queueStats.done}</footer>
        </div>

        <TaskDetail task={selectedTask} submitTask={submitTask} queryTask={queryTask} />
      </section>
    </div>
  );
}

function CreateTaskView({ state, assetById, taskForm, setTaskForm, setActiveView, saveTaskDraft, editingTaskId, cli, addTempImage, removeTempImage, previewCommand, generateTaskTitle, pasteClipboardImage, pasteSystemClipboardImage }) {
  const [previewSrc, setPreviewSrc] = useState(null);
  const [previewAlt, setPreviewAlt] = useState('');
  const [audioPreviewAsset, setAudioPreviewAsset] = useState(null);
  const [aiModal, setAiModal] = useState({ open: false, label: '', description: '', error: '' });
  const openImagePreview = (path, alt) => {
    if (!path) return;
    setPreviewSrc(convertFileSrc(path));
    setPreviewAlt(alt || '');
  };

  const boundResources = useMemo(() => getTaskHitResources({
    image_asset_ids: taskForm.image_asset_ids || [],
    temp_image_asset_ids: taskForm.temp_image_asset_ids || [],
    audio_asset_ids: taskForm.audio_asset_ids || [],
  }, assetById), [
    assetById,
    taskForm.image_asset_ids,
    taskForm.temp_image_asset_ids,
    taskForm.audio_asset_ids,
  ]);
  const boundRoleImages = boundResources.filter((item) => item.displayType === 'role_image');
  const boundTempImages = boundResources.filter((item) => item.displayType === 'temp_image');
  const boundAudios = boundResources.filter((item) => item.displayType === 'role_audio');

  const mentionItems = useMemo(() => {
    return buildMentionItems({
      roles: state.roles,
      assetById,
      tempImagePaths: taskForm.temp_image_paths,
      tempImageAssetIds: taskForm.temp_image_asset_ids,
    });
  }, [state.roles, assetById, taskForm.temp_image_paths, taskForm.temp_image_asset_ids]);

  useEffect(() => {
    if (!String(taskForm.prompt || '').trim()) return;
    if (String(taskForm.prompt || '').includes('@') && !mentionItems.length) return;
    setTaskForm((current) => {
      const next = applyPromptMentionsToTaskForm(current, mentionItems);
      if (
        sameStringArray(current.role_ids, next.role_ids)
        && sameStringArray(current.manual_mention_ids, next.manual_mention_ids)
        && sameStringArray(current.image_asset_ids, next.image_asset_ids)
        && sameStringArray(current.audio_asset_ids, next.audio_asset_ids)
      ) {
        return current;
      }
      return next;
    });
  }, [mentionItems, setTaskForm, taskForm.prompt]);

  // Single atomic update: merges plain text + mention-derived bindings in one setTaskForm call.
  // Preserves role_ids added via role picker by only overwriting the mention-derived subset.
  const handleEditorUpdate = useCallback((plainText, refs) => {
    setTaskForm((current) => {
      const nonMentionRoleIds = (current.role_ids || []).filter(
        (id) => !(current.manual_mention_ids || []).includes(id)
      );
      return {
        ...current,
        prompt: plainText,
        role_ids: uniqueValues([...nonMentionRoleIds, ...refs.roleIds]),
        manual_mention_ids: refs.roleIds,
        image_asset_ids: uniqueValues([...refs.imageAssetIds, ...(current.temp_image_asset_ids || [])]),
        audio_asset_ids: refs.audioAssetIds,
      };
    });
  }, []);

  // Paste wrappers that atomically add temp image state before returning the asset.
  const handlePasteImageForEditor = useCallback(async (file) => {
    const asset = await pasteClipboardImage(file);
    setTaskForm((current) => ({
      ...current,
      temp_image_paths: [...(current.temp_image_paths || []), asset.stored_path].slice(0, 9),
      temp_image_asset_ids: [...(current.temp_image_asset_ids || []), asset.id].slice(0, 9),
    }));
    return asset;
  }, [pasteClipboardImage]);

  const handlePasteSystemImageForEditor = useCallback(async () => {
    const asset = await pasteSystemClipboardImage();
    setTaskForm((current) => ({
      ...current,
      temp_image_paths: [...(current.temp_image_paths || []), asset.stored_path].slice(0, 9),
      temp_image_asset_ids: [...(current.temp_image_asset_ids || []), asset.id].slice(0, 9),
    }));
    return asset;
  }, [pasteSystemClipboardImage]);

  const canSaveDraft = canSaveTaskDraft(taskForm);
  const canApplyPreset = canApplyCreateTaskPreset(taskForm);
  const isEditingTask = Boolean(editingTaskId);
  const pageHeader = buildSecondaryPageHeaderConfig('task', { mode: isEditingTask ? 'edit' : 'create', name: taskForm.title });

  const applyPresetTemplate = useCallback(() => {
    setTaskForm((current) => applyCreateTaskPreset(current, mentionItems));
  }, [mentionItems, setTaskForm]);
  const regenerateTitle = useCallback(async () => {
    const desc = taskForm.prompt.trim().slice(0, 50) + (taskForm.prompt.length > 50 ? '…' : '');
    setAiModal({ open: true, label: '生成任务标题', description: desc, error: '' });
    try {
      const title = await generateTaskTitle?.(taskForm.prompt);
      if (title) {
        setTaskForm((current) => ({ ...current, title }));
        setAiModal({ open: false, label: '', description: '', error: '' });
      } else {
        setAiModal((m) => ({ ...m, open: true, error: 'AI 未返回标题，请检查模型配置或 API Key' }));
        setTimeout(() => setAiModal({ open: false, label: '', description: '', error: '' }), 6000);
      }
    } catch (err) {
      const msg = String(err).replace(/^Error:\s*/i, '');
      setAiModal((m) => ({ ...m, open: true, error: msg }));
      setTimeout(() => setAiModal({ open: false, label: '', description: '', error: '' }), 6000);
    }
  }, [generateTaskTitle, setTaskForm, taskForm.prompt]);

  return (
    <div className="create-page">
      <div className="create-page-main">
        <SecondaryPageHeader
          title={pageHeader.title}
          backLabel={pageHeader.backLabel}
          onBack={() => setActiveView('queue')}
          actions={(
            <>
              <button type="button" className="outline-button" onClick={previewCommand}>预览命令</button>
              {isEditingTask ? (
                <button type="button" className="outline-button" onClick={regenerateTitle} disabled={!taskForm.prompt.trim()}>
                  <Sparkles size={14} /> 重生成标题
                </button>
              ) : null}
              <button type="button" className="gradient-button" disabled={!canSaveDraft} onClick={saveTaskDraft}>
                <Plus size={14} /> {isEditingTask ? '保存修改' : '保存任务'}
              </button>
            </>
          )}
        />
        <div className="create-page-body">
              {/* 左侧表单 */}
          <div className="create-form-panel">
            <form onSubmit={saveTaskDraft} className="create-form-stack">
              <div className="create-field-row create-model-row">
                {/* 模型 */}
                <div className="create-field">
                  <label><span>主模型</span><b className="required">*</b></label>
                  <select value={taskForm.params.model_version} onChange={(e) => updateTaskParams(setTaskForm, { model_version: e.target.value })}>
                    {modelVersions.map((m) => <option key={m} value={m}>{m}</option>)}
                  </select>
                </div>

                {/* 宽高比 */}
                <div className="create-field">
                  <label><span>宽高比</span><b className="required">*</b></label>
                  <select value={taskForm.params.ratio} onChange={(e) => updateTaskParams(setTaskForm, { ratio: e.target.value })}>
                    {ratios.map((r) => <option key={r} value={r}>{r}</option>)}
                  </select>
                </div>

                <div className="create-field create-duration-field">
                  <label><span>时长</span><b className="required">*</b></label>
                  <select
                    value={taskForm.params.duration}
                    onChange={(e) => updateTaskParams(setTaskForm, { duration: Number(e.target.value) })}
                  >
                    {durationOptions.map((seconds) => (
                      <option key={seconds} value={seconds}>{seconds} 秒</option>
                    ))}
                  </select>
                </div>
              </div>

              {/* 提示词 */}
              <div className="create-field">
                <label><span>提示词</span><b className="required">*</b></label>
                {canApplyPreset ? (
                  <button type="button" className="preset-template-card" onClick={applyPresetTemplate}>
                    <Sparkles size={15} />
                    <span>一键添加预设模板</span>
                    <em>自动匹配可用的 @分镜图、角色图片和音频素材</em>
                  </button>
                ) : null}
                <PromptMentionEditor
                  value={taskForm.prompt}
                  mentionItems={mentionItems}
                  maxLength={TASK_PROMPT_MAX_LENGTH}
                  placeholder="@女主日常服 在海边漫步，阳光照在身上，海浪轻轻打沙滩，微风拂动长发，画面唯美治愈。"
                  onUpdate={handleEditorUpdate}
                  onPasteImage={handlePasteImageForEditor}
                  onPasteSystemImage={handlePasteSystemImageForEditor}
                  tempImagePaths={taskForm.temp_image_paths}
                />
                <div className="info-strip">
                  <Sparkles size={13} />
                  输入 @ 可引用具体图片（如 @女主厨师服）、音频或临时图片。
                </div>
              </div>

              <div className="schedule-hint-card">
                <CalendarClock size={16} />
                <div>
                  <strong>默认不定时</strong>
                  <span>保存后进入任务中心，可单选或多选任务后再指定开始时间、立即提交或批量排布。</span>
                </div>
              </div>

              {/* 临时图片/分镜图 */}
              <div className="create-field">
                <label><span>临时图片（分镜图）</span></label>
                <div className="temp-image-grid">
                  <button type="button" className="temp-upload-card" onClick={addTempImage}>
                    <Upload size={18} />
                    <span>上传临时图片 / 分镜图</span>
                    <em>支持 PNG/JPG/WebP，单张 ≤ 10MB</em>
                  </button>
                  {taskForm.temp_image_paths.map((path, index) => (
                    <div key={path} className="temp-image-card">
                      <Thumb asset={{ kind: 'image', path }} label={fileExt(path)} onClick={() => openImagePreview(path, path.split('/').pop())} />
                      <div className="temp-image-info">
                        <span className="temp-image-name">{path.split('/').pop()}</span>
                      </div>
                      <button type="button" className="temp-image-remove" onClick={() => removeTempImage(index)}>×</button>
                      <button type="button" className="temp-image-preview" title="预览" onClick={() => openImagePreview(path, path.split('/').pop())}><ZoomIn size={12} /></button>
                    </div>
                  ))}
                </div>
                <div className="info-strip">
                  <Image size={13} />
                  可在提示词中通过 @分镜图01 等方式引用临时图片。
                </div>
              </div>
            </form>
          </div>

          {/* 右侧已绑定素材 */}
          <aside className="create-resource-panel">
            <div className="create-resource-head">
              <h3>已绑定素材</h3>
            </div>

            {/* 角色图片 */}
            <div className="create-resource-section">
              <div className="create-section-head">
                <h4>角色图片（{boundRoleImages.length}）</h4>
                <button type="button" className="section-manage-btn">管理</button>
              </div>
              {boundRoleImages.length ? (
                <div className="resource-thumb-row">
                  {boundRoleImages.map(({ asset }) => (
                    <Thumb
                      key={asset.id}
                      asset={asset}
                      label={asset.name}
                      onClick={() => openImagePreview(asset.stored_path, asset.name)}
                    />
                  ))}
                </div>
              ) : (
                <p className="resource-empty">暂无 @ 角色图片</p>
              )}
            </div>

            {/* 音频素材 */}
            <div className="create-resource-section">
              <div className="create-section-head">
                <h4>音频素材（{boundAudios.length}）</h4>
                <button type="button" className="section-manage-btn">管理</button>
              </div>
              {boundAudios.length ? (
                <div className="resource-audio-list">
                  {boundAudios.map(({ asset }) => (
                    <AudioPreviewRow key={asset.id} asset={asset} onClick={() => setAudioPreviewAsset(asset)} />
                  ))}
                </div>
              ) : (
                <p className="resource-empty">暂无 @ 音频素材</p>
              )}
            </div>

            {/* 临时参考图 */}
            <div className="create-resource-section">
              <div className="create-section-head">
                <h4>临时参考图（{boundTempImages.length}）</h4>
              </div>
              {boundTempImages.length ? (
                <div className="resource-thumb-row">
                  {boundTempImages.map(({ asset }) => (
                    <Thumb
                      key={asset.id}
                      asset={asset}
                      label={asset.name}
                      onClick={() => openImagePreview(asset.stored_path, asset.name)}
                    />
                  ))}
                </div>
              ) : (
                <p className="resource-empty">暂无临时参考图</p>
              )}
            </div>
          </aside>
        </div>
        <ImageModal src={previewSrc} alt={previewAlt} onClose={() => setPreviewSrc(null)} />
        <AudioAssetModal asset={audioPreviewAsset} onClose={() => setAudioPreviewAsset(null)} />
        <AiThinkingModal open={aiModal.open} label={aiModal.label} description={aiModal.description} error={aiModal.error} />
      </div>
    </div>
  );
}

function RolesView({
  state,
  assetById,
  roleForm,
  setRoleForm,
  createRole,
  chooseInitialRoleFile,
  chooseFilesForSelectedRole,
  askConfirm,
  deleteRole,
  removeRoleMedia,
  renameAsset,
  dragActive,
  selectedRole,
  selectedRoleMedia,
  selectedRoleId,
  setSelectedRoleId,
  roleSearchQuery,
  setRoleSearchQuery,
  roleActiveTab,
  setRoleActiveTab,
  roleViewMode,
  setRoleViewMode,
  roleEditor,
  setRoleEditor,
}) {
  const filteredRoles = useMemo(() => {
    let list = state.roles;
    if (roleSearchQuery.trim()) {
      const q = roleSearchQuery.toLowerCase();
      list = list.filter((r) =>
        r.name?.toLowerCase().includes(q) ||
        r.tags?.some((t) => t.toLowerCase().includes(q))
      );
    }
    if (roleActiveTab === 'images') {
      list = list.filter((r) => getRoleMedia(r, assetById).images.length > 0);
    } else if (roleActiveTab === 'audios') {
      list = list.filter((r) => getRoleMedia(r, assetById).audios.length > 0);
    }
    return list;
  }, [state.roles, roleSearchQuery, roleActiveTab, assetById]);

  const handleNewRole = () => {
    setSelectedRoleId('');
    setRoleEditor(createRoleEditor('create'));
  };

  const handleEditRole = () => {
    if (!selectedRole) return;
    setRoleEditor(createRoleEditor('edit', selectedRole));
  };

  const handleSaveEdit = async (event) => {
    event.preventDefault();
    await createRole(event, roleEditor?.form);
  };

  if (roleEditor) {
    const editingRole = roleEditor.roleId
      ? state.roles.find((role) => role.id === roleEditor.roleId) || null
      : null;
    const editingRoleMedia = getRoleMedia(editingRole, assetById);
    return (
      <RoleEditPage
        mode={roleEditor.mode}
        roleForm={roleEditor.form}
        setRoleForm={setRoleForm}
        selectedRole={editingRole}
        selectedRoleMedia={editingRoleMedia}
        onSave={handleSaveEdit}
        onCancel={() => setRoleEditor(null)}
        chooseInitialRoleFile={chooseInitialRoleFile}
        chooseFilesForSelectedRole={chooseFilesForSelectedRole}
        removeRoleMedia={(assetId) => removeRoleMedia(assetId, roleEditor.roleId)}
        renameAsset={renameAsset}
        askConfirm={askConfirm}
        deleteRole={deleteRole}
      />
    );
  }

  return (
    <div className="role-page">
      <div className="role-page-main">
        <div className="role-page-toolbar">
          <div className="role-toolbar-title">
            <h2>角色库</h2>
            <p>管理您的角色及其图片与音频资源</p>
          </div>
          <button type="button" className="gradient-button role-new-btn" onClick={handleNewRole}>
            <Plus size={15} /> 新建角色 <ChevronDown size={13} />
          </button>
        </div>
        <div className="role-page-filter">
          <div className="role-search-box">
            <Search size={15} />
            <input
              value={roleSearchQuery}
              onChange={(e) => setRoleSearchQuery(e.target.value)}
              placeholder="搜索角色名称或标签"
            />
          </div>
          <div className="role-tabs">
            <button type="button" className={roleActiveTab === 'all' ? 'active' : ''} onClick={() => setRoleActiveTab('all')}>全部角色</button>
            <button type="button" className={roleActiveTab === 'images' ? 'active' : ''} onClick={() => setRoleActiveTab('images')}>图片资源</button>
            <button type="button" className={roleActiveTab === 'audios' ? 'active' : ''} onClick={() => setRoleActiveTab('audios')}>音频资源</button>
          </div>
          <div className="role-view-toggle">
            <button type="button" className={roleViewMode === 'grid' ? 'active' : ''} onClick={() => setRoleViewMode('grid')}><Grid size={15} /></button>
            <button type="button" className={roleViewMode === 'list' ? 'active' : ''} onClick={() => setRoleViewMode('list')}><LayoutList size={15} /></button>
          </div>
        </div>
        <div className={`role-card-grid ${roleViewMode === 'list' ? 'list-mode' : ''}`}>
          {filteredRoles.map((role) => {
            const media = getRoleMedia(role, assetById);
            const isSelected = role.id === selectedRoleId;
            return (
              <button
                key={role.id}
                type="button"
                className={`role-card ${isSelected ? 'selected' : ''}`}
                onClick={() => setSelectedRoleId(role.id)}
              >
                <div className="role-card-avatar">
                  <Thumb asset={media.images[0]} label={role.name} />
                </div>
                <div className="role-card-body">
                  <div className="role-card-head">
                    <strong>{role.name}</strong>
                    {isSelected ? <span className="role-default-badge">默认</span> : null}
                    <button type="button" className="role-card-more" onClick={(e) => { e.stopPropagation(); }}>
                      <MoreHorizontal size={15} />
                    </button>
                  </div>
                  <div className="role-card-tags">
                    {(role.tags || []).slice(0, 3).map((tag) => (
                      <span key={tag} className="role-tag-chip">{tag}</span>
                    ))}
                  </div>
                  <p className="role-card-desc">{role.description || '暂无描述'}</p>
                  <div className="role-card-stats">
                    <span><Image size={12} /> 图片 <b>{media.images.length}</b></span>
                    <span><FileAudio size={12} /> 音频 <b>{media.audios.length}</b></span>
                    <span><Star size={12} /> 音色 <b>{media.audios.length ? 1 : 0}</b></span>
                  </div>
                </div>
              </button>
            );
          })}
          {!filteredRoles.length ? (
            <div className="role-empty-state">
              <User size={32} />
              <p>还没有角色，点击「新建角色」创建第一个角色。</p>
            </div>
          ) : null}
        </div>
      </div>

      <aside className="role-detail-panel">
        {selectedRole ? (
          <div className="role-detail-inner">
            <div className="role-detail-header">
              <div className="role-detail-title-row">
                <h3>{selectedRole.name}</h3>
                <span className="role-default-badge">默认</span>
                <button type="button" className="icon-ghost" title="编辑" onClick={handleEditRole}><Pencil size={14} /></button>
                <button type="button" className="icon-ghost" title="关闭" onClick={() => setSelectedRoleId('')}><X size={14} /></button>
              </div>
              <div className="role-detail-avatar">
                <Thumb asset={selectedRoleMedia.images[0]} label={selectedRole.name} />
              </div>
              <div className="role-detail-tags">
                {(selectedRole.tags || []).map((tag) => (
                  <span key={tag} className="role-tag-chip">{tag}</span>
                ))}
              </div>
              <p className="role-detail-desc">{selectedRole.description || '暂无角色描述。'}</p>
              <div className="role-detail-meta">
                <span>创建时间：{formatDate(selectedRole.created_at) || '-'}</span>
                <span className="role-id-row">角色 ID：{shortRoleId(selectedRole.id)} <button type="button" className="icon-ghost mini" title="复制"><Copy size={11} /></button></span>
              </div>
            </div>

            <div className="role-detail-divider" />

            <RoleDetailImageSection
              images={selectedRoleMedia.images}
              chooseFilesForSelectedRole={chooseFilesForSelectedRole}
              removeRoleMedia={(assetId) => removeRoleMedia(assetId, selectedRole.id)}
              renameAsset={renameAsset}
              askConfirm={askConfirm}
            />
            <RoleDetailAudioSection
              audios={selectedRoleMedia.audios}
              removeRoleMedia={(assetId) => removeRoleMedia(assetId, selectedRole.id)}
              askConfirm={askConfirm}
            />
            <RoleDetailVoiceSection
              item={selectedRoleMedia.audios[0]}
              onManageResources={handleEditRole}
              removeRoleMedia={(assetId) => removeRoleMedia(assetId, selectedRole.id)}
              askConfirm={askConfirm}
            />

            <div className="role-detail-actions">
              <button type="button" className="outline-button" onClick={handleEditRole}>
                <Pencil size={14} /> 编辑角色信息
              </button>
              <button type="button" className="gradient-button" onClick={handleEditRole}>
                <ImagePlus size={14} /> 管理资源
              </button>
            </div>
          </div>
        ) : (
          <div className="role-detail-empty">
            <User size={28} />
            <p>选择角色查看详情</p>
          </div>
        )}
      </aside>
    </div>
  );
}

// ── QueueView helper components ─────────────────────────────────────────────

function StatCard2({ label, count, icon: Icon, color, sub, spin }) {
  return (
    <div className={`qc-stat-card ${color}`}>
      <div className="qc-stat-icon"><Icon size={16} className={spin ? 'spin' : ''} /></div>
      <div className="qc-stat-body">
        <span className="qc-stat-count">{count}</span>
        <span className="qc-stat-label">{label}</span>
        {sub ? <span className="qc-stat-sub">{sub}</span> : null}
      </div>
    </div>
  );
}

const TaskCard = React.memo(function TaskCard({ task, index, selected, selectedForBatch, assetById, roles, onSelect, onToggleSelection }) {
  const thumbPath = useMemo(() => {
    for (const id of (task.image_asset_ids || [])) {
      const a = assetById.get(id);
      if (a?.stored_path) return a.stored_path;
    }
    const role = roles.find((r) => r.id === (task.role_ids || [])[0]);
    if (role?.asset_ids?.length) {
      const a = assetById.get(role.asset_ids[0]);
      if (a?.stored_path) return a.stored_path;
    }
    return null;
  }, [task, assetById, roles]);

  const handleClick = useCallback(() => onSelect(task.id), [onSelect, task.id]);

  return (
    <div className={`qc-task-row${selected ? ' selected' : ''}`} onClick={handleClick} role="button" tabIndex={0}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleClick(); }}>
      <button
        type="button"
        className={`qc-task-check${selectedForBatch ? ' checked' : ''}`}
        title={selectedForBatch ? '取消选择' : '选择任务'}
        disabled={!canScheduleTask(task)}
        onClick={(event) => {
          event.stopPropagation();
          onToggleSelection?.(task);
        }}
      >
        {selectedForBatch ? <CheckCircle2 size={11} /> : null}
      </button>
      <span className="qc-task-index">{index}</span>
      <div className="qc-task-thumb">
        {thumbPath ? (
          <img src={convertFileSrc(thumbPath)} alt="" className="qc-thumb-img" />
        ) : (
          <div className="qc-thumb-placeholder"><Image size={14} /></div>
        )}
      </div>
      <div className="qc-task-info">
        <span className="qc-task-title">{task.title || '未命名任务'}</span>
        <span className="qc-task-sub">{task.params?.model_version || ''}{task.params?.ratio ? ` · ${task.params.ratio}` : ''}</span>
      </div>
      <div className="qc-task-right">
        <StatusBadge status={task.status} />
        <span className={`qc-task-time${task.status === 'scheduled' ? ' scheduled' : ''}`}>
          {task.status === 'scheduled' && task.scheduled_at
            ? formatDatePart(task.scheduled_at, 'time')
            : formatDatePart(task.updated_at, 'time')}
        </span>
      </div>
    </div>
  );
});

function RingProgress({ percent, status }) {
  const r = 30;
  const cx = 38;
  const cy = 38;
  const circ = 2 * Math.PI * r;
  const offset = circ - (Math.min(100, Math.max(0, percent)) / 100) * circ;
  const isFailed = ['failed'].includes(status);
  const isRunning = ['submitting', 'querying'].includes(status);
  const stroke = isFailed ? '#f04444' : percent === 100 ? '#22c55e' : isRunning ? '#7168ff' : '#c7cbff';
  return (
    <svg width="76" height="76" className="qc-ring">
      <circle cx={cx} cy={cy} r={r} fill="none" stroke="#edf1f8" strokeWidth="5" />
      <circle cx={cx} cy={cy} r={r} fill="none" stroke={stroke} strokeWidth="5" strokeLinecap="round"
        strokeDasharray={circ} strokeDashoffset={offset}
        transform={`rotate(-90 ${cx} ${cy})`} style={{ transition: 'stroke-dashoffset 0.4s ease' }} />
      <text x={cx} y={cy + 5} textAnchor="middle" fontSize="12" fontWeight="700" fill="#1f2944">
        {isFailed ? '✗' : `${percent}%`}
      </text>
    </svg>
  );
}

// ── QueueView ────────────────────────────────────────────────────────────────

function QueueView({
  tasks,
  settings,
  assetById,
  state,
  submitTask,
  queryTask,
  pendingTaskOps = {},
  pendingExecutionOps = {},
  queryExecutionRecord = async () => {},
  processQueueOnce,
  selectedTaskId,
  setSelectedTaskId,
  pauseTask,
  resumeTask,
  rescheduleTask,
  deleteTask,
  setActiveView,
  setTaskForm,
  setEditingTaskId,
  askConfirm,
  refreshState,
  setFeedback,
  lastTickAt,
}) {
  // ── local state ──────────────────────────────────────────────────────────
  const [searchQuery, setSearchQuery] = useState('');
  const [statusTab, setStatusTab] = useState('all');
  const [modelFilter, setModelFilter] = useState('all');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(8);
  const [resourcePreview, setResourcePreview] = useState(null);
  const [selectedBatchIds, setSelectedBatchIds] = useState([]);
  const [selectedExecutionId, setSelectedExecutionId] = useState(null);
  const [scheduleModal, setScheduleModal] = useState(null);
  const [commandPreviewModal, setCommandPreviewModal] = useState(null);

  // ── derived data ─────────────────────────────────────────────────────────
  const stats = useMemo(() => deriveQueueStats(tasks), [tasks]);
  const modelOptions = useMemo(() => getModelOptions(tasks), [tasks]);
  const filteredSorted = useMemo(
    () => sortTasks(filterTasks(tasks, { searchQuery, statusTab, modelFilter })),
    [tasks, searchQuery, statusTab, modelFilter]
  );
  useEffect(() => { setPage(1); }, [searchQuery, statusTab, modelFilter]);
  const paged = useMemo(() => paginateTasks(filteredSorted, page, pageSize), [filteredSorted, page, pageSize]);

  const selectedTask = useMemo(
    () => tasks.find((t) => t.id === selectedTaskId) || null,
    [tasks, selectedTaskId]
  );

  useEffect(() => {
    if (!selectedTaskId && tasks.length) setSelectedTaskId(tasks[0].id);
  }, [tasks]);

  const [showAttemptsModal, setShowAttemptsModal] = useState(false);
  const taskHistory = useMemo(() => deriveTaskHistory(selectedTask), [selectedTask]);
  const currentExecution = useMemo(
    () => deriveCurrentExecutionRecord(selectedTask, selectedExecutionId),
    [selectedTask, selectedExecutionId]
  );
  const executionView = useMemo(() => {
    if (!selectedTask || !currentExecution) return selectedTask;
    const isCurrentTaskSubmit = currentExecution.submit_id && currentExecution.submit_id === selectedTask.submit_id;
    const snapshot = currentExecution.input_snapshot || {};
    return {
      ...selectedTask,
      status: currentExecution.status || selectedTask.status,
      submit_id: currentExecution.submit_id || selectedTask.submit_id,
      command_preview: currentExecution.command_preview?.length ? currentExecution.command_preview : selectedTask.command_preview,
      result_paths: currentExecution.result_paths || [],
      result_urls: currentExecution.result_urls || [],
      attempts: currentExecution.query_records || currentExecution.attempts || [],
      last_error: currentExecution.error_detail || (isCurrentTaskSubmit ? selectedTask.last_error : ''),
      queue_info: isCurrentTaskSubmit ? selectedTask.queue_info : null,
      image_asset_ids: snapshot.image_asset_ids || selectedTask.image_asset_ids,
      audio_asset_ids: snapshot.audio_asset_ids || selectedTask.audio_asset_ids,
      role_ids: snapshot.role_ids || selectedTask.role_ids,
      manual_mention_ids: snapshot.manual_mention_ids || selectedTask.manual_mention_ids,
      temp_image_asset_ids: snapshot.temp_image_asset_ids || selectedTask.temp_image_asset_ids,
    };
  }, [selectedTask, currentExecution]);
  const flowSteps = useMemo(() => deriveTaskFlowSteps(executionView), [executionView]);
  const progress = useMemo(() => deriveTaskProgress(executionView), [executionView]);
  const allAttempts = useMemo(
    () => deriveCurrentQueryRecords(selectedTask, selectedExecutionId).slice().reverse(),
    [selectedTask, selectedExecutionId]
  );
  const recentAttempts = useMemo(() => allAttempts.slice(0, 4), [allAttempts]);
  const commandText = useMemo(
    () => executionView?.command_preview?.join(' \\\n  ') || '',
    [executionView]
  );
  const commandPresentation = useMemo(
    () => getCommandPreviewPresentation(commandText),
    [commandText]
  );
  const hitResources = useMemo(() => getTaskHitResources(executionView, assetById), [executionView, assetById]);
  const resultItems = useMemo(() => getTaskResultItems(executionView), [executionView]);
  const selectedBatchTasks = useMemo(
    () => selectedBatchIds.map((id) => tasks.find((task) => task.id === id)).filter(Boolean),
    [selectedBatchIds, tasks]
  );
  const schedulablePagedIds = useMemo(
    () => paged.items.filter(canScheduleTask).map((task) => task.id),
    [paged.items]
  );

  useEffect(() => {
    setResourcePreview(null);
    setCommandPreviewModal(null);
    setSelectedExecutionId(null);
  }, [selectedTaskId]);

  useEffect(() => {
    setSelectedBatchIds((ids) => ids.filter((id) => tasks.some((task) => task.id === id && canScheduleTask(task))));
  }, [tasks]);

  // ── handlers ─────────────────────────────────────────────────────────────
  const handleEditTask = (task) => {
    setEditingTaskId(task.id);
    setTaskForm(buildTaskFormFromTaskForEdit(task, assetById));
    setActiveView('create');
  };
  const handleDuplicateTask = (task) => {
    setEditingTaskId(null);
    setTaskForm(buildTaskFormFromTaskForDuplicate(task, assetById));
    setActiveView('create');
    setFeedback(`已复制「${task.title || '未命名任务'}」，保存后会生成新任务`);
  };
  const handleDeleteTask = (task) => {
    askConfirm({
      message: `确认删除任务「${task.title || '未命名任务'}」？`,
      onConfirm: () => deleteTask(task.id),
    });
  };
  const handleDeleteExecutionRecord = (taskId, executionId, label) => {
    askConfirm({
      title: '删除执行记录',
      body: `确认删除「${label}」？本地视频文件不会被删除，仅移除任务中心记录。`,
      confirmText: '删除记录',
      onConfirm: async () => {
        try {
          await invoke('delete_execution_record_command', { taskId, executionId });
          setFeedback('执行记录已删除');
          if (selectedExecutionId === executionId) setSelectedExecutionId(null);
          await refreshState();
        } catch (error) {
          setFeedback(String(error));
        }
      },
    });
  };
  const handleClearDone = () => {
    const doneTasks = tasks.filter((t) => t.status === 'succeeded');
    if (!doneTasks.length) return;
    askConfirm({
      message: `确认清空 ${doneTasks.length} 条已完成任务？此操作不可撤销。`,
      onConfirm: async () => {
        for (const t of doneTasks) await deleteTask(t.id);
      },
    });
  };
  const toggleTaskSelection = useCallback((task) => {
    if (!canScheduleTask(task)) return;
    setSelectedBatchIds((ids) => ids.includes(task.id) ? ids.filter((id) => id !== task.id) : [...ids, task.id]);
  }, []);
  const toggleSelectPage = () => {
    setSelectedBatchIds((ids) => {
      const pageAllSelected = schedulablePagedIds.length && schedulablePagedIds.every((id) => ids.includes(id));
      if (pageAllSelected) return ids.filter((id) => !schedulablePagedIds.includes(id));
      return uniqueValues([...ids, ...schedulablePagedIds]);
    });
  };
  const openPrepareGenerate = (task) => {
    if (!task || !canScheduleTask(task)) {
      setFeedback('当前任务正在执行或查询，暂不可准备生成');
      return;
    }
    setScheduleModal({ mode: 'prepare', taskIds: [task.id], title: `准备生成「${task.title || '未命名任务'}」` });
  };
  const openBatchSchedule = () => {
    const taskIds = selectedBatchTasks.filter(canScheduleTask).map((task) => task.id);
    if (!taskIds.length) {
      setFeedback('请先选择可排期的任务');
      return;
    }
    setScheduleModal({ mode: 'batch', taskIds, title: `批量排布 ${taskIds.length} 个任务` });
  };
  const applySchedulePlan = async ({ scheduledAt, intervalMinutes }) => {
    if (!scheduleModal?.taskIds?.length) return;
    try {
      if (scheduleModal.mode === 'batch') {
        const startAt = scheduledAt || new Date().toISOString();
        const plan = buildBatchSchedulePlan(scheduleModal.taskIds, { startAt, intervalMinutes });
        for (const item of plan) {
          await rescheduleTask(item.taskId, item.scheduledAt);
        }
        setFeedback(`已排布：${formatSchedulePlanSummary(plan)}`);
        setSelectedBatchIds([]);
      } else if (scheduleModal.mode === 'prepare') {
        const operation = resolvePrepareGenerateOperation({ scheduledAt });
        if (operation.type === 'submit') {
          await submitTask(scheduleModal.taskIds[0]);
          setFeedback('已开始生成');
        } else {
          await rescheduleTask(scheduleModal.taskIds[0], operation.scheduledAt);
          setFeedback('已设置定时生成');
        }
      } else {
        await rescheduleTask(scheduleModal.taskIds[0], scheduledAt);
        setFeedback(scheduledAt ? '任务已重新排期' : '任务已设为立即提交');
      }
      setScheduleModal(null);
    } catch (error) {
      setFeedback(String(error));
    }
  };
  const openCommandPreview = () => {
    if (!commandPresentation.hasCommand) return;
    setCommandPreviewModal({
      title: selectedTask?.title || '未命名任务',
      commandText,
    });
  };

  // ── STATUS TAB DEFS ───────────────────────────────────────────────────────
  const STATUS_TABS = [
    { key: 'all', label: '全部', count: tasks.length },
    { key: 'waiting', label: '等待', count: stats.waiting },
    { key: 'running', label: '执行中', count: stats.running },
    { key: 'retry', label: '重试', count: stats.retry },
    { key: 'done', label: '已完成', count: stats.done },
    { key: 'failed', label: '失败', count: stats.failed },
  ];

  const handleNewTask = () => {
    setEditingTaskId(null);
    setTaskForm(createEmptyTaskForm());
    setActiveView('create');
  };

  return (
    <div className="queue-center">
      {/* ── Header ── */}
      <div className="qc-header">
        <div className="qc-header-text">
          <h1 className="qc-title">任务中心</h1>
          <p className="qc-subtitle">统一管理任务保存、单个排期、批量排布、执行状态与结果回看</p>
          <p className="qc-scheduler-hint">
            <Clock3 size={11} /> 本地调度每 30 秒自动检查
            {lastTickAt ? <> · 上次检查 {lastTickAt.toLocaleTimeString()} </> : null}
            {(() => {
              const next = tasks.filter((t) => t.status === 'scheduled' && t.scheduled_at).sort((a, b) => new Date(a.scheduled_at) - new Date(b.scheduled_at))[0];
              return next ? <> · 下次预定 {formatDate(next.scheduled_at)}</> : null;
            })()}
            <span className="qc-sleep-note">电脑睡眠或退出应用时无法提交，恢复后自动补偿</span>
          </p>
        </div>
        <button type="button" className="gradient-button qc-new-task-btn" onClick={handleNewTask}>
          <Plus size={14} /> 新建任务
        </button>
      </div>

      {/* ── Stats ── */}
      <div className="qc-stats">
        <StatCard2 label="等待中" count={stats.waiting} icon={Clock3} color="waiting" />
        <StatCard2 label="执行中" count={stats.running} icon={Loader2} color="running" spin={stats.running > 0} />
        <StatCard2 label="重试中" count={stats.retry} icon={RefreshCcw} color="retry" />
        <StatCard2 label="已完成" count={stats.done} icon={CheckCircle2} color="done" />
        <StatCard2 label="失败" count={stats.failed} icon={AlertCircle} color="failed" />
        <StatCard2 label="固定并发" count={1} icon={Gauge} color="neutral" sub="单通道顺序执行" />
      </div>

      {/* ── Toolbar ── */}
      <div className="qc-toolbar">
        <div className="qc-search-wrap">
          <Search size={13} className="qc-search-icon" />
          <input className="qc-search-input" placeholder="搜索任务名、提示词、submit_id…"
            value={searchQuery} onChange={(e) => setSearchQuery(e.target.value)} />
          {searchQuery ? (
            <button type="button" className="qc-search-clear" onClick={() => setSearchQuery('')}><X size={11} /></button>
          ) : null}
        </div>
        <select className="qc-select" value={modelFilter} onChange={(e) => setModelFilter(e.target.value)}>
          <option value="all">全部模型</option>
          {modelOptions.map((m) => <option key={m} value={m}>{m}</option>)}
        </select>
        <div className="qc-toolbar-sep" />
        <button type="button" className="qc-btn" onClick={refreshState}><RefreshCcw size={13} /> 刷新</button>
        <button type="button" className="qc-btn" onClick={handleClearDone} disabled={stats.done === 0}>
          <Trash2 size={13} /> 清空已完成{stats.done > 0 ? `（${stats.done}）` : ''}
        </button>
        <button type="button" className="qc-btn" onClick={toggleSelectPage} disabled={!schedulablePagedIds.length}>
          <CheckCircle2 size={13} /> {schedulablePagedIds.length && schedulablePagedIds.every((id) => selectedBatchIds.includes(id)) ? '取消本页' : '选择本页'}
        </button>
        <button type="button" className="qc-btn" onClick={openBatchSchedule} disabled={!selectedBatchTasks.length}>
          <CalendarClock size={13} /> 批量排布{selectedBatchTasks.length ? `（${selectedBatchTasks.length}）` : ''}
        </button>
        <button type="button" className="qc-btn" onClick={() => setActiveView('settings')}><Settings size={13} /> 执行策略</button>
        <div className="qc-toolbar-end">
          <button type="button" className="qc-btn qc-btn-primary" onClick={processQueueOnce}><Play size={13} /> 运行一次</button>
        </div>
      </div>

      {/* ── Three-column body ── */}
      <div className="qc-body">
        {/* ── LEFT: task list ── */}
        <div className="qc-task-list">
          <div className="qc-tabs">
            {STATUS_TABS.map((tab) => (
              <button key={tab.key} type="button"
                className={`qc-tab${statusTab === tab.key ? ' active' : ''}`}
                onClick={() => setStatusTab(tab.key)}>
                {tab.label}
                {tab.count > 0 ? <span className="qc-tab-badge">{tab.count}</span> : null}
              </button>
            ))}
          </div>
          <div className="qc-task-rows">
            {paged.items.length ? paged.items.map((task, idx) => (
              <TaskCard key={task.id} task={task} index={paged.startIndex + idx + 1}
                selected={task.id === selectedTaskId}
                selectedForBatch={selectedBatchIds.includes(task.id)}
                assetById={assetById} roles={state.roles}
                onSelect={setSelectedTaskId}
                onToggleSelection={toggleTaskSelection} />
            )) : (
              <div className="qc-empty">
                <ClipboardList size={22} />
                <p>{searchQuery || statusTab !== 'all' || modelFilter !== 'all'
                  ? '无匹配任务' : '暂无任务，先创建一个'}</p>
              </div>
            )}
          </div>
          <div className="qc-pagination">
            <span className="qc-page-label">
              {formatPaginationLabel(paged.startIndex, paged.endIndex, paged.total)}
            </span>
            <div className="qc-page-controls">
              <button type="button" disabled={page <= 1} onClick={() => setPage((p) => p - 1)}>
                <ChevronLeft size={12} />
              </button>
              <span className="qc-page-num">{paged.page} / {paged.totalPages}</span>
              <button type="button" disabled={page >= paged.totalPages} onClick={() => setPage((p) => p + 1)}>
                <ChevronRight size={12} />
              </button>
            </div>
            <select className="qc-select qc-page-size" value={pageSize}
              onChange={(e) => { setPageSize(Number(e.target.value)); setPage(1); }}>
              <option value={8}>8 条/页</option>
              <option value={12}>12 条/页</option>
              <option value={20}>20 条/页</option>
            </select>
          </div>
        </div>

        {/* ── MIDDLE: task detail ── */}
        <div className="qc-detail">
          {selectedTask ? (
            <>
              <div className="qc-detail-body">
              {/* Flow steps */}
              <section className="qc-section">
                <h4 className="qc-section-title">本次执行流转</h4>
                <div className="qc-flow-steps">
                  {flowSteps.map((step, i) => (
                    <React.Fragment key={step.key}>
                      <div className={`qc-flow-step ${step.state}`}>
                        <div className="qc-flow-dot">
                          {step.state === 'done' ? <CheckCircle2 size={13} />
                            : step.state === 'active' && step.spinning ? <Loader2 size={13} className="spin" />
                            : step.state === 'active' ? <span className="qc-flow-active-dot" />
                            : step.state === 'error' ? <AlertCircle size={13} />
                            : <span className="qc-flow-hollow" />}
                        </div>
                        <span className="qc-flow-label">{step.label}</span>
                      </div>
                      {i < flowSteps.length - 1 ? (
                        <div className={`qc-flow-line${i < flowSteps.findIndex((s) => s.state !== 'done') - 1 ? ' filled' : ''}`} />
                      ) : null}
                    </React.Fragment>
                  ))}
                </div>
              </section>

              {/* Meta */}
              <section className="qc-section">
                <h4 className="qc-section-title">任务信息</h4>
                <div className="qc-meta-grid">
                  <span className="qc-meta-label">任务名</span>
                  <span className="qc-meta-value">{selectedTask.title || '未命名任务'}</span>
                  <span className="qc-meta-label">状态</span>
                  <span className="qc-meta-value"><StatusBadge status={executionView?.status || selectedTask.status} task={executionView || selectedTask} /></span>
                  <span className="qc-meta-label">模型</span>
                  <span className="qc-meta-value">{selectedTask.params?.model_version || '-'}</span>
                  <span className="qc-meta-label">宽高比</span>
                  <span className="qc-meta-value">{selectedTask.params?.ratio || '-'}</span>
                  <span className="qc-meta-label">计划时间</span>
                  <span className="qc-meta-value">
                    {selectedTask.status === 'scheduled' && selectedTask.scheduled_at
                      ? <span className={`qc-scheduled-pill${new Date(selectedTask.scheduled_at).getTime() <= Date.now() ? ' overdue' : ''}`}>
                          {new Date(selectedTask.scheduled_at).getTime() <= Date.now()
                            ? <>已到期，等待补偿提交 <AlertCircle size={12} /></>
                            : `预计提交：${formatDate(selectedTask.scheduled_at)}`}
                        </span>
                      : '未定时'}
                  </span>
                  <span className="qc-meta-label">更新时间</span>
                  <span className="qc-meta-value">{formatDate(selectedTask.updated_at) || '-'}</span>
                  {currentExecution?.submit_id ? (
                    <>
                      <span className="qc-meta-label">submit_id</span>
                      <span className="qc-meta-value qc-submit-id-row">
                        <span className="qc-submit-id mono">{currentExecution.submit_id}</span>
                        <button type="button" className="icon-ghost mini"
                          onClick={() => navigator.clipboard.writeText(currentExecution.submit_id).catch(() => {})}>
                          <Copy size={11} />
                        </button>
                      </span>
                    </>
                  ) : null}
                  {executionView?.status === 'querying' ? (
                    <>
                      <span className="qc-meta-label">轮询间隔</span>
                      <span className="qc-meta-value">{state.settings.poll_interval_seconds ?? 60} 秒 / 次</span>
                    </>
                  ) : null}
                </div>
              </section>

              {/* Command */}
              {commandPresentation.hasCommand ? (
                <section className="qc-section">
                  <div className="qc-section-head">
                    <h4 className="qc-section-title">命令预览</h4>
                    <div className="qc-command-actions">
                      <button
                        type="button"
                        className="qc-command-toggle"
                        onClick={openCommandPreview}
                      >
                        <ExternalLink size={13} />
                        {commandPresentation.actionLabel}
                      </button>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="qc-command-collapsed"
                    onClick={openCommandPreview}
                  >
                    <Command size={13} />
                    <span>{commandPresentation.hint}</span>
                  </button>
                </section>
              ) : null}

              {/* Hit resources */}
              <section className="qc-section">
                <h4 className="qc-section-title">命中资源</h4>
                {hitResources.length ? (
                  <div className="qc-resource-grid">
                    {hitResources.map(({ type, displayType, label, asset }) => (
                      <button
                        key={`${displayType}:${asset.id}`}
                        type="button"
                        className={`qc-resource-item ${type} ${displayType}`}
                        title={`预览${label}：${asset.name || asset.id.slice(0, 8)}`}
                        onClick={() => setResourcePreview({ type, displayType, asset })}
                      >
                        {type === 'image' && asset.stored_path ? (
                          <img src={convertFileSrc(asset.stored_path)} alt="" className="qc-resource-thumb" />
                        ) : (
                          <div className="qc-resource-icon">{type === 'audio' ? <FileAudio size={16} /> : <Image size={16} />}</div>
                        )}
                        <span className="qc-resource-tag">{label}</span>
                        <span className="qc-resource-name">{asset.name || asset.id.slice(0, 8)}</span>
                      </button>
                    ))}
                  </div>
                ) : <p className="qc-empty-sm">无命中资源</p>}
              </section>

              {/* Results */}
              {resultItems.length ? (
                <section className="qc-section">
                  <h4 className="qc-section-title">生成结果</h4>
                  {resultItems.map((item) => (
                    <div key={`${item.kind}:${item.value}`} className="qc-result-card">
                      {item.kind === 'path' ? (
                        <video
                          className="qc-result-video"
                          src={convertFileSrc(item.value)}
                          controls
                          preload="metadata"
                        />
                      ) : (
                        <video
                          className="qc-result-video"
                          src={item.value}
                          controls
                          preload="metadata"
                        />
                      )}
                      <div className="qc-result-row">
                        <span className="mono qc-result-path" title={item.value}>{item.label}</span>
                        {item.kind === 'path' ? (
                          <button type="button" className="icon-ghost mini" title="打开所在目录"
                            onClick={async () => { try { await invoke('open_result_dir_command', { path: item.value }); } catch (e) { setFeedback(String(e)); } }}>
                            <FolderOpen size={12} />
                          </button>
                        ) : (
                          <button type="button" className="icon-ghost mini" title="在浏览器打开"
                            onClick={() => window.open(item.value, '_blank', 'noopener,noreferrer')}>
                            <ExternalLink size={12} />
                          </button>
                        )}
                      </div>
                    </div>
                  ))}
                </section>
              ) : null}

              {/* Execution History */}
              {taskHistory.length > 0 ? (
                <section className="qc-section">
                  <h4 className="qc-section-title">执行记录</h4>
                  <div className="qc-history-list">
                    {taskHistory.map((item, idx) => (
                      <div
                        key={item.id}
                        className={`qc-history-item${currentExecution?.id === item.id ? ' selected' : ''}`}
                        role="button"
                        tabIndex={0}
                        onClick={() => setSelectedExecutionId(item.id)}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') {
                            event.preventDefault();
                            setSelectedExecutionId(item.id);
                          }
                        }}
                      >
                        <div className="qc-history-header">
                          <span className="qc-history-label">{historyItemLabel(item, taskHistory.length - idx)}</span>
                          <span className={`status-badge ${item.status}`}>{item.status}</span>
                          {currentExecution?.id === item.id ? <span className="qc-history-current">当前查看</span> : null}
                          {item.finished_at ? <span className="qc-history-time">{item.finished_at.slice(0, 16).replace('T', ' ')}</span> : null}
                          <div className="qc-history-item-actions">
                            {item.submit_id ? (
                              <button
                                type="button"
                                className="icon-ghost mini"
                                title={`查询此次结果（${item.submit_id.slice(0, 8)}）`}
                                disabled={pendingExecutionOps[item.id]?.query}
                                onClick={(event) => {
                                  event.stopPropagation();
                                  queryExecutionRecord(selectedTask.id, item.id, item.submit_id);
                                }}
                              >
                                {pendingExecutionOps[item.id]?.query
                                  ? <Loader2 size={11} className="spin" />
                                  : <RefreshCcw size={11} />}
                              </button>
                            ) : null}
                            <button
                              type="button"
                              className="icon-ghost mini danger"
                              title="删除此条执行记录"
                              onClick={(event) => {
                                event.stopPropagation();
                                handleDeleteExecutionRecord(
                                  selectedTask.id,
                                  item.id,
                                  historyItemLabel(item, taskHistory.length - idx)
                                );
                              }}
                            >
                              <Trash2 size={11} />
                            </button>
                          </div>
                        </div>
                        {item.result_paths.length > 0 || item.result_urls.length > 0 ? (
                          <div className="qc-history-file-list">
                            {item.result_paths.map((p) => (
                              <div key={p} className="qc-history-file-row">
                                <span className="mono qc-result-path" title={p}>{p.split('/').pop()}</span>
                                <button type="button" className="icon-ghost mini" title="打开所在目录"
                                  onClick={async (event) => { event.stopPropagation(); try { await invoke('open_result_dir_command', { path: p }); } catch (e) { setFeedback(String(e)); } }}>
                                  <FolderOpen size={12} />
                                </button>
                              </div>
                            ))}
                            {item.result_paths.length === 0 && item.result_urls.map((u) => (
                              <div key={u} className="qc-history-file-row">
                                <span className="mono qc-result-path" title={u}>{u.split('/').pop()?.slice(0, 40) || u}</span>
                                <button type="button" className="icon-ghost mini" title="在浏览器打开"
                                  onClick={(event) => { event.stopPropagation(); window.open(u, '_blank', 'noopener,noreferrer'); }}>
                                  <ExternalLink size={12} />
                                </button>
                              </div>
                            ))}
                          </div>
                        ) : null}
                        {item.error_detail && !isInterruptNotice(item.error_detail) && item.result_paths.length === 0 && item.result_urls.length === 0 ? (
                          <p className="qc-error-text" style={{ marginTop: 4 }}>{item.error_detail}</p>
                        ) : null}
                      </div>
                    ))}
                  </div>
                </section>
              ) : null}
              </div>

              {/* Actions */}
              <div className="qc-detail-actions">
                <button type="button" className="qc-btn" onClick={() => handleEditTask(selectedTask)}>
                  <Pencil size={13} /> 编辑
                </button>
                <button type="button" className="qc-btn" onClick={() => handleDuplicateTask(selectedTask)}>
                  <Copy size={13} /> 复制
                </button>
                <button type="button" className="qc-btn qc-btn-primary" onClick={() => openPrepareGenerate(selectedTask)}
                  disabled={!canScheduleTask(selectedTask) || pendingTaskOps[selectedTask.id]?.submit}>
                  {pendingTaskOps[selectedTask.id]?.submit
                    ? <><Loader2 size={13} className="spin" /> 提交中</>
                    : <><Play size={13} /> 准备生成</>}
                </button>
                <button type="button" className="qc-btn" onClick={() => queryTask(selectedTask.id, currentExecution?.submit_id || null)}
                  disabled={!currentExecution?.submit_id || pendingTaskOps[selectedTask.id]?.query}>
                  {pendingTaskOps[selectedTask.id]?.query
                    ? <><Loader2 size={13} className="spin" /> 查询中</>
                    : <><RefreshCcw size={13} /> 查询本次结果</>}
                </button>
                {['scheduled', 'queued', 'retry_wait'].includes(selectedTask.status) ? (
                  <button type="button" className="qc-btn" onClick={() => pauseTask(selectedTask.id)}>暂停</button>
                ) : null}
                {selectedTask.status === 'paused' ? (
                  <>
                    <button type="button" className="qc-btn" onClick={() => resumeTask(selectedTask.id, 'immediate')}>立即恢复</button>
                    {selectedTask.scheduled_at ? (
                      <button type="button" className="qc-btn" onClick={() => resumeTask(selectedTask.id, 'scheduled')}>按计划恢复</button>
                    ) : null}
                  </>
                ) : null}
                {selectedTask.status === 'scheduled' ? (
                  <button
                    type="button"
                    className="qc-btn"
                    onClick={() => askConfirm({
                      title: '取消预定',
                      body: '只取消计划时间，任务、执行记录和素材不会被删除。取消后任务回到待生成状态。',
                      confirmText: '取消预定',
                      onConfirm: async () => {
                        await rescheduleTask(selectedTask.id, '');
                        setFeedback('已取消预定');
                      },
                    })}
                  >
                    <X size={13} /> 取消预定
                  </button>
                ) : null}
                {canDeleteTask(selectedTask) ? (
                  <button type="button" className="qc-btn qc-btn-danger" onClick={() => handleDeleteTask(selectedTask)}>
                    <Trash2 size={13} /> 删除任务
                  </button>
                ) : null}
              </div>
            </>
          ) : (
            <div className="qc-empty qc-empty-panel">
              <ClipboardList size={28} />
              <p>选择任务查看详情</p>
            </div>
          )}
        </div>

        {/* ── RIGHT: execution monitor ── */}
        <div className="qc-monitor">
          {selectedTask ? (
            <>
              <section className="qc-section">
                <h4 className="qc-section-title">执行监控</h4>
                <div className="qc-progress-area">
                  <RingProgress percent={progress.percent} status={executionView?.status || selectedTask.status} />
                  <div className="qc-progress-info">
                    <span className="qc-stage">{progress.stage}</span>
                    <div className="qc-progress-bar-track">
                      <div className="qc-progress-bar-fill" style={{ width: `${progress.percent}%` }} />
                    </div>
                    {selectedTask.status !== 'succeeded' && (
                    <div className="qc-monitor-meta-row">
                      <span>重试</span>
                      <span>{selectedTask.attempt_count || 0} / {settings?.concurrency_retry_max_attempts || 8}</span>
                    </div>
                    )}
                    {executionView?.status === 'querying' && executionView.queue_info ? (
                    <div className="qc-queue-progress">
                      <div className="qc-queue-progress-head">
                        <span className="qc-queue-label">排队位置</span>
                        <span className="qc-queue-pos">
                          <b>#{executionView.queue_info.queue_idx ?? '-'}</b>
                          <em> / {executionView.queue_info.queue_length != null ? executionView.queue_info.queue_length.toLocaleString() : '-'}</em>
                        </span>
                      </div>
                      <div className="qc-queue-bar-track">
                        <div className="qc-queue-bar-fill" style={{
                          width: `${executionView.queue_info.queue_idx != null && executionView.queue_info.queue_length
                            ? Math.max(2, Math.round((1 - executionView.queue_info.queue_idx / executionView.queue_info.queue_length) * 100))
                            : 0}%`
                        }} />
                      </div>
                      <div className="qc-queue-sub">
                        <span>优先级 {executionView.queue_info.priority ?? '-'}</span>
                        <span>{executionView.queue_info.queue_status ?? ''}</span>
                      </div>
                    </div>
                  ) : null}
                  <div className="qc-monitor-meta-row">
                      <span>更新</span>
                      <span>{formatDatePart(selectedTask.updated_at, 'time') || '-'}</span>
                    </div>
                  </div>
                </div>
              </section>

              <section className="qc-section">
                <div className="qc-section-head">
                  <h4 className="qc-section-title">查询记录</h4>
                  {allAttempts.length > 4 ? (
                    <button type="button" className="qc-link-btn" onClick={() => setShowAttemptsModal(true)}>
                      查看全部 ({allAttempts.length})
                    </button>
                  ) : null}
                </div>
                {recentAttempts.length ? (
                  <div className="qc-attempt-log">
                    {recentAttempts.map((attempt) => <AttemptRow key={attempt.id} attempt={attempt} />)}
                  </div>
                ) : <p className="qc-empty-sm">暂无查询记录</p>}
              </section>
              {showAttemptsModal ? (
                <AttemptsModal attempts={allAttempts} onClose={() => setShowAttemptsModal(false)} />
              ) : null}

              <section className="qc-section">
                <h4 className="qc-section-title">异常与重试策略</h4>
                {executionView?.last_error ? (
                  <p className="qc-error-text">{executionView.last_error}</p>
                ) : null}
                <div className="qc-retry-grid">
                  <span>重试条件</span><span>并发上限错误</span>
                  <span>重试方式</span><span>静默重试</span>
                  <span>最大重试</span><span>{settings?.concurrency_retry_max_attempts || 8} 次</span>
                  <span>最终策略</span><span>标记失败</span>
                </div>
              </section>

              <div className="qc-health-hint">
                {executionView?.status === 'succeeded' ? (
                  <span className="qc-health done"><CheckCircle2 size={12} /> 任务已成功完成</span>
                ) : ['failed'].includes(executionView?.status) ? (
                  <span className="qc-health fail"><AlertCircle size={12} /> 任务失败，可重新提交</span>
                ) : executionView?.status === 'submitted' && selectedTask?.auto_query_stopped ? (
                  <span className="qc-health fail"><AlertCircle size={12} /> 自动查询已停止（等待超过 4 小时），请手动查询</span>
                ) : executionView?.status === 'submitted' ? (
                  <span className="qc-health running"><Clock3 size={12} /> 已提交，等待自动查询</span>
                ) : executionView?.status === 'querying' ? (
                  <span className="qc-health running"><Loader2 size={12} className="spin" /> {executionView.queue_info ? '排队等待生成中' : '查询结果中'}</span>
                ) : executionView?.status === 'submitting' ? (
                  <span className="qc-health running"><Loader2 size={12} className="spin" /> 提交中</span>
                ) : executionView?.status === 'retry_wait' ? (
                  <span className="qc-health retry"><RefreshCcw size={12} /> 等待重试</span>
                ) : (
                  <span className="qc-health idle"><Clock3 size={12} /> 等待调度</span>
                )}
              </div>
            </>
          ) : (
            <div className="qc-empty qc-empty-panel">
              <Gauge size={28} />
              <p>选中任务后显示监控</p>
            </div>
          )}
        </div>
      </div>
      {resourcePreview?.type === 'image' && resourcePreview.asset?.stored_path ? (
        <ImageModal
          src={convertFileSrc(resourcePreview.asset.stored_path)}
          alt={resourcePreview.asset.name || ''}
          onClose={() => setResourcePreview(null)}
        />
      ) : null}
      {resourcePreview?.type === 'audio' ? (
        <AudioAssetModal asset={resourcePreview.asset} onClose={() => setResourcePreview(null)} />
      ) : null}
      {scheduleModal ? (
        <SchedulePickerModal
          title={scheduleModal.title}
          mode={scheduleModal.mode}
          taskCount={scheduleModal.taskIds.length}
          onClose={() => setScheduleModal(null)}
          onApply={applySchedulePlan}
        />
      ) : null}
      {commandPreviewModal ? (
        <CommandPreviewModal
          title={commandPreviewModal.title}
          commandText={commandPreviewModal.commandText}
          onClose={() => setCommandPreviewModal(null)}
        />
      ) : null}
    </div>
  );
}

function CommandPreviewModal({ title, commandText, onClose }) {
  const copyCommand = () => navigator.clipboard.writeText(commandText).catch(() => {});

  return (
    <div className="modal-backdrop command-preview-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="command-preview-dialog" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
        <header className="command-preview-dialog-head">
          <div>
            <span>命令预览</span>
            <h3>{title || '未命名任务'}</h3>
          </div>
          <button type="button" className="icon-ghost" onClick={onClose}><X size={16} /></button>
        </header>
        <pre className="command-preview-dialog-block"><code>{commandText}</code></pre>
        <footer>
          <button type="button" className="outline-button" onClick={copyCommand}>
            <Copy size={14} /> 复制命令
          </button>
          <button type="button" className="gradient-button" onClick={onClose}>关闭</button>
        </footer>
      </section>
    </div>
  );
}


function LogsView({ logs, clearLogs }) {
  return (
    <div className="panel full-panel">
      <PanelHeading title="日志" action={<button type="button" className="outline-button" onClick={clearLogs}>清空日志</button>} />
      <div className="log-list">
        {logs.map((line, index) => <p key={`${line}-${index}`}>{line}</p>)}
        {!logs.length ? <p className="empty-cell">暂无日志。</p> : null}
      </div>
    </div>
  );
}

function SchedulePickerModal({ title, mode = 'single', taskCount = 1, onClose, onApply }) {
  const isBatch = mode === 'batch';
  const isPrepare = mode === 'prepare';
  const today = formatDateInputValue(new Date());
  const tomorrowDate = new Date();
  tomorrowDate.setDate(tomorrowDate.getDate() + 1);
  const [scheduleMode, setScheduleMode] = useState(isBatch ? 'relative' : 'immediate');
  const [relativeHours, setRelativeHours] = useState(2);
  const [day, setDay] = useState('tomorrow');
  const [quietTime, setQuietTime] = useState('02:00');
  const [customDate, setCustomDate] = useState(formatDateInputValue(tomorrowDate));
  const [customTime, setCustomTime] = useState('02:00');
  const [intervalMinutes, setIntervalMinutes] = useState(30);
  const [error, setError] = useState('');

  const scheduleOptions = [
    {
      key: 'immediate',
      label: isPrepare ? '立即生成' : '立即提交',
      hint: isBatch ? '第一条现在开始，其余按间隔排布' : isPrepare ? '现在提交到即梦生成' : '移除计划时间，回到待提交',
    },
    { key: 'relative', label: '几小时后', hint: '适合临时错峰提交' },
    { key: 'dayTime', label: '今天 / 明天凌晨', hint: '常用夜间批量提交' },
    { key: 'custom', label: '自定义时间', hint: '指定日期和分钟' },
  ];

  const buildSelectedScheduleAt = () => {
    if (scheduleMode === 'relative') {
      return resolveScheduleAt({ mode: 'relative', hours: relativeHours });
    }
    if (scheduleMode === 'dayTime') {
      return resolveScheduleAt({ mode: 'dayTime', day, time: quietTime });
    }
    if (scheduleMode === 'custom') {
      return resolveScheduleAt({ mode: 'custom', customValue: `${customDate}T${customTime}` });
    }
    return isBatch ? new Date(Date.now() + 1000).toISOString() : null;
  };

  const handleApply = () => {
    setError('');
    let scheduledAt;
    try {
      scheduledAt = buildSelectedScheduleAt();
    } catch (err) {
      setError('请选择有效时间');
      return;
    }
    if (scheduledAt && new Date(scheduledAt).getTime() <= Date.now()) {
      setError('计划时间必须晚于当前时间');
      return;
    }
    onApply?.({ scheduledAt, intervalMinutes });
  };

  return (
    <div className="modal-backdrop schedule-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="schedule-modal" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
        <header className="schedule-modal-head">
          <div>
            <span>{isBatch ? `批量排布 ${taskCount} 个任务` : isPrepare ? '准备生成' : '单个任务排期'}</span>
            <h3>{title || '安排提交时间'}</h3>
          </div>
          <button type="button" className="icon-ghost" onClick={onClose}><X size={16} /></button>
        </header>

        <div className="schedule-option-grid">
          {scheduleOptions.map((option) => (
            <button
              key={option.key}
              type="button"
              className={`schedule-option${scheduleMode === option.key ? ' active' : ''}`}
              onClick={() => setScheduleMode(option.key)}
            >
              <strong>{option.label}</strong>
              <span>{option.hint}</span>
            </button>
          ))}
        </div>

        <div className="schedule-controls">
          {scheduleMode === 'relative' ? (
            <label>
              <span>延后小时</span>
              <input type="number" min="1" max="72" value={relativeHours} onChange={(e) => setRelativeHours(e.target.value)} />
            </label>
          ) : null}
          {scheduleMode === 'dayTime' ? (
            <div className="schedule-inline-fields">
              <label>
                <span>日期</span>
                <select value={day} onChange={(e) => setDay(e.target.value)}>
                  <option value="today">今天凌晨</option>
                  <option value="tomorrow">明天凌晨</option>
                </select>
              </label>
              <label>
                <span>时间</span>
                <input type="time" value={quietTime} onChange={(e) => setQuietTime(e.target.value)} />
              </label>
            </div>
          ) : null}
          {scheduleMode === 'custom' ? (
            <div className="schedule-inline-fields">
              <label>
                <span>日期</span>
                <input type="date" min={today} value={customDate} onChange={(e) => setCustomDate(e.target.value)} />
              </label>
              <label>
                <span>时间</span>
                <input type="time" value={customTime} onChange={(e) => setCustomTime(e.target.value)} />
              </label>
            </div>
          ) : null}
          {isBatch ? (
            <label>
              <span>每隔</span>
              <select value={intervalMinutes} onChange={(e) => setIntervalMinutes(Number(e.target.value))}>
                <option value={15}>15 分钟</option>
                <option value={30}>30 分钟</option>
                <option value={45}>45 分钟</option>
                <option value={60}>1 小时</option>
                <option value={90}>1.5 小时</option>
              </select>
            </label>
          ) : null}
        </div>

        {error ? <p className="schedule-error">{error}</p> : null}

        <footer className="schedule-modal-actions">
          <button type="button" className="outline-button" onClick={onClose}>取消</button>
          <button type="button" className="gradient-button" onClick={handleApply}>
            <CalendarClock size={14} /> {isBatch ? '确认排布' : isPrepare ? (scheduleMode === 'immediate' ? '立即生成' : '确认定时生成') : '确认安排'}
          </button>
        </footer>
      </section>
    </div>
  );
}

function SettingsView({ cli, settingsForm, setSettingsForm, checkCli, saveSettings, installCli, installCliStatus, loginCli, loginCliStatus }) {
  const aiModelConfigs = settingsForm.ai_model_configs?.length ? settingsForm.ai_model_configs : [defaultAiModelConfig];
  const activeAiModelId = settingsForm.active_ai_model_id || aiModelConfigs[0]?.id || defaultAiModelConfig.id;
  const patchAiModel = (index, patch) => {
    const next = aiModelConfigs.map((config, configIndex) => (
      configIndex === index ? { ...config, ...patch } : config
    ));
    setSettingsForm({ ...settingsForm, ai_model_configs: next, active_ai_model_id: activeAiModelId });
  };
  const addAiModel = () => {
    const id = `openai-${Date.now()}`;
    setSettingsForm({
      ...settingsForm,
      ai_model_configs: [
        ...aiModelConfigs,
        {
          ...defaultAiModelConfig,
          id,
          name: `OpenAI 配置 ${aiModelConfigs.length + 1}`,
        },
      ],
      active_ai_model_id: id,
    });
  };
  const removeAiModel = (id) => {
    const next = aiModelConfigs.filter((config) => config.id !== id);
    const fallback = next[0] || defaultAiModelConfig;
    setSettingsForm({
      ...settingsForm,
      ai_model_configs: next.length ? next : [fallback],
      active_ai_model_id: activeAiModelId === id ? fallback.id : activeAiModelId,
    });
  };

  return (
    <div className="settings-layout">
      <form className="panel" onSubmit={saveSettings}>
        <PanelHeading title="设置" />
        <div className="setting-group">
          <h3>CLI 配置</h3>
          <label>CLI 路径<input value={cli.path || '未检测到'} readOnly /></label>
          <p className={cli.available ? 'good-text' : 'error-text'}>{cli.available ? '检测正常' : cli.message}</p>
          <div className="button-cluster">
            <button className="outline-button" type="button" onClick={checkCli}>重新检测</button>
            <button className="outline-button" type="button" onClick={installCli} disabled={installCliStatus === 'installing'}>
              {installCliStatus === 'installing' ? '安装中…' : '一键安装'}
            </button>
            {installCliStatus === 'success' ? <p className="good-text">安装成功</p> : null}
            {installCliStatus === 'failed' ? <p className="error-text">安装失败，请检查日志</p> : null}
          </div>
          <div className="button-cluster">
            <button className="outline-button" type="button" onClick={() => loginCli(false)} disabled={!cli.available || loginCliStatus === 'logging'}>
              {loginCliStatus === 'logging' ? '登录中…' : 'CLI 登录'}
            </button>
            <button className="outline-button" type="button" onClick={() => loginCli(true)} disabled={!cli.available || loginCliStatus === 'logging'}>
              Headless 登录
            </button>
            {loginCliStatus === 'success' ? <p className="good-text">登录流程完成</p> : null}
            {loginCliStatus === 'failed' ? <p className="error-text">登录失败，请检查日志</p> : null}
          </div>
          <label>
            macOS 安装命令
            <input
              value={settingsForm.mac_install_command || ''}
              onChange={(event) => setSettingsForm({ ...settingsForm, mac_install_command: event.target.value })}
            />
          </label>
          <label>
            Windows PowerShell 安装命令
            <input
              value={settingsForm.windows_install_command || ''}
              placeholder="填入官方 PowerShell 安装命令后启用 Windows 一键安装"
              onChange={(event) => setSettingsForm({ ...settingsForm, windows_install_command: event.target.value })}
            />
          </label>
        </div>
        <div className="setting-group">
          <div className="setting-group-head">
            <h3>AI 模型配置</h3>
            <button className="outline-button" type="button" onClick={addAiModel}>
              <Plus size={13} /> 新增模型
            </button>
          </div>
          <label>
            当前使用模型
            <select
              value={activeAiModelId}
              onChange={(event) => setSettingsForm({ ...settingsForm, active_ai_model_id: event.target.value, ai_model_configs: aiModelConfigs })}
            >
              {aiModelConfigs.map((config) => (
                <option key={config.id} value={config.id}>{config.name || config.model || config.id}</option>
              ))}
            </select>
          </label>
          <p className="setting-hint">用于保存任务时自动生成简短标题；未配置 API Key 时会自动回退到本地标题。</p>
          <div className="ai-model-list">
            {aiModelConfigs.map((config, index) => (
              <div className={`ai-model-card${config.id === activeAiModelId ? ' active' : ''}`} key={config.id}>
                <div className="ai-model-card-head">
                  <strong>{config.name || '未命名模型'}</strong>
                  <div className="ai-model-card-actions">
                    <AiModelTestButton config={config} />
                    <button
                      type="button"
                      className="icon-ghost mini"
                      title="设为当前模型"
                      onClick={() => setSettingsForm({ ...settingsForm, active_ai_model_id: config.id, ai_model_configs: aiModelConfigs })}
                    >
                      <CheckCircle2 size={12} />
                    </button>
                    <button
                      type="button"
                      className="icon-ghost mini"
                      title="删除模型"
                      disabled={aiModelConfigs.length <= 1}
                      onClick={() => removeAiModel(config.id)}
                    >
                      <Trash2 size={12} />
                    </button>
                  </div>
                </div>
                <div className="ai-model-grid">
                  <label>
                    名称
                    <input value={config.name || ''} onChange={(event) => patchAiModel(index, { name: event.target.value })} />
                  </label>
                  <label>
                    模式
                    <select value={config.api_mode || 'responses'} onChange={(event) => patchAiModel(index, { api_mode: event.target.value })}>
                      <option value="responses">OpenAI Responses</option>
                      <option value="chat">Chat Completions</option>
                    </select>
                  </label>
                  <label>
                    Base URL
                    <input value={config.base_url || ''} onChange={(event) => patchAiModel(index, { base_url: event.target.value })} />
                  </label>
                  <label>
                    Model
                    <input value={config.model || ''} onChange={(event) => patchAiModel(index, { model: event.target.value })} />
                  </label>
                  <label className="ai-model-secret">
                    API Key
                    <input
                      type="password"
                      value={config.api_key || ''}
                      placeholder="sk-..."
                      onChange={(event) => patchAiModel(index, { api_key: event.target.value })}
                    />
                  </label>
                </div>
              </div>
            ))}
          </div>
        </div>
        <div className="setting-group">
          <h3>自动查询设置</h3>
          <label className="switch-line">
            <input
              type="checkbox"
              checked={settingsForm.auto_query_enabled ?? true}
              onChange={(event) => setSettingsForm({ ...settingsForm, auto_query_enabled: event.target.checked })}
            />
            提交后自动查询结果
          </label>
          <label>
            轮询间隔（秒）
            <input
              type="number"
              min="10"
              max="300"
              value={settingsForm.poll_interval_seconds ?? 60}
              onChange={(event) => setSettingsForm({ ...settingsForm, poll_interval_seconds: Number(event.target.value) })}
            />
          </label>
          <label>
            日志保留条数
            <input
              type="number"
              min="50"
              max="10000"
              value={settingsForm.log_retention_count ?? 500}
              onChange={(event) => setSettingsForm({ ...settingsForm, log_retention_count: Number(event.target.value) })}
            />
          </label>
          <label className="setting-row-check">
            <span>预定任务期间防止系统睡眠</span>
            <input
              type="checkbox"
              checked={settingsForm.prevent_sleep ?? true}
              onChange={(event) => setSettingsForm({ ...settingsForm, prevent_sleep: event.target.checked })}
            />
            <small>仅在应用运行时有效，不能防止关机或退出应用。开启后可能增加耗电，但可提升准点提交概率。macOS 使用 caffeinate，Windows 使用 SetThreadExecutionState；Linux 暂不支持。</small>
          </label>
        </div>
        <div className="setting-group">
          <h3>并发限制策略</h3>
          <label>
            并发限制策略
            <select
              value={settingsForm.concurrency_limit_policy || 'SilentRetry'}
              onChange={(event) => setSettingsForm({ ...settingsForm, concurrency_limit_policy: event.target.value })}
            >
              <option value="SilentRetry">静默重试</option>
              <option value="SilentFail">静默失败</option>
            </select>
          </label>
          <label>
            最大重试次数
            <input
              type="number"
              min="0"
              value={settingsForm.concurrency_retry_max_attempts || 8}
              onChange={(event) => setSettingsForm({ ...settingsForm, concurrency_retry_max_attempts: event.target.value })}
            />
          </label>
          <label>
            并发重试间隔（秒）
            <input
              type="number"
              min="30"
              value={settingsForm.concurrency_retry_delay_seconds || 300}
              onChange={(event) => setSettingsForm({ ...settingsForm, concurrency_retry_delay_seconds: event.target.value })}
            />
          </label>
          <button className="gradient-button" type="submit">保存设置</button>
        </div>
      </form>
      <aside className="panel roadmap">
        <h3>未来功能</h3>
        {['CLI 一键安装源配置', '完整任务历史筛选', '日志保留清理'].map((item) => (
          <p key={item}><CheckCircle2 size={15} /> {item}<span>即将上线</span></p>
        ))}
      </aside>
    </div>
  );
}

function Metric({ title, value, sub, tone }) {
  return (
    <div className={`metric-card ${tone || ''}`}>
      <span>{title}</span>
      <strong>{value}</strong>
      {sub ? <em>{sub}</em> : null}
    </div>
  );
}

function PanelHeading({ title, action }) {
  return (
    <div className="panel-heading">
      <h2>{title}</h2>
      {action}
    </div>
  );
}

function SecondaryPageHeader({ title, backLabel, onBack, actions }) {
  return (
    <div className="secondary-page-header">
      <div className="secondary-page-title-group">
        <button type="button" className="ghost-link secondary-page-back" onClick={onBack}>
          <ArrowLeft size={18} /> {backLabel}
        </button>
        <h2>{title}</h2>
      </div>
      {actions ? <div className="secondary-page-actions">{actions}</div> : null}
    </div>
  );
}

function TaskDetail({ task, submitTask, queryTask }) {
  if (!task) {
    return (
      <aside className="panel task-detail">
        <PanelHeading title="任务详情" />
        <p className="empty-cell">暂无任务。</p>
      </aside>
    );
  }
  return (
    <aside className="panel task-detail">
      <div className="task-detail-head">
        <div>
          <h2>任务详情：{task.title || '未命名任务'}</h2>
          <StatusBadge task={task} />
        </div>
        <div className="button-cluster">
          <button type="button" onClick={() => submitTask(task.id)}><Play size={15} />立即生成</button>
          <button type="button" onClick={() => queryTask(task.id)} disabled={!task.submit_id}><RefreshCcw size={15} />查询结果</button>
        </div>
      </div>
      <dl>
        <dt>模型</dt><dd>{task.params?.model_version}</dd>
        <dt>宽高比</dt><dd>{task.params?.ratio}</dd>
        <dt>计划时间</dt><dd>{formatDate(task.scheduled_at) || '-'}</dd>
        <dt>下次执行</dt><dd>{formatDate(task.next_run_at) || '-'}</dd>
        <dt>submit_id</dt><dd>{task.submit_id || '-'}</dd>
        <dt>attempt</dt><dd>{task.attempt_count || 0}</dd>
      </dl>
      <h3>命令预览</h3>
      <code>{task.command_preview?.join(' \\\n  ') || '暂无命令预览：需要提示词和至少 1 张图片素材。'}</code>
      {task.result_urls?.length || task.result_paths?.length ? (
        <section className="result-list">
          <h3>生成结果</h3>
          {task.result_urls?.map((url) => <a key={url} href={url}>{url}</a>)}
          {task.result_paths?.map((path) => <p key={path}>{path}</p>)}
        </section>
      ) : null}
      {task.last_error ? <p className="error-text">{task.last_error}</p> : null}
      {(() => {
        const qRecords = deriveCurrentQueryRecords(task).slice().reverse();
        return qRecords.length ? (
          <section className="attempt-list">
            <h3>查询记录</h3>
            {qRecords.map((attempt) => (
              <article key={attempt.id}>
                <strong>{statusLabel(attempt.status)} · {formatDate(attempt.finished_at)}</strong>
                {attempt.error_kind ? <span>{attempt.error_kind}</span> : null}
                {attempt.stderr ? <p>{attempt.stderr}</p> : null}
              </article>
            ))}
          </section>
        ) : null;
      })()}
    </aside>
  );
}

function TaskRow({ task, submitTask, queryTask }) {
  return (
    <article className="task-row">
      <div>
        <strong>{task.title || '未命名任务'}</strong>
        <p>{task.prompt}</p>
        {task.last_error ? <p className="error-text">{task.last_error}</p> : null}
      </div>
        <StatusBadge task={task} />
        <div className="row-actions">
        <button type="button" title="立即生成" onClick={() => submitTask(task.id)}><Play size={14} /></button>
        <button type="button" title="查询结果" disabled={!task.submit_id} onClick={() => queryTask(task.id)}><RefreshCcw size={14} /></button>
      </div>
    </article>
  );
}

function AiModelTestButton({ config }) {
  const [state, setState] = useState('idle'); // idle | loading | ok | err
  const [msg, setMsg] = useState('');
  async function runTest() {
    if (state === 'loading') return;
    setState('loading');
    setMsg('');
    try {
      const result = await invoke('test_ai_model_command', {
        apiKey: config.api_key || '',
        baseUrl: config.base_url || '',
        model: config.model || '',
        apiMode: config.api_mode || 'responses',
      });
      setState('ok');
      setMsg(result || '连接成功');
      setTimeout(() => setState('idle'), 4000);
    } catch (err) {
      setState('err');
      setMsg(String(err).replace(/^Error:\s*/i, ''));
      setTimeout(() => setState('idle'), 6000);
    }
  }
  return (
    <div className="ai-model-test-wrap">
      <button
        type="button"
        className={`ai-model-test-btn ${state}`}
        title="测试连接"
        onClick={runTest}
        disabled={state === 'loading'}
      >
        {state === 'loading' ? <Loader2 size={11} className="spin" /> : <Zap size={11} />}
        {state === 'loading' ? '测试中' : '测试'}
      </button>
      {state === 'ok' && <span className="ai-model-test-result ok"><CheckCircle2 size={11} />{msg}</span>}
      {state === 'err' && <span className="ai-model-test-result err"><AlertCircle size={11} />{msg}</span>}
    </div>
  );
}

function parseAttemptQueueInfo(stdout) {
  try { return JSON.parse(stdout)?.queue_info || null; } catch { return null; }
}

function AttemptRow({ attempt }) {
  const dot = attempt.status === 'succeeded' ? 'done'
    : ['submitting', 'querying'].includes(attempt.status) ? 'running'
    : attempt.status === 'failed' ? 'fail' : 'idle';
  const qi = attempt.status === 'querying' ? parseAttemptQueueInfo(attempt.stdout) : null;
  return (
    <div className="qc-attempt-row">
      <span className={`qc-attempt-dot ${dot}`} />
      <span className="qc-attempt-time">{formatDate(attempt.finished_at || attempt.started_at)}</span>
      <span className="qc-attempt-label">
        {statusLabel(attempt.status)}
        {qi ? (
          <span className="qc-attempt-queue">
            &nbsp;·&nbsp;<b>#{qi.queue_idx ?? '-'}</b>
            <em> / {qi.queue_length != null ? qi.queue_length.toLocaleString() : '-'}</em>
            {qi.queue_status ? <span className="qc-attempt-qs"> {qi.queue_status}</span> : null}
          </span>
        ) : null}
        {attempt.error_kind ? ` · ${attempt.error_kind}` : ''}
      </span>
    </div>
  );
}

function AttemptsModal({ attempts, onClose }) {
  return (
    <div className="attempts-modal-overlay" onClick={onClose}>
      <div className="attempts-modal-card" onClick={(e) => e.stopPropagation()}>
        <div className="attempts-modal-head">
          <h4>全部查询记录（{attempts.length} 条）</h4>
          <button type="button" className="icon-ghost" onClick={onClose}><X size={16} /></button>
        </div>
        <div className="attempts-modal-body">
          {attempts.map((attempt) => {
            const qi = attempt.status === 'querying' ? parseAttemptQueueInfo(attempt.stdout) : null;
            const dot = attempt.status === 'succeeded' ? 'done'
              : ['submitting', 'querying'].includes(attempt.status) ? 'running'
              : attempt.status === 'failed' ? 'fail' : 'idle';
            return (
              <div key={attempt.id} className="attempts-modal-row">
                <span className={`qc-attempt-dot ${dot}`} />
                <div className="attempts-modal-row-main">
                  <div className="attempts-modal-row-top">
                    <span className="attempts-modal-label">{statusLabel(attempt.status)}</span>
                    <span className="attempts-modal-time">{formatDate(attempt.finished_at || attempt.started_at)}</span>
                  </div>
                  {qi ? (
                    <div className="attempts-modal-qi">
                      <span>排队位置 <b>#{qi.queue_idx ?? '-'}</b> / {qi.queue_length != null ? qi.queue_length.toLocaleString() : '-'}</span>
                      {qi.priority != null ? <span>优先级 {qi.priority}</span> : null}
                      {qi.queue_status ? <span>{qi.queue_status}</span> : null}
                    </div>
                  ) : null}
                  {attempt.error_detail ? <div className="attempts-modal-err">{attempt.error_detail}</div> : null}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function taskDisplayLabel(task) {
  if (!task) return '';
  const s = task.status || 'queued';
  if (s === 'scheduled' && task.scheduled_at && new Date(task.scheduled_at).getTime() <= Date.now()) {
    return statusLabel('overdue_scheduled');
  }
  if (s === 'submitted' && task.auto_query_stopped) {
    return '查询已停止';
  }
  if (s === 'querying') {
    const qs = task.queue_info?.queue_status?.toLowerCase() || '';
    const base = (qs.includes('processing') || qs.includes('running')) ? '生成中' : '排队中';
    const qi = task.queue_info;
    if (qi?.queue_idx != null && qi?.queue_length != null) {
      return `${base}(${qi.queue_idx.toLocaleString()}/${qi.queue_length.toLocaleString()})`;
    }
    return base;
  }
  return statusLabel(s);
}

function StatusBadge({ status, task }) {
  const normalized = status || task?.status || 'queued';
  const label = task ? taskDisplayLabel(task) : statusLabel(normalized);
  return <span className={`status-badge ${normalized}`}>{label}</span>;
}

function StatusDot({ status, task }) {
  const normalized = status || task?.status || 'queued';
  const label = task ? taskDisplayLabel(task) : statusLabel(normalized);
  const dotClass = ['submitting', 'querying'].includes(normalized) ? 'running'
    : ['queued', 'scheduled', 'retry_wait'].includes(normalized) ? 'waiting'
    : normalized === 'succeeded' ? 'done'
    : ['failed', 'blocked'].includes(normalized) ? 'fail'
    : 'pending';
  return (
    <span className={`status-dot-pill ${normalized}`}>
      <span className={`status-dot ${dotClass}`} />
      {label}
    </span>
  );
}

function Field({ label, required, children }) {
  return (
    <label className="field">
      <span>{label}{required ? <b>*</b> : null}</span>
      {children}
    </label>
  );
}

function RoleResourcePreview({ title, count, media }) {
  return (
    <section className="resource-section">
      <h3>{title}（{count} 张）</h3>
      <div className="thumb-row">
        {media.map((asset) => <Thumb key={asset.id} asset={asset} label={asset.name} />)}
        {!media.length ? <p className="empty-cell">暂无图片</p> : null}
      </div>
    </section>
  );
}

function AudioPreview({ title, items }) {
  return (
    <section className="resource-section">
      <h3>{title}（{items.length} 条）</h3>
      <div className="audio-list">
        {items.map((asset) => (
          <div className="audio-item" key={asset.id}>
            <button type="button"><Play size={14} /></button>
            <strong>{asset.name}</strong>
            <Waveform />
            <em>{asset.duration_seconds ? `${Math.round(asset.duration_seconds)}s` : '--'}</em>
          </div>
        ))}
        {!items.length ? <p className="empty-cell">暂无音频</p> : null}
      </div>
    </section>
  );
}


function RoleDetailImageSection({ images, chooseFilesForSelectedRole, removeRoleMedia, renameAsset, askConfirm }) {
  const [previewSrc, setPreviewSrc] = useState(null);
  const [previewAlt, setPreviewAlt] = useState('');
  const [renamingId, setRenamingId] = useState(null);
  const [renameValue, setRenameValue] = useState('');

  const startRename = (asset) => {
    setRenamingId(asset.id);
    setRenameValue(asset.name || '');
  };

  const confirmRename = async (assetId) => {
    if (renameValue.trim()) {
      await renameAsset(assetId, renameValue.trim());
    }
    setRenamingId(null);
  };

  return (
    <div className="role-detail-section">
      <div className="role-section-head">
        <h4>图片资源（{images.length}）</h4>
      </div>
      <div className="role-detail-thumb-row">
        {images.map((asset, index) => (
          <div className="role-detail-thumb" key={asset.id}>
            <Thumb asset={asset} label={asset.name || `图片 ${index + 1}`} onClick={() => { setPreviewSrc(convertFileSrc(asset.stored_path)); setPreviewAlt(asset.name); }} />
            {renamingId === asset.id ? (
              <div className="thumb-rename-row">
                <input value={renameValue} onChange={(e) => setRenameValue(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); confirmRename(asset.id); } if (e.key === 'Escape') setRenamingId(null); }} autoFocus />
                <button type="button" onClick={() => confirmRename(asset.id)}>✓</button>
                <button type="button" onClick={() => setRenamingId(null)}>×</button>
              </div>
            ) : (
              <span className="thumb-label" onDoubleClick={() => startRename(asset)}>{asset.name || `图片 ${index + 1}`}</span>
            )}
            <button type="button" className="thumb-action-btn" title="重命名" onClick={() => startRename(asset)}><Pencil size={10} /></button>
            <button
              type="button"
              className="thumb-remove-btn"
              title="移除图片"
              onClick={() => askConfirm({
                title: '移除图片',
                body: `确认从当前角色中移除「${asset.name || '这张图片'}」吗？`,
                confirmText: '移除',
                onConfirm: () => removeRoleMedia(asset.id),
              })}
            >
              <X size={10} />
            </button>
          </div>
        ))}
        <button type="button" className="role-detail-thumb-add" onClick={chooseFilesForSelectedRole}>
          <ImagePlus size={18} />
          <span>添加图片</span>
        </button>
      </div>
      {!images.length ? <p className="role-section-empty">暂无图片，点击添加角色参考图。</p> : null}
      <ImageModal src={previewSrc} alt={previewAlt} onClose={() => setPreviewSrc(null)} />
    </div>
  );
}

function RoleDetailAudioSection({ audios, removeRoleMedia, askConfirm }) {
  const displayAudios = audios.slice(0, 2);
  const [playingId, setPlayingId] = useState(null);
  const audioRef = React.useRef(null);

  const togglePlay = (asset) => {
    if (playingId === asset.id) {
      audioRef.current?.pause();
      setPlayingId(null);
      return;
    }
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.currentTime = 0;
    }
    const audio = new Audio(convertFileSrc(asset.stored_path));
    audio.onended = () => setPlayingId(null);
    audio.onerror = () => setPlayingId(null);
    audioRef.current = audio;
    audio.play().catch(() => setPlayingId(null));
    setPlayingId(asset.id);
  };

  return (
    <div className="role-detail-section">
      <div className="role-section-head">
        <h4>音频资源（{audios.length}）</h4>
        {audios.length > 2 ? <button type="button" className="section-link">查看全部</button> : null}
      </div>
      <div className="role-detail-audio-list">
        {displayAudios.map((asset, index) => (
          <div className="role-detail-audio-row" key={asset.id}>
            <button type="button" className={`play-round${playingId === asset.id ? ' playing' : ''}`} onClick={() => togglePlay(asset)}>
              {playingId === asset.id ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
            </button>
            <strong>{asset.name || `音频 ${index + 1}`}</strong>
            <Waveform active={playingId === asset.id} />
            <em>{asset.duration_seconds ? `${Math.round(asset.duration_seconds)}s` : '--'}</em>
            <button type="button" className="row-delete" title="移除音频"
              onClick={() => askConfirm({
                title: '移除音频',
                body: `确认从当前角色中移除「${asset.name || '这条音频'}」吗？`,
                confirmText: '移除',
                onConfirm: () => removeRoleMedia(asset.id),
              })}
            >
              <MoreHorizontal size={14} />
            </button>
          </div>
        ))}
      </div>
      {!audios.length ? <p className="role-section-empty">暂无音频，导入音频后可作为角色音色素材。</p> : null}
    </div>
  );
}

function RoleDetailVoiceSection({ item, onManageResources, removeRoleMedia, askConfirm }) {
  const [isPlaying, setIsPlaying] = useState(false);
  const audioRef = React.useRef(null);

  const togglePlay = () => {
    if (!item) return;
    if (isPlaying) {
      audioRef.current?.pause();
      setIsPlaying(false);
      return;
    }
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.currentTime = 0;
    }
    const audio = new Audio(convertFileSrc(item.stored_path));
    audio.onended = () => setIsPlaying(false);
    audio.onerror = () => setIsPlaying(false);
    audioRef.current = audio;
    audio.play().catch(() => setIsPlaying(false));
    setIsPlaying(true);
  };

  return (
    <div className="role-detail-section">
      <div className="role-section-head">
        <h4>音色样本（{item ? 1 : 0}）</h4>
        <span className="section-subtitle">默认音色</span>
        <button type="button" className="section-link" onClick={onManageResources}>管理资源</button>
      </div>
      {item ? (
        <div className="voice-sample-detail-card">
          <div className="voice-sample-top">
            <Star size={14} className="voice-star" />
            <button type="button" className={`play-round${isPlaying ? ' playing' : ''}`} onClick={togglePlay}>
              {isPlaying ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
            </button>
            <strong>{item.name || '默认音色'}</strong>
            <Waveform active={isPlaying} />
            <em>{item.duration_seconds ? `${Math.round(item.duration_seconds)}s` : '--'}</em>
            <button type="button" className="icon-ghost mini" title="更多"><MoreHorizontal size={13} /></button>
          </div>
          <div className="voice-sample-bottom">
            <p>作为默认音色样本，用于本角色音色生成</p>
            <span className="voice-active-badge">默认使用中</span>
          </div>
        </div>
      ) : (
        <div className="voice-sample-empty-card">
          <p>还没有设置默认音色，导入音频后会优先使用第一条音频。</p>
          <button type="button" className="outline-button" onClick={onManageResources}>
            <FileAudio size={14} /> 导入音频
          </button>
        </div>
      )}
    </div>
  );
}

function RoleEditPage({
  mode,
  roleForm,
  setRoleForm,
  selectedRole,
  selectedRoleMedia,
  onSave,
  onCancel,
  chooseInitialRoleFile,
  chooseFilesForSelectedRole,
  removeRoleMedia,
  renameAsset,
  askConfirm,
  deleteRole,
}) {
  const currentTags = splitCsv(roleForm.tags);
  const editorMedia = getRoleEditorMedia(mode, selectedRoleMedia, roleForm);
  const primaryImage = editorMedia.images[0];
  const title = mode === 'create' ? '新建角色' : (roleForm.name || selectedRole?.name || '编辑角色');
  const pageHeader = buildSecondaryPageHeaderConfig('role', { mode, name: roleForm.name || selectedRole?.name });
  const [previewSrc, setPreviewSrc] = useState(null);
  const [previewAlt, setPreviewAlt] = useState('');
  const [renamingId, setRenamingId] = useState(null);
  const [renameValue, setRenameValue] = useState('');

  const startRename = (asset) => {
    setRenamingId(asset.id);
    setRenameValue(asset.name || '');
  };

  const confirmRename = async (assetId) => {
    if (renameValue.trim()) {
      await renameAsset(assetId, renameValue.trim());
    }
    setRenamingId(null);
  };

  const openPreview = (path, alt) => {
    if (!path) return;
    setPreviewSrc(convertFileSrc(path));
    setPreviewAlt(alt || '');
  };

  return (
    <form className="role-editor-page" onSubmit={onSave}>
      <div className="role-editor-shell">
        <SecondaryPageHeader
          title={pageHeader.title}
          backLabel={pageHeader.backLabel}
          onBack={onCancel}
          actions={(
            <>
              <button type="button" className="outline-button" onClick={() => chooseInitialRoleFile('image')}>
                <Camera size={15} /> 更换头像
              </button>
              <button type="button" className="icon-ghost"><MoreHorizontal size={16} /></button>
              {mode === 'edit' && selectedRole ? (
                <button
                  type="button"
                  className="danger-outline"
                  onClick={() => askConfirm({
                    title: '删除角色',
                    body: `确认删除「${selectedRole.name}」吗？已被任务引用的角色会被后端阻止删除。`,
                    confirmText: '删除',
                    onConfirm: () => { deleteRole(selectedRole.id); onCancel(); },
                  })}
                >
                  <Trash2 size={14} /> 删除
                </button>
              ) : null}
              <button type="submit" className="gradient-button"><Save size={14} /> 保存</button>
            </>
          )}
        />

        <section className="role-editor-hero">
          <div className="role-editor-avatar">
            <Thumb asset={primaryImage} label={title} />
          </div>
          <div className="role-editor-form">
            <div className="role-editor-title-row">
              <h2>基础信息</h2>
              {selectedRole ? <span className="role-editor-id">ID: {shortRoleId(selectedRole.id)}</span> : null}
              <button type="button" className="icon-ghost"><Pencil size={14} /></button>
            </div>
            <Field label="角色名" required>
              <div className="input-with-count">
                <input value={roleForm.name} maxLength={50} onChange={(e) => setRoleForm({ ...roleForm, name: e.target.value })} placeholder="输入角色名称" />
                <span className="field-count">{roleForm.name.length}/50</span>
              </div>
            </Field>
            <Field label="标签">
              <div className="tag-chip-list">
                {currentTags.map((tag) => (
                  <button
                    type="button"
                    className="tag-chip"
                    key={tag}
                    onClick={() => setRoleForm({ ...roleForm, tags: currentTags.filter((t) => t !== tag).join('，') })}
                  >
                    {tag}<X size={13} />
                  </button>
                ))}
                <button type="button" className="add-tag-chip"><Plus size={14} /> 添加标签 <ChevronDown size={14} /></button>
              </div>
            </Field>
            <Field label="别名">
              <input value={roleForm.aliases} onChange={(e) => setRoleForm({ ...roleForm, aliases: e.target.value })} placeholder="别名，逗号分隔" />
            </Field>
            <Field label="角色描述">
              <div className="prompt-editor">
                <textarea
                  rows={3}
                  value={roleForm.description}
                  maxLength={1000}
                  onChange={(e) => setRoleForm({ ...roleForm, description: e.target.value })}
                  placeholder="描述角色外观、性格、音色或生成时需要保持的一致性。"
                />
                <span className="field-count">{roleForm.description.length}/1000</span>
              </div>
            </Field>
          </div>
        </section>

        <section className="role-editor-resource">
          <div className="role-section-head">
            <h4>参考图片管理（{editorMedia.images.length} 张）</h4>
            <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
              <ImagePlus size={14} /> 添加图片
            </button>
          </div>
          <div className="role-editor-image-row">
            {mode === 'create' && editorMedia.images[0] ? (
              <div className="role-editor-image-card">
                <Thumb asset={editorMedia.images[0]} label="待导入参考图" onClick={() => openPreview(editorMedia.images[0].stored_path, '待导入参考图')} />
                <span className="thumb-label">待导入参考图</span>
                <button
                  type="button"
                  className="thumb-remove-btn"
                  title="移除图片"
                  onClick={() => setRoleForm({ ...roleForm, imagePath: '' })}
                >
                  <X size={10} />
                </button>
              </div>
            ) : editorMedia.images.map((asset, index) => (
              <div className="role-editor-image-card" key={asset.id}>
                <Thumb asset={asset} label={asset.name || `图片 ${index + 1}`} onClick={() => openPreview(asset.stored_path, asset.name)} />
                {renamingId === asset.id ? (
                  <div className="thumb-rename-row">
                    <input value={renameValue} onChange={(e) => setRenameValue(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); confirmRename(asset.id); } if (e.key === 'Escape') setRenamingId(null); }} autoFocus />
                    <button type="button" onClick={() => confirmRename(asset.id)}>✓</button>
                    <button type="button" onClick={() => setRenamingId(null)}>×</button>
                  </div>
                ) : (
                  <span className="thumb-label" onDoubleClick={() => startRename(asset)}>{asset.name || `图片 ${index + 1}`}</span>
                )}
                <button type="button" className="thumb-action-btn" title="重命名" onClick={() => startRename(asset)}><Pencil size={10} /></button>
                <button
                  type="button"
                  className="thumb-remove-btn"
                  title="移除图片"
                  onClick={() => askConfirm({
                    title: '移除图片',
                    body: `确认从当前角色中移除「${asset.name || '这张图片'}」吗？`,
                    confirmText: '移除',
                    onConfirm: () => removeRoleMedia(asset.id),
                  })}
                >
                  <X size={10} />
                </button>
              </div>
            ))}
            {mode === 'create' && !editorMedia.images.length ? (
              <p className="role-section-empty">暂无图片，必须添加至少一张参考图。</p>
            ) : null}
            {mode !== 'create' && !editorMedia.images.length ? (
              <p className="role-section-empty">暂无图片，添加后会作为角色参考图。</p>
            ) : null}
          </div>
        </section>

        <section className="role-editor-resource">
          <div className="role-section-head">
            <h4>音频素材管理（{editorMedia.audios.length} 条）</h4>
            <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
              <FileAudio size={14} /> 添加音频
            </button>
          </div>
          <div className="role-editor-audio-table">
            <div className="role-editor-audio-head"><span>文件名</span><span>时长</span><span>波形</span><span>操作</span></div>
            {mode === 'create' && editorMedia.audios[0] ? (
              <div className="role-editor-audio-row">
                <button type="button" className="play-round"><Play size={14} /></button>
                <strong>{editorMedia.audios[0].name || '待导入音色'}</strong>
                <em>--</em>
                <Waveform />
                <button
                  type="button"
                  className="row-delete"
                  title="移除音频"
                  onClick={() => setRoleForm({ ...roleForm, audioPath: '' })}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            ) : editorMedia.audios.map((asset, index) => (
              <div className="role-editor-audio-row" key={asset.id}>
                <button type="button" className="play-round"><Play size={14} /></button>
                <strong>{asset.name || `音频 ${index + 1}`}</strong>
                <em>{asset.duration_seconds ? `${Math.round(asset.duration_seconds)}s` : '--'}</em>
                <Waveform />
                <button
                  type="button"
                  className="row-delete"
                  title="移除音频"
                  onClick={() => askConfirm({
                    title: '移除音频',
                    body: `确认从当前角色中移除「${asset.name || '这条音频'}」吗？`,
                    confirmText: '移除',
                    onConfirm: () => removeRoleMedia(asset.id),
                  })}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            ))}
            {mode === 'create' && !editorMedia.audios.length ? (
              <p className="role-section-empty">暂无音频，导入音频后可作为角色音色素材。</p>
            ) : null}
            {mode !== 'create' && !editorMedia.audios.length ? (
              <p className="role-section-empty">暂无音频，导入音频后可作为角色音色素材。</p>
            ) : null}
          </div>
        </section>

        <section className="role-editor-resource">
          <div className="role-section-head">
            <h4>音色样本 / 默认配音（{editorMedia.audios[0] ? 1 : 0} 条）</h4>
          </div>
          {editorMedia.audios[0] ? (
            <div className="role-editor-voice-card">
              <button type="button" className="play-round"><Play size={14} /></button>
              <div>
                <strong>{editorMedia.audios[0].name || '默认音色'}</strong>
                <span>00:36 ｜ 采样率 48kHz ｜ 单声道</span>
              </div>
              <Waveform />
              <b>默认音色（当前）</b>
              <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
                <FileAudio size={14} /> 更换音色
              </button>
            </div>
          ) : (
            <div className="voice-sample-empty-card">
              <p>还没有设置默认音色，导入音频后会优先使用第一条音频。</p>
              <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
                <FileAudio size={14} /> 导入音频
              </button>
            </div>
          )}
        </section>
        <ImageModal src={previewSrc} alt={previewAlt} onClose={() => setPreviewSrc(null)} />
      </div>
    </form>
  );
}

function RoleEditModal({ mode, roleForm, setRoleForm, onSave, onCancel, chooseInitialRoleFile, askConfirm, deleteRole, selectedRole }) {
  const currentTags = splitCsv(roleForm.tags);
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onCancel}>
      <section className="role-edit-dialog" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
        <div className="role-edit-dialog-head">
          <h2>{mode === 'create' ? '新建角色' : '编辑角色信息'}</h2>
          <button type="button" className="icon-ghost" onClick={onCancel}><X size={16} /></button>
        </div>
        <form onSubmit={onSave} className="role-edit-form">
          <Field label="角色名" required>
            <div className="input-with-count">
              <input value={roleForm.name} maxLength={50} onChange={(e) => setRoleForm({ ...roleForm, name: e.target.value })} placeholder="输入角色名称" />
              <span className="field-count">{roleForm.name.length}/50</span>
            </div>
          </Field>
          <Field label="标签">
            <div className="tag-chip-list">
              {currentTags.map((tag) => (
                <button type="button" className="tag-chip" key={tag}
                  onClick={() => setRoleForm({ ...roleForm, tags: currentTags.filter((t) => t !== tag).join('，') })}
                >
                  {tag}<X size={13} />
                </button>
              ))}
              <button type="button" className="add-tag-chip"><Plus size={14} /> 添加标签 <ChevronDown size={14} /></button>
            </div>
          </Field>
          <Field label="别名">
            <div className="input-with-count">
              <input value={roleForm.aliases} onChange={(e) => setRoleForm({ ...roleForm, aliases: e.target.value })} placeholder="别名，逗号分隔" />
              <span className="field-count">{roleForm.aliases.length}/50</span>
            </div>
          </Field>
          <Field label="角色描述">
            <div className="prompt-editor">
              <textarea rows={3} value={roleForm.description} maxLength={1000}
                onChange={(e) => setRoleForm({ ...roleForm, description: e.target.value })}
                placeholder="描述角色外观、性格、音色或生成时需要保持的一致性。"
              />
              <span className="field-count">{roleForm.description.length}/1000</span>
            </div>
          </Field>
          {mode === 'create' ? (
            <Field label="参考图">
              <div className="file-picker-field">
                <input value={roleForm.imagePath} onChange={(e) => setRoleForm({ ...roleForm, imagePath: e.target.value })} placeholder="点击右侧选择或将图片拖入窗口" readOnly />
                <button type="button" onClick={() => chooseInitialRoleFile('image')}>选择图片</button>
              </div>
            </Field>
          ) : null}
          <div className="role-edit-form-actions">
            {mode === 'edit' && selectedRole ? (
              <button type="button" className="danger-outline"
                onClick={() => askConfirm({
                  title: '删除角色',
                  body: `确认删除「${selectedRole.name}」吗？已被任务引用的角色会被后端阻止删除。`,
                  confirmText: '删除',
                  onConfirm: () => { deleteRole(selectedRole.id); onCancel(); },
                })}
              >
                <Trash2 size={14} /> 删除角色
              </button>
            ) : null}
            <div className="role-edit-form-right">
              <button type="button" className="outline-button" onClick={onCancel}>取消</button>
              <button type="submit" className="gradient-button"><Save size={14} /> 保存</button>
            </div>
          </div>
        </form>
      </section>
    </div>
  );
}

function MediaImageManager({ images, dragActive, chooseFilesForSelectedRole, removeRoleMedia, renameAsset, askConfirm }) {
  const [previewSrc, setPreviewSrc] = useState(null);
  const [previewAlt, setPreviewAlt] = useState('');
  const [renamingId, setRenamingId] = useState(null);
  const [renameValue, setRenameValue] = useState('');

  const startRename = (asset) => {
    setRenamingId(asset.id);
    setRenameValue(asset.name || '');
  };

  const confirmRename = async (assetId) => {
    if (renameValue.trim() && renameAsset) {
      await renameAsset(assetId, renameValue.trim());
    }
    setRenamingId(null);
  };

  return (
    <section className="role-media-section">
      <div className="role-section-head">
        <div>
          <h3>参考图片管理</h3>
          <p>拖拽或选择文件导入，生成时会随角色自动匹配。</p>
        </div>
        <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
          <ImagePlus size={15} /> 添加图片
        </button>
      </div>
      <div className={`drop-zone role-drop-zone ${dragActive ? 'active' : ''}`}>
        <div>
          <strong>拖入角色图片或音频</strong>
          <p>支持 png、jpg、jpeg、webp、mp3、wav、m4a、aac，会复制到 App 缓存。</p>
        </div>
        <button type="button" onClick={chooseFilesForSelectedRole}>选择文件</button>
      </div>
      <div className="image-manager-grid">
        {images.map((asset, index) => (
          <article className="managed-image-card" key={asset.id}>
            <Thumb asset={asset} label={asset.name} onClick={() => { setPreviewSrc(convertFileSrc(asset.stored_path)); setPreviewAlt(asset.name); }} />
            <div>
              {renamingId === asset.id ? (
                <div className="thumb-rename-row">
                  <input value={renameValue} onChange={(e) => setRenameValue(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); confirmRename(asset.id); } if (e.key === 'Escape') setRenamingId(null); }} autoFocus />
                  <button type="button" onClick={() => confirmRename(asset.id)}>✓</button>
                  <button type="button" onClick={() => setRenamingId(null)}>×</button>
                </div>
              ) : (
                <strong onDoubleClick={() => startRename(asset)}>{asset.name || `参考图 ${index + 1}`}</strong>
              )}
              <span>{index === 0 ? '默认头像' : '参考图片'}</span>
            </div>
            <button type="button" className="thumb-action-btn" title="重命名" onClick={() => startRename(asset)}><Pencil size={10} /></button>
            <button
              type="button"
              className="managed-image-remove"
              title="移除图片"
              onClick={() => askConfirm({
                title: '移除图片',
                body: `确认从当前角色中移除「${asset.name || '这张图片'}」吗？未被其他任务引用时会同步清理缓存文件。`,
                confirmText: '移除',
                onConfirm: () => removeRoleMedia(asset.id),
              })}
            >
              <Trash2 size={14} />
            </button>
          </article>
        ))}
        {!images.length ? <p className="empty-cell">暂无参考图片。MVP 创建角色至少需要一张图片。</p> : null}
      </div>
      <ImageModal src={previewSrc} alt={previewAlt} onClose={() => setPreviewSrc(null)} />
    </section>
  );
}

function MediaAudioManager({ title, items, removeRoleMedia, askConfirm }) {
  return (
    <section className="role-media-section">
      <div className="role-section-head">
        <div>
          <h3>{title}</h3>
          <p>音频会作为角色音色素材，后续生成任务可随角色自动绑定。</p>
        </div>
      </div>
      <div className="audio-table">
        {items.map((asset, index) => (
          <article className="managed-audio-row" key={asset.id}>
            <button type="button" className="play-round"><Play size={14} /></button>
            <div>
              <strong>{asset.name || `音频 ${index + 1}`}</strong>
              <span>{asset.duration_seconds ? `${Math.round(asset.duration_seconds)} 秒` : '音频素材'}</span>
            </div>
            <Waveform />
            <button
              type="button"
              title="移除音频"
              className="row-delete"
              onClick={() => askConfirm({
                title: '移除音频',
                body: `确认从当前角色中移除「${asset.name || '这条音频'}」吗？`,
                confirmText: '移除',
                onConfirm: () => removeRoleMedia(asset.id),
              })}
            >
              <Trash2 size={14} />
            </button>
          </article>
        ))}
        {!items.length ? <p className="empty-cell">暂无音频素材。</p> : null}
      </div>
    </section>
  );
}

function VoiceSampleManager({ item, chooseFilesForSelectedRole, removeRoleMedia, askConfirm }) {
  return (
    <section className="role-media-section voice-sample-card">
      <div>
        <h3>默认音色</h3>
        <p>{item ? item.name : '还没有设置默认音色，导入音频后会优先使用第一条音频。'}</p>
      </div>
      <div className="voice-actions">
        {item ? (
          <button type="button" className="outline-button"><Play size={15} /> 试听</button>
        ) : (
          <button type="button" className="outline-button" onClick={chooseFilesForSelectedRole}>
            <FileAudio size={15} /> 导入音频
          </button>
        )}
        {item ? (
          <button
            type="button"
            className="danger-outline"
            onClick={() => askConfirm({
              title: '移除默认音色',
              body: `确认移除「${item.name || '默认音色'}」吗？`,
              confirmText: '移除',
              onConfirm: () => removeRoleMedia(item.id),
            })}
          >
            <Trash2 size={14} /> 移除
          </button>
        ) : null}
      </div>
    </section>
  );
}

function CreditModal({ credit, onClose, onRefresh }) {
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="confirm-dialog" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
        <div className="confirm-icon">
          <Coins size={18} />
        </div>
        <div>
          <h2>额度详情</h2>
          {credit.available ? (
            <div className="credit-detail-grid">
              {credit.total ? (
                <div className="credit-row">
                  <span className="credit-label">总额度</span>
                  <strong className="credit-value">{credit.total}</strong>
                </div>
              ) : null}
              {credit.used ? (
                <div className="credit-row">
                  <span className="credit-label">已使用</span>
                  <strong className="credit-value">{credit.used}</strong>
                </div>
              ) : null}
              {credit.remaining ? (
                <div className="credit-row">
                  <span className="credit-label">剩余</span>
                  <strong className="credit-value credit-remaining">{credit.remaining}</strong>
                </div>
              ) : null}
              {credit.raw_text ? (
                <details className="credit-raw-section">
                  <summary>原始输出</summary>
                  <pre className="credit-raw-text">{credit.raw_text}</pre>
                </details>
              ) : null}
            </div>
          ) : (
            <p className="error-text">无法获取额度信息，请确认 CLI 已登录。</p>
          )}
        </div>
        <footer>
          <button type="button" className="outline-button" onClick={onRefresh}>刷新额度</button>
          <button type="button" className="gradient-button" onClick={onClose}>关闭</button>
        </footer>
      </section>
    </div>
  );
}

function ConfirmDialog({ modal, onCancel, onConfirm }) {
  if (!modal) return null;
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onCancel}>
      <section className={`confirm-dialog ${modal.tone || ''}`} role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
        <div className="confirm-icon">
          <Trash2 size={18} />
        </div>
        <div>
          <h2>{modal.title || '确认操作'}</h2>
          <p>{modal.body || '这个操作确认后会立即生效。'}</p>
        </div>
        <footer>
          <button type="button" className="outline-button" onClick={onCancel}>{modal.cancelText || '取消'}</button>
          <button type="button" className={modal.tone === 'danger' ? 'danger-button' : 'gradient-button'} onClick={onConfirm}>
            {modal.confirmText || '确认'}
          </button>
        </footer>
      </section>
    </div>
  );
}


function EditIcon() {
  return <Pencil size={14} />;
}


// getRoleMedia, buildQueueStats, resolveDropTarget, removeTempImageFromForm,
// computeRoleAssetIdsOnSave, resolveRemoveMediaTarget are imported at top of file
// fileExt, isImagePath, isAudioPath, isSupportedRoleMedia, uniqueFilePaths, splitCsv are imported from media-utils.js

function splitCsv(value) {
  return splitCsvUtil(value);
}

function shortRoleId(value) {
  if (!value) return '-';
  return String(value).replace(/^role_/, '').slice(0, 8);
}

function uniqueValues(values) {
  return Array.from(new Set((values || []).filter(Boolean)));
}

function sameStringArray(a = [], b = []) {
  if ((a || []).length !== (b || []).length) return false;
  return (a || []).every((value, index) => value === (b || [])[index]);
}

function updateTaskParams(setTaskForm, patch) {
  setTaskForm((current) => ({ ...current, params: { ...current.params, ...patch } }));
}

function formatDate(value) {
  if (!value) return '';
  const time = new Date(value);
  if (Number.isNaN(time.getTime())) return value;
  return time.toLocaleString();
}

function formatDatePart(value, part) {
  if (!value) return '-';
  const time = new Date(value);
  if (Number.isNaN(time.getTime())) return value;
  if (part === 'date') return time.toLocaleDateString();
  if (part === 'time') return time.toLocaleTimeString();
  return time.toLocaleString();
}

function formatDateInputValue(date) {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

function formatCountdown(value) {
  if (!value) return '-';
  const target = new Date(value);
  if (Number.isNaN(target.getTime())) return value;
  const diff = target.getTime() - Date.now();
  if (diff <= 0) return '即将执行';
  const minutes = Math.floor(diff / 60000);
  if (minutes < 60) return `${minutes} 分钟后`;
  const hours = Math.floor(minutes / 60);
  const remainMinutes = minutes % 60;
  if (hours < 24) return `${hours} 小时 ${remainMinutes} 分钟后`;
  const days = Math.floor(hours / 24);
  return `${days} 天 ${hours % 24} 小时后`;
}

function statusLabel(status) {
  const labels = {
    draft: '草稿',
    queued: '排队中',
    scheduled: '预定中',
    overdue_scheduled: '已到期，等待补偿提交',
    submitting: '提交中',
    submitted: '已提交',
    querying: '查询中',
    retry_wait: '等待重试',
    succeeded: '成功',
    failed: '失败',
    blocked: '阻断',
    paused: '已暂停',
  };
  return labels[status] || status;
}

createRoot(document.getElementById('root')).render(<App />);
