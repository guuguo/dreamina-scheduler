import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
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
  normalizeImageModelSettingsForm,
  patchImageModelConfig,
  mergeSettingsFormOnStateRefresh,
  findLatestSuccessfulResultPath,
} from './app-logic.js';
import { buildMentionItems } from './mention-utils.js';
import {
  createEmptyImageGenForm,
  filterMentionItemsForImageGen,
  addReferenceAsset,
  removeReferenceAsset,
  applyMentionRefsToImageGenForm,
  shouldSuppressImageContextMenu,
  IMAGEGEN_MAX_REFERENCES,
} from './imagegen-utils.js';
import PromptMentionEditor from './components/PromptMentionEditor.jsx';
import { applyEditorUpdate, applyMentionRefsToTaskForm } from './prompt-editor-utils.js';
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
  buildSaveTaskDraftButtonState,
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
  FileAudio,
  FolderOpen,
  Gauge,
  Grid,
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
  deriveTaskDispatchInfo,
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
import { StatusPill } from './components/ui/StatusPill.jsx';
import { ToggleSwitch } from './components/ui/ToggleSwitch.jsx';
import { NumberedSection } from './components/ui/NumberedSection.jsx';
import { PasswordInput } from './components/ui/PasswordInput.jsx';
import { CopyableInput } from './components/ui/CopyableInput.jsx';
import { StatCard } from './components/ui/StatCard.jsx';
import { SearchBox } from './components/ui/SearchBox.jsx';
import { FilterSelect } from './components/ui/FilterSelect.jsx';
import { InlineCopyButton } from './components/ui/InlineCopyButton.jsx';
import { Pager } from './components/ui/Pager.jsx';
import { resolveMediaSrc } from './media-src.js';
import {
  normalizeLogEntry,
  deriveLogStats,
  deriveCategoryCounts,
  filterLogs,
  paginateLogs,
  deriveLogAssociations,
  buildLogExportPayload,
  findDefaultSelectedLogId,
  getQuickLocationFilters,
  deriveTaskOptions,
  LOG_LEVELS,
  LOG_SOURCES,
  LEVEL_VARIANT_MAP,
  SOURCE_LABEL_MAP,
  CATEGORY_LABEL_MAP,
  SIDEBAR_CATEGORIES,
  QUICK_LOCATIONS,
  TIME_RANGE_OPTIONS,
  DEFAULT_PAGE_SIZE,
} from './log-view-utils.js';
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
const defaultImageModelConfig = {
  id: 'default-image-openai',
  name: 'OpenAI 图片默认',
  api_key: '',
  base_url: 'https://api.openai.com/v1',
  model: 'gpt-image-1',
};
const views = [
  { id: 'roles', label: '角色库', icon: User },
  { id: 'queue', label: '任务中心', icon: ListChecks },
  { id: 'imagegen', label: '生图', icon: Image },
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
    image_model_configs: [defaultImageModelConfig],
    active_image_model_id: defaultImageModelConfig.id,
    image_model_config: defaultImageModelConfig,
    prevent_sleep: true,
  },
  assets: [],
  roles: [],
  tasks: [],
  logs: [],
};

const tauriRuntimeAvailable = typeof window !== 'undefined' && Boolean(window.__TAURI_INTERNALS__?.metadata?.currentWindow);

function App() {
  const [activeView, setActiveView] = useState('queue');
  const [state, setState] = useState(emptyState);
  const [cli, setCli] = useState({ available: false, path: '', message: '等待检测', version: '', commit: '', build_time: '', version_raw: '', installed_release_version: '', installed_release_date: '', installed_release_notes: '', installed_release_path: '' });
  const [cliUpdate, setCliUpdate] = useState(null);
  const [cliUpdateStatus, setCliUpdateStatus] = useState('idle'); // idle | checking | success | failed
  const [hostPlatform, setHostPlatform] = useState({ label: 'Desktop' });
  const [feedback, setFeedback] = useState('');
  useEffect(() => {
    if (!feedback) return;
    const t = setTimeout(() => setFeedback(''), 3000);
    return () => clearTimeout(t);
  }, [feedback]);
  useEffect(() => {
    const blockLinkNavigation = (event) => {
      const anchor = event.target?.closest?.('a[href]');
      if (!anchor) return;
      const href = anchor.getAttribute('href') || '';
      if (!href || href.startsWith('#')) return;
      event.preventDefault();
      event.stopPropagation();
      navigator.clipboard?.writeText(anchor.href)
        .then(() => setFeedback('已复制链接，并阻止内置浏览器打开'))
        .catch(() => setFeedback('已阻止内置浏览器打开'));
    };
    document.addEventListener('click', blockLinkNavigation, true);
    return () => document.removeEventListener('click', blockLinkNavigation, true);
  }, []);
  const [pendingTaskOps, setPendingTaskOps] = useState({});
  const [pendingExecutionOps, setPendingExecutionOps] = useState({});
  const tickLockRef = useRef(false);
  const refreshStateRef = useRef(null);
  // 记录上次拉取的状态签名，空闲时签名不变即跳过整份 get_app_state，省去大文件读解析。
  const lastStateSignatureRef = useRef('');
  const [lastTickAt, setLastTickAt] = useState(null);
  const [selectedTaskId, setSelectedTaskId] = useState('');
  const [selectedRoleId, setSelectedRoleId] = useState('');
  const [roleSearchQuery, setRoleSearchQuery] = useState('');
  const [roleActiveTab, setRoleActiveTab] = useState('all');
  const [roleViewMode, setRoleViewMode] = useState('grid');
  const [roleEditor, setRoleEditor] = useState(null);
  const [dragActive, setDragActive] = useState(false);
  const [confirmModal, setConfirmModal] = useState(null);
  const [appVersion, setAppVersion] = useState('');
  useEffect(() => {
    if (!tauriRuntimeAvailable) return;
    getVersion().then(setAppVersion).catch(() => {});
  }, []);
  const [creditInfo, setCreditInfo] = useState({ available: false, total: '', used: '', remaining: '', raw_text: '' });
  const [creditModalOpen, setCreditModalOpen] = useState(false);
  const [settingsForm, setSettingsForm] = useState(emptyState.settings);
  const [settingsDirty, setSettingsDirty] = useState(false);
  const settingsDirtyRef = useRef(false);
  const updateSettingsForm = (next) => {
    settingsDirtyRef.current = true;
    setSettingsDirty(true);
    setSettingsForm(next);
  };
  const roleForm = roleEditor?.form || createEmptyRoleForm();
  const setRoleForm = (patch) => {
    setRoleEditor((current) => patchRoleEditorForm(current, patch));
  };
  const [taskForm, setTaskForm] = useState(() => createEmptyTaskForm());
  const [editingTaskId, setEditingTaskId] = useState(null);
  const [savingTaskDraft, setSavingTaskDraft] = useState(false);
  const [savingTaskDraftPhase, setSavingTaskDraftPhase] = useState('');
  // 图片生成预览项提升到 App 层，避免切 tab 后 ImageGenView 卸载导致状态丢失
  // 历史记录从 state.imagegen_history 派生（后端持久化）
  const [imageGenPreview, setImageGenPreview] = useState(null);
  const dropContextRef = useRef({ activeView, selectedRoleId, roleEditor });

  async function refreshState(options = {}) {
    try {
      const next = await invoke('get_app_state');
      setState(next);
      // 记录本次状态对应的签名，供空闲轮询比对（失败不影响刷新本身）。
      try {
        lastStateSignatureRef.current = await invoke('get_state_signature');
      } catch {
        // 忽略：拿不到签名时下次轮询会照常全量刷新
      }
      const latestTick = [...(next.logs || [])]
        .reverse()
        .find((log) => log.category === 'scheduler_tick' && (log.eventType || log.event_type) === 'tick');
      if (latestTick?.timestamp) setLastTickAt(new Date(latestTick.timestamp));
      setSettingsForm((current) => mergeSettingsFormOnStateRefresh({
        activeView,
        settingsDirty: options.forceSettingsSync ? false : settingsDirtyRef.current,
        currentSettingsForm: current,
        incomingSettings: next.settings,
        emptySettings: emptyState.settings,
      }));
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

  async function checkCliUpdate() {
    setCliUpdateStatus('checking');
    try {
      const result = await invoke('check_dreamina_cli_update');
      setCliUpdate(result);
      setCliUpdateStatus('success');
      setFeedback(result.message || 'CLI 更新检查完成');
    } catch (error) {
      setCliUpdateStatus('failed');
      setFeedback(`CLI 更新检查失败：${String(error)}`);
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
    if (!tauriRuntimeAvailable) return undefined;
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

  // 后端 Rust 线程负责调度；前端只刷新状态用于展示最新 tick 和任务状态。
  // 空闲优化：先取廉价签名（仅 stat 文件，不读整份状态），签名未变则跳过 get_app_state，
  // 避免空闲时每 30s 读+解析整份大状态文件。
  useEffect(() => {
    const timer = window.setInterval(async () => {
      try {
        const sig = await invoke('get_state_signature');
        if (sig === lastStateSignatureRef.current) return; // 无变化，跳过整份刷新
      } catch {
        // 取签名失败则退化为照常全量刷新
      }
      refreshStateRef.current?.();
    }, 30000);
    return () => {
      window.clearInterval(timer);
    };
  }, []);

  // 记录前端生命周期，辅助判断是进程退出、WebView 暂停，还是后端调度仍在运行。
  useEffect(() => {
    if (!tauriRuntimeAvailable) return undefined;
    const logLifecycle = (eventType, message) => {
      invoke('record_lifecycle_event_command', {
        eventType,
        message,
        detail: `visibility=${document.visibilityState}`,
      }).catch(() => {});
    };
    logLifecycle('frontend_ready', '前端页面就绪');
    const handleFocus = () => {
      logLifecycle('frontend_focus', '窗口聚焦');
      refreshStateRef.current?.();
    };
    const handleBlur = () => logLifecycle('frontend_blur', '窗口失焦');
    const handleVisibility = () => {
      if (document.hidden) {
        logLifecycle('frontend_hidden', '页面隐藏');
      } else {
        logLifecycle('frontend_visible', '页面恢复可见');
        refreshStateRef.current?.();
      }
    };
    const handlePageHide = () => logLifecycle('frontend_pagehide', '页面卸载或进入缓存');
    window.addEventListener('focus', handleFocus);
    window.addEventListener('blur', handleBlur);
    window.addEventListener('pagehide', handlePageHide);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      window.removeEventListener('focus', handleFocus);
      window.removeEventListener('blur', handleBlur);
      window.removeEventListener('pagehide', handlePageHide);
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
  const latestSuccessfulResultPath = useMemo(
    () => findLatestSuccessfulResultPath(state.tasks),
    [state.tasks],
  );

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
    if (savingTaskDraft) return;
    let draft = {
      ...taskForm,
      image_asset_ids: taskForm.image_asset_ids || [],
      audio_asset_ids: taskForm.audio_asset_ids || [],
      scheduled_at: null,
    };
    setSavingTaskDraft(true);
    setSavingTaskDraftPhase('');
    try {
      if (!String(draft.title || '').trim()) {
        setSavingTaskDraftPhase('title');
        const generatedTitle = await generateTaskTitle(draft.prompt);
        if (generatedTitle) draft = { ...draft, title: generatedTitle };
      }
      setSavingTaskDraftPhase('saving');
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
    } finally {
      setSavingTaskDraft(false);
      setSavingTaskDraftPhase('');
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
      await checkCli();
      await checkCliUpdate();
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

  async function importTempImages() {
    const selected = await open({
      multiple: true,
      filters: [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
    });
    if (!selected) return [];
    const paths = Array.isArray(selected) ? selected : [selected];
    const imported = [];
    for (const path of paths) {
      // eslint-disable-next-line no-await-in-loop
      const name = path.split('/').pop().replace(/\.[^.]+$/, '') || '临时图片';
      const asset = await invoke('import_temp_image_command', { input: { path, name } });
      imported.push(asset);
    }
    await refreshState();
    return imported;
  }

  async function addTempImage() {
    try {
      const imported = await importTempImages();
      if (!imported.length) return;
      setTaskForm((current) => ({
        ...current,
        temp_image_paths: [...current.temp_image_paths, ...imported.map((asset) => asset.stored_path)].slice(0, 9),
        temp_image_asset_ids: [...(current.temp_image_asset_ids || []), ...imported.map((asset) => asset.id)].slice(0, 9),
        image_asset_ids: uniqueValues([...(current.image_asset_ids || []), ...imported.map((asset) => asset.id)]).slice(0, 9),
      }));
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
      const imageSettings = normalizeImageModelSettingsForm(settingsForm);
      const savedSettings = await invoke('update_settings_command', {
        input: {
          concurrency_limit_policy: 'SilentRetry',
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
          image_model_configs: imageSettings.image_model_configs,
          active_image_model_id: imageSettings.active_image_model_id,
          prevent_sleep: settingsForm.prevent_sleep ?? true,
          image_model_config: imageSettings.image_model_config || null,
        },
      });
      setFeedback('设置已保存');
      settingsDirtyRef.current = false;
      setSettingsDirty(false);
      setSettingsForm(normalizeImageModelSettingsForm(savedSettings || emptyState.settings));
      await refreshState({ forceSettingsSync: true });
    } catch (error) {
      setFeedback(String(error));
    }
  }

  async function openLatestOutputFolder() {
    if (!latestSuccessfulResultPath) return;
    try {
      await invoke('open_result_dir_command', { path: latestSuccessfulResultPath });
      setFeedback('已打开最近产出目录');
    } catch (error) {
      setFeedback(String(error));
    }
  }

  function startWindowDrag(event) {
    if (!tauriRuntimeAvailable) return;
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
                title={item.label}
              >
                <Icon size={17} />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>
        <button
          type="button"
          className="sidebar-output-button"
          disabled={!latestSuccessfulResultPath}
          onClick={openLatestOutputFolder}
          title={latestSuccessfulResultPath ? `打开最近产出目录：${latestSuccessfulResultPath}` : '暂无成功视频'}
        >
          <FolderOpen size={17} />
          <span>产出文件夹</span>
        </button>
        {appVersion ? <span className="version">v{appVersion}</span> : null}
      </aside>

      <section className="app-window">
        <header className="window-bar" data-tauri-drag-region onMouseDown={startWindowDrag}>
          <div className="window-title" data-tauri-drag-region onMouseDown={startWindowDrag}>
            <span className="traffic-spacer" data-tauri-drag-region />
            <div className="brand-mark small" />
            <strong>Dreamina Scheduler</strong>
            <StatusPill variant={cli.available ? 'ok' : 'bad'}>
              <CheckCircle2 size={13} />
              {cli.available ? `dreamina CLI ${cli.version || '已连接'}` : cli.message}
            </StatusPill>
            {cli.available ? (
              <StatusPill
                variant={creditInfo.available ? 'ok' : 'neutral'}
                style={{ cursor: 'pointer' }}
                onClick={() => { refreshCredit(); setCreditModalOpen(true); }}
                title="点击查看额度详情"
              >
                <Coins size={13} />
                {creditInfo.available
                  ? (creditInfo.remaining ? `剩余 ${creditInfo.remaining}` : `总额 ${creditInfo.total}`)
                  : '额度未知'}
              </StatusPill>
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
            savingTaskDraft={savingTaskDraft}
            savingTaskDraftPhase={savingTaskDraftPhase}
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
        {activeView === 'imagegen' ? (
          <ImageGenView
            settingsForm={settingsForm}
            history={state.imagegen_history || []}
            previewItem={imageGenPreview}
            setPreviewItem={setImageGenPreview}
            state={state}
            assetById={assetById}
            importTempImages={importTempImages}
            pasteClipboardImage={pasteClipboardImage}
            pasteSystemClipboardImage={pasteSystemClipboardImage}
            refreshState={refreshState}
            setFeedback={setFeedback}
          />
        ) : null}
        {activeView === 'logs' ? <LogsView logs={state.logs} tasks={state.tasks} settings={state.settings} clearLogs={clearLogs} setActiveView={setActiveView} setSelectedTaskId={setSelectedTaskId} /> : null}
        {activeView === 'settings' ? (
          <SettingsView
            cli={cli}
            settingsForm={settingsForm}
            setSettingsForm={updateSettingsForm}
            checkCli={checkCli}
            checkCliUpdate={checkCliUpdate}
            cliUpdate={cliUpdate}
            cliUpdateStatus={cliUpdateStatus}
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

function CreateTaskView({ state, assetById, taskForm, setTaskForm, setActiveView, saveTaskDraft, editingTaskId, cli, addTempImage, removeTempImage, previewCommand, generateTaskTitle, savingTaskDraft, savingTaskDraftPhase, pasteClipboardImage, pasteSystemClipboardImage }) {
  const [previewSrc, setPreviewSrc] = useState(null);
  const [previewAlt, setPreviewAlt] = useState('');
  const [audioPreviewAsset, setAudioPreviewAsset] = useState(null);
  const [aiModal, setAiModal] = useState({ open: false, label: '', description: '', error: '' });
  const openImagePreview = (path, alt) => {
    if (!path) return;
    setPreviewSrc(resolveMediaSrc(path));
    setPreviewAlt(alt || '');
  };

  // Extract mention labels from prompt_doc for displayName alignment
  const mentionAttrsByAssetId = useMemo(() => {
    const doc = taskForm.prompt_doc;
    if (!doc) return new Map();
    const map = new Map();
    const walk = (node) => {
      if (node.type === 'mention' && node.attrs?.assetId) {
        if (!map.has(node.attrs.assetId)) {
          map.set(node.attrs.assetId, { label: node.attrs.label, type: node.attrs.type });
        }
      }
      (node.content || []).forEach(walk);
    };
    walk(doc);
    return map;
  }, [taskForm.prompt_doc]);

  const boundResources = useMemo(() => {
    const resources = getTaskHitResources({
      image_asset_ids: taskForm.image_asset_ids || [],
      temp_image_asset_ids: taskForm.temp_image_asset_ids || [],
      audio_asset_ids: taskForm.audio_asset_ids || [],
    }, assetById);
    // Override displayName with mention label from prompt_doc when available
    return resources.map((item) => ({
      ...item,
      displayName: mentionAttrsByAssetId.get(item.asset.id)?.label || item.displayName,
    }));
  }, [
    assetById,
    taskForm.image_asset_ids,
    taskForm.temp_image_asset_ids,
    taskForm.audio_asset_ids,
    mentionAttrsByAssetId,
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
        && sameStringArray(current.temp_image_asset_ids, next.temp_image_asset_ids)
        && sameStringArray(current.audio_asset_ids, next.audio_asset_ids)
      ) {
        return current;
      }
      return next;
    });
  }, [mentionItems, setTaskForm, taskForm.prompt]);

  // Doc-first editor update: persists prompt_doc alongside derived prompt + mention bindings.
  const handleEditorUpdate = useCallback(({ plainText, refs, doc }) => {
    setTaskForm((current) => {
      return applyEditorUpdate(current, { plainText, refs, doc });
    });
  }, []);

  // Mention click: open image preview or audio playback
  const handleMentionClick = useCallback(({ type, assetId }) => {
    if (!assetId) return;
    const asset = assetById.get(assetId);
    if (!asset?.stored_path) return;
    if (type === 'image' || type === 'temp_image') {
      openImagePreview(asset.stored_path, asset.name);
    } else if (type === 'audio') {
      setAudioPreviewAsset(asset);
    }
  }, [assetById, openImagePreview, setAudioPreviewAsset]);

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

  const handlePasteAudioForEditor = useCallback(async (file) => {
    const asset = await pasteClipboardImage(file);
    setTaskForm((current) => ({
      ...current,
      audio_asset_ids: [...(current.audio_asset_ids || []), asset.id],
    }));
    return asset;
  }, [pasteClipboardImage]);

  const canSaveDraft = canSaveTaskDraft(taskForm);
  const canApplyPreset = canApplyCreateTaskPreset(taskForm);
  const isEditingTask = Boolean(editingTaskId);
  const saveButton = buildSaveTaskDraftButtonState({
    canSaveDraft,
    isEditingTask,
    savingTaskDraft,
    savingTaskDraftPhase,
  });
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
              <button type="button" className="gradient-button" disabled={saveButton.disabled} onClick={saveTaskDraft}>
                {saveButton.icon === 'loader' ? <Loader2 size={14} className="spin" /> : <Plus size={14} />} {saveButton.label}
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
                    <em>自动匹配可用的 @图片、角色图片和音频素材</em>
                  </button>
                ) : null}
                <PromptMentionEditor
                  value={taskForm.prompt}
                  promptDoc={taskForm.prompt_doc}
                  mentionItems={mentionItems}
                  maxLength={TASK_PROMPT_MAX_LENGTH}
                  placeholder="@女主日常服 在海边漫步，阳光照在身上，海浪轻轻打沙滩，微风拂动长发，画面唯美治愈。"
                  onUpdate={handleEditorUpdate}
                  onPasteImage={handlePasteImageForEditor}
                  onPasteSystemImage={handlePasteSystemImageForEditor}
                  onPasteAudio={handlePasteAudioForEditor}
                  onMentionClick={handleMentionClick}
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
                      <button type="button" className="temp-image-save-role" title="保存为角色" onClick={() => handleSaveAsRole(path)}><User size={12} /></button>
                    </div>
                  ))}
                </div>
                <div className="info-strip">
                  <Image size={13} />
                  可在提示词中通过 @图片1 等方式引用临时图片。
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
                  {boundRoleImages.map((item) => (
                    <Thumb
                      key={item.asset.id}
                      asset={item.asset}
                      label={item.displayName}
                      subLabel={item.subName !== item.displayName ? item.subName : null}
                      onClick={() => openImagePreview(item.asset.stored_path, item.displayName)}
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
                  {boundTempImages.map((item) => (
                    <Thumb
                      key={item.asset.id}
                      asset={item.asset}
                      label={item.displayName}
                      subLabel={item.subName !== item.displayName ? item.subName : null}
                      onClick={() => openImagePreview(item.asset.stored_path, item.displayName)}
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

  const handleSaveAsRole = (imagePath, suggestedName = '') => {
    setActiveView('roles');
    const editor = createRoleEditor('create');
    if (imagePath) editor.form.imagePath = imagePath;
    if (suggestedName) editor.form.name = suggestedName;
    setRoleEditor(editor);
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

  const dispatchInfo = useMemo(() => deriveTaskDispatchInfo(task), [task]);
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
          <img src={resolveMediaSrc(thumbPath)} alt="" className="qc-thumb-img" />
        ) : (
          <div className="qc-thumb-placeholder"><Image size={14} /></div>
        )}
      </div>
      <div className="qc-task-info">
        <span className="qc-task-title">{task.title || '未命名任务'}</span>
        <span className="qc-task-sub">{task.params?.model_version || ''}{task.params?.ratio ? ` · ${task.params.ratio}` : ''}</span>
        {['queued', 'scheduled', 'retry_wait'].includes(task.status) ? (
          <span className="qc-task-dispatch-sub">{dispatchInfo.compactText}</span>
        ) : null}
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

function isConcurrencyLimitMessage(message = '') {
  const original = String(message || '');
  const text = original.toLowerCase();
  return text.includes('exceedconcurrencylimit')
    || text.includes('concurrencylimit')
    || text.includes('ret=1310')
    || text.includes('ret = 1310')
    || original.includes('并发上限')
    || original.includes('并发限制');
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
  const plannedCandidateCount = Math.max(1, Number(selectedTask?.planned_submit_count || 1));
  const successfulCandidateCount = (selectedTask?.execution_records || [])
    .filter((record) => record.status === 'succeeded').length;
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
  const concurrencyCooldownUntil = useMemo(() => {
    const now = Date.now();
    return tasks
      .filter((item) => item.status === 'retry_wait'
        && isConcurrencyLimitMessage(item.last_error)
        && item.next_run_at
        && new Date(item.next_run_at).getTime() > now)
      .map((item) => item.next_run_at)
      .sort()[0] || '';
  }, [tasks]);
  const dispatchInfo = useMemo(
    () => deriveTaskDispatchInfo(selectedTask, { concurrencyCooldownUntil }),
    [selectedTask, concurrencyCooldownUntil]
  );
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
  const openQueueMode = () => {
    const taskIds = selectedBatchTasks.filter(canScheduleTask).map((task) => task.id);
    if (!taskIds.length) {
      setFeedback('请先选择可排队的任务');
      return;
    }
    setScheduleModal({ mode: 'queue', taskIds, title: `排队模式 ${taskIds.length} 个任务` });
  };
  const applySchedulePlan = async ({ scheduledAt, intervalMinutes, plannedSubmitCount }) => {
    if (!scheduleModal?.taskIds?.length) return;
    try {
      const submitCount = Math.max(1, Math.min(10, Number(plannedSubmitCount || 1)));
      if (scheduleModal.mode === 'batch') {
        const startAt = scheduledAt || new Date().toISOString();
        const plan = buildBatchSchedulePlan(scheduleModal.taskIds, { startAt, intervalMinutes });
        for (const item of plan) {
          await invoke('set_task_planned_submit_count_command', {
            taskId: item.taskId,
            plannedSubmitCount: submitCount,
          });
          await rescheduleTask(item.taskId, item.scheduledAt);
        }
        setFeedback(`已排布：${formatSchedulePlanSummary(plan)}`);
        setSelectedBatchIds([]);
      } else if (scheduleModal.mode === 'queue') {
        for (const taskId of scheduleModal.taskIds) {
          await invoke('set_task_planned_submit_count_command', {
            taskId,
            plannedSubmitCount: submitCount,
          });
          await rescheduleTask(taskId, scheduledAt || '');
        }
        setFeedback(scheduledAt
          ? `已设置排队：${scheduleModal.taskIds.length} 个任务 · 起始 ${formatDate(scheduledAt)}`
          : `已设为立即排队：${scheduleModal.taskIds.length} 个任务`);
        setSelectedBatchIds([]);
      } else if (scheduleModal.mode === 'prepare') {
        await invoke('set_task_planned_submit_count_command', {
          taskId: scheduleModal.taskIds[0],
          plannedSubmitCount: submitCount,
        });
        const operation = resolvePrepareGenerateOperation({ scheduledAt });
        if (operation.type === 'submit') {
          await submitTask(scheduleModal.taskIds[0]);
          setFeedback('已开始生成');
        } else {
          await rescheduleTask(scheduleModal.taskIds[0], operation.scheduledAt);
          setFeedback('已设置定时生成');
        }
      } else {
        await invoke('set_task_planned_submit_count_command', {
          taskId: scheduleModal.taskIds[0],
          plannedSubmitCount: submitCount,
        });
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
        <button type="button" className="qc-btn" onClick={openQueueMode} disabled={!selectedBatchTasks.length}>
          <ListChecks size={13} /> 排队模式{selectedBatchTasks.length ? `（${selectedBatchTasks.length}）` : ''}
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
                  <span className="qc-meta-label">成功候选</span>
                  <span className="qc-meta-value">{successfulCandidateCount} / {plannedCandidateCount}</span>
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
                  {['queued', 'scheduled', 'retry_wait'].includes(selectedTask.status) ? (
                    <>
                      <span className="qc-meta-label">提交尝试</span>
                      <span className="qc-meta-value">{dispatchInfo.attemptCount} 次 / 目标 {dispatchInfo.plannedSubmitCount} 个候选</span>
                      <span className="qc-meta-label">{dispatchInfo.nextLabel}</span>
                      <span className="qc-meta-value">
                        {dispatchInfo.nextAt ? formatDate(dispatchInfo.nextAt) : dispatchInfo.nextText}
                      </span>
                      <span className="qc-meta-label">等待原因</span>
                      <span className="qc-meta-value">{dispatchInfo.reason || '-'}</span>
                    </>
                  ) : null}
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
                        <Copy size={13} />
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
                          <img src={resolveMediaSrc(asset.stored_path)} alt="" className="qc-resource-thumb" />
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
                          <button type="button" className="icon-ghost mini" title="复制链接"
                            onClick={() => navigator.clipboard?.writeText(item.value).then(() => setFeedback('已复制结果链接')).catch(() => setFeedback('复制失败'))}>
                            <Copy size={12} />
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
                                <button type="button" className="icon-ghost mini" title="复制链接"
                                  onClick={(event) => { event.stopPropagation(); navigator.clipboard?.writeText(u).then(() => setFeedback('已复制结果链接')).catch(() => setFeedback('复制失败')); }}>
                                  <Copy size={12} />
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
                      <>
                        <div className="qc-monitor-meta-row">
                          <span>提交尝试</span>
                          <span>{dispatchInfo.attemptCount} 次</span>
                        </div>
                        <div className="qc-monitor-meta-row">
                          <span>并发等待</span>
                          <span>{dispatchInfo.concurrencyRetryText}</span>
                        </div>
                        {['queued', 'scheduled', 'retry_wait'].includes(selectedTask.status) ? (
                          <div className="qc-monitor-meta-row">
                            <span>{dispatchInfo.nextLabel}</span>
                            <span>{dispatchInfo.nextAt ? formatDatePart(dispatchInfo.nextAt, 'time') : dispatchInfo.nextText}</span>
                          </div>
                        ) : null}
                      </>
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
                  <span>等待条件</span><span>远端并发已满</span>
                  <span>处理方式</span><span>持续静默等待并重试</span>
                  <span>停止条件</span><span>提交成功或手动停止任务</span>
                  <span>临时上传错误</span><span>最多自动重试 3 次</span>
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
                  <span className="qc-health retry"><RefreshCcw size={12} /> {dispatchInfo.reason || '等待重试'}</span>
                ) : executionView?.status === 'queued' ? (
                  <span className="qc-health idle"><Clock3 size={12} /> {dispatchInfo.reason || '等待调度器空闲'}</span>
                ) : executionView?.status === 'scheduled' ? (
                  <span className="qc-health idle"><Clock3 size={12} /> {dispatchInfo.reason || '等待预定开始时间'}</span>
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
          src={resolveMediaSrc(resourcePreview.asset.stored_path)}
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


function LogsView({ logs, tasks, settings, clearLogs, setActiveView, setSelectedTaskId }) {
  // ── 规范化 ──
  const normalized = useMemo(() => (logs || []).map(normalizeLogEntry), [logs]);

  // ── UI state ──
  const [search, setSearch] = useState('');
  const [levelFilter, setLevelFilter] = useState('');
  const [sourceFilter, setSourceFilter] = useState('');
  const [taskFilter, setTaskFilter] = useState('');
  const [timeRange, setTimeRange] = useState('all');
  const [category, setCategory] = useState('all');
  const [selectedLogId, setSelectedLogId] = useState(null);
  const [autoRefresh, setAutoRefresh] = useState(false);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);

  // ── 派生 ──
  const stats = useMemo(() => deriveLogStats(normalized, settings?.log_retention_count || 500), [normalized, settings]);
  const catCounts = useMemo(() => deriveCategoryCounts(normalized), [normalized]);
  const filtered = useMemo(() => filterLogs(normalized, { search, level: levelFilter, source: sourceFilter, taskId: taskFilter, timeRange, category }), [normalized, search, levelFilter, sourceFilter, taskFilter, timeRange, category]);
  const paginated = useMemo(() => paginateLogs(filtered, page, pageSize), [filtered, page, pageSize]);
  const selectedLog = useMemo(() => normalized.find((l) => l.id === selectedLogId) || null, [normalized, selectedLogId]);
  const associations = useMemo(() => deriveLogAssociations(normalized, selectedLog), [normalized, selectedLog]);
  const taskOptions = useMemo(() => deriveTaskOptions(normalized), [normalized]);

  // 默认选中
  useEffect(() => {
    if (!selectedLogId && normalized.length) {
      setSelectedLogId(findDefaultSelectedLogId(normalized));
    }
  }, [normalized, selectedLogId]);

  // 筛选变化时重置分页
  useEffect(() => { setPage(1); }, [search, levelFilter, sourceFilter, taskFilter, timeRange, category]);

  // 自动刷新
  useEffect(() => {
    if (!autoRefresh) return;
    const id = setInterval(() => { /* 依赖现有 refreshState 机制 */ }, 3000);
    return () => clearInterval(id);
  }, [autoRefresh]);

  // ── 操作 ──
  function handleExport(fmt) {
    const content = buildLogExportPayload(filtered, fmt);
    const blob = new Blob([content], { type: fmt === 'json' ? 'application/json' : 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `logs.${fmt}`;
    a.click();
    URL.revokeObjectURL(url);
  }

  function handleCopyLog() {
    if (!selectedLog) return;
    const text = buildLogExportPayload([selectedLog], 'text');
    navigator.clipboard.writeText(text);
  }

  function handleLocateTask() {
    if (!selectedLog?.taskId) return;
    const task = tasks?.find((t) => t.id === selectedLog.taskId);
    if (task) {
      setSelectedTaskId(task.id);
      setActiveView('queue');
    }
  }

  function handleViewContext() {
    if (!selectedLog?.taskId && !selectedLog?.submitId) return;
    if (selectedLog.taskId) setTaskFilter(selectedLog.taskId);
  }

  function handleClearFilters() {
    setSearch(''); setLevelFilter(''); setSourceFilter(''); setTaskFilter(''); setTimeRange('all'); setCategory('all');
  }

  function handleQuickLocation(key) {
    const f = getQuickLocationFilters(key);
    if (f.level) setLevelFilter(f.level);
    if (f.timeRange) setTimeRange(f.timeRange);
  }

  // ── 级别/来源选项 ──
  const levelOptions = [{ key: '', label: '全部级别' }, ...LOG_LEVELS.map((l) => ({ key: l, label: l.toUpperCase() }))];
  const sourceOptions = [{ key: '', label: '全部来源' }, ...LOG_SOURCES.map((s) => ({ key: s, label: SOURCE_LABEL_MAP[s] || s }))];
  const taskFilterOptions = [{ key: '', label: '全部任务' }, ...taskOptions.map((t) => ({ key: t.id, label: t.title }))];

  return (
    <div className="log-center">
      {/* 标题区 */}
      <div className="log-center__header">
        <div>
          <h2 className="log-center__title">日志中心</h2>
          <p className="log-center__subtitle">查看任务执行日志、CLI 输出、错误信息与系统事件</p>
        </div>
        <div className="log-center__header-actions">
          <button type="button" className="outline-button" onClick={clearLogs}>清空日志</button>
        </div>
      </div>

      {/* 统计卡片 */}
      <div className="log-center__stats">
        <StatCard icon="List" title="今日日志" value={stats.today} tone="info" />
        <StatCard icon="AlertCircle" title="错误" value={stats.errors} tone="error" />
        <StatCard icon="AlertTriangle" title="警告" value={stats.warnings} tone="warn" />
        <StatCard icon="Info" title="信息" value={stats.infos} tone="success" />
        <StatCard icon="ShieldCheck" title="日志保留" value={stats.retention} />
      </div>

      {/* 筛选工具栏 */}
      <div className="log-center__toolbar">
        <SearchBox value={search} onChange={setSearch} placeholder="搜索日志内容、submit_id、任务名..." />
        <FilterSelect value={levelFilter} onChange={setLevelFilter} options={levelOptions} label="级别" />
        <FilterSelect value={sourceFilter} onChange={setSourceFilter} options={sourceOptions} label="来源" />
        <FilterSelect value={taskFilter} onChange={setTaskFilter} options={taskFilterOptions} label="任务" />
        <FilterSelect value={timeRange} onChange={setTimeRange} options={TIME_RANGE_OPTIONS} label="时间" />
        <ToggleSwitch checked={autoRefresh} onChange={setAutoRefresh} label="自动刷新" />
        <button type="button" className="outline-button" onClick={() => handleExport('json')} title="导出 JSON"><Download size={14} /> 导出</button>
        <button type="button" className="outline-button" onClick={handleClearFilters}>清空筛选</button>
      </div>

      {/* 主体三栏 */}
      <div className="log-center__body">
        {/* 左侧分类 */}
        <aside className="log-center__sidebar">
          <div className="log-center__sidebar-section">
            <h3 className="log-center__sidebar-heading">分类</h3>
            {SIDEBAR_CATEGORIES.map((cat) => (
              <button
                key={cat.key}
                type="button"
                className={`log-center__sidebar-item${category === cat.key ? ' active' : ''}`}
                onClick={() => setCategory(cat.key)}
              >
                <span>{cat.label}</span>
                <em>{catCounts[cat.key] || 0}</em>
              </button>
            ))}
          </div>
          <div className="log-center__sidebar-section">
            <h3 className="log-center__sidebar-heading">快速定位</h3>
            {QUICK_LOCATIONS.map((ql) => (
              <button
                key={ql.key}
                type="button"
                className="log-center__sidebar-item"
                onClick={() => handleQuickLocation(ql.key)}
              >
                <span>{ql.label}</span>
              </button>
            ))}
          </div>
        </aside>

        {/* 中间表格 */}
        <div className="log-center__table-wrap">
          <table className="log-center__table">
            <thead>
              <tr>
                <th className="log-center__th--time">时间</th>
                <th className="log-center__th--level">级别</th>
                <th className="log-center__th--source">来源</th>
                <th className="log-center__th--task">任务 / submit_id</th>
                <th className="log-center__th--msg">摘要</th>
              </tr>
            </thead>
            <tbody>
              {paginated.items.length ? paginated.items.map((log) => (
                <tr
                  key={log.id}
                  className={`log-center__row${selectedLogId === log.id ? ' selected' : ''}`}
                  onClick={() => setSelectedLogId(log.id)}
                >
                  <td className="log-center__td--time">{log.timestamp ? log.timestamp.slice(11, 19) : '-'}</td>
                  <td className="log-center__td--level">
                    <StatusPill variant={LEVEL_VARIANT_MAP[log.level] || 'neutral'}>{log.level}</StatusPill>
                  </td>
                  <td className="log-center__td--source">{SOURCE_LABEL_MAP[log.source] || log.source}</td>
                  <td className="log-center__td--task">
                    {log.taskTitle ? <span className="log-center__task-name">{log.taskTitle}</span> : null}
                    {log.submitId ? <span className="log-center__submit-id">{log.submitId}</span> : null}
                    {!log.taskTitle && !log.submitId ? '-' : null}
                  </td>
                  <td className="log-center__td--msg">{log.message}</td>
                </tr>
              )) : (
                <tr><td colSpan={5} className="log-center__empty">无匹配日志</td></tr>
              )}
            </tbody>
          </table>
        </div>

        {/* 右侧详情 */}
        <aside className="log-center__detail">
          {selectedLog ? (
            <div className="log-center__detail-inner">
              <div className="log-center__detail-header">
                <StatusPill variant={LEVEL_VARIANT_MAP[selectedLog.level] || 'neutral'}>{selectedLog.level}</StatusPill>
                <InlineCopyButton text={selectedLog.message} title="复制消息" />
              </div>
              <div className="log-center__detail-fields">
                <div className="log-center__detail-field"><span className="log-center__detail-label">时间</span><span className="log-center__detail-value">{selectedLog.timestamp || '-'}</span></div>
                <div className="log-center__detail-field"><span className="log-center__detail-label">来源</span><span className="log-center__detail-value">{SOURCE_LABEL_MAP[selectedLog.source] || selectedLog.source}</span></div>
                {selectedLog.taskTitle ? <div className="log-center__detail-field"><span className="log-center__detail-label">任务</span><span className="log-center__detail-value">{selectedLog.taskTitle}</span></div> : null}
                {selectedLog.submitId ? <div className="log-center__detail-field"><span className="log-center__detail-label">submit_id</span><span className="log-center__detail-value">{selectedLog.submitId}<InlineCopyButton text={selectedLog.submitId} /></span></div> : null}
                {selectedLog.module ? <div className="log-center__detail-field"><span className="log-center__detail-label">模块</span><span className="log-center__detail-value">{selectedLog.module}</span></div> : null}
                {selectedLog.errorDetail ? <div className="log-center__detail-field"><span className="log-center__detail-label">错误详情</span><span className="log-center__detail-value log-center__detail-value--error">{selectedLog.errorDetail}</span></div> : null}
              </div>
              {selectedLog.detail ? (
                <div className="log-center__detail-block">
                  <h4 className="log-center__detail-block-title">日志详情</h4>
                  <pre className="log-center__detail-pre">{selectedLog.detail}</pre>
                </div>
              ) : null}
              {selectedLog.rawOutput || selectedLog.stdout || selectedLog.stderr ? (
                <div className="log-center__detail-block">
                  <h4 className="log-center__detail-block-title">原始输出</h4>
                  {selectedLog.stdout ? <pre className="log-center__detail-pre">{selectedLog.stdout}</pre> : null}
                  {selectedLog.stderr ? <pre className="log-center__detail-pre log-center__detail-pre--err">{selectedLog.stderr}</pre> : null}
                  {selectedLog.rawOutput ? <pre className="log-center__detail-pre">{selectedLog.rawOutput}</pre> : null}
                </div>
              ) : null}
              {associations.length ? (
                <div className="log-center__detail-block">
                  <h4 className="log-center__detail-block-title">关联事件</h4>
                  {associations.map((a) => (
                    <div key={a.id} className="log-center__assoc-item" onClick={() => setSelectedLogId(a.id)}>
                      <StatusPill variant={LEVEL_VARIANT_MAP[a.level] || 'neutral'}>{a.level}</StatusPill>
                      <span className="log-center__assoc-msg">{a.message}</span>
                    </div>
                  ))}
                </div>
              ) : null}
              <div className="log-center__detail-actions">
                <button type="button" className="outline-button" onClick={handleCopyLog}>复制日志</button>
                {selectedLog.taskId ? <button type="button" className="gradient-button" onClick={handleLocateTask}>定位任务</button> : null}
                {(selectedLog.taskId || selectedLog.submitId) ? <button type="button" className="outline-button" onClick={handleViewContext}>查看上下文</button> : null}
              </div>
            </div>
          ) : (
            <div className="log-center__detail-empty">选择日志查看详情</div>
          )}
        </aside>
      </div>

      {/* 分页 */}
      <Pager
        page={paginated.page}
        totalPages={paginated.totalPages}
        total={paginated.total}
        pageSize={paginated.pageSize}
        onPageChange={setPage}
        onPageSizeChange={(s) => { setPageSize(s); setPage(1); }}
      />
    </div>
  );
}

function SchedulePickerModal({ title, mode = 'single', taskCount = 1, onClose, onApply }) {
  const isBatch = mode === 'batch';
  const isQueue = mode === 'queue';
  const isPrepare = mode === 'prepare';
  const isMultiTask = isBatch || isQueue;
  const today = formatDateInputValue(new Date());
  const tomorrowDate = new Date();
  tomorrowDate.setDate(tomorrowDate.getDate() + 1);
  const [scheduleMode, setScheduleMode] = useState(isMultiTask ? 'relative' : 'immediate');
  const [relativeHours, setRelativeHours] = useState(2);
  const [day, setDay] = useState('tomorrow');
  const [quietTime, setQuietTime] = useState('02:00');
  const [customDate, setCustomDate] = useState(formatDateInputValue(tomorrowDate));
  const [customTime, setCustomTime] = useState('02:00');
  const [intervalMinutes, setIntervalMinutes] = useState(0);
  const [plannedSubmitCount, setPlannedSubmitCount] = useState(1);
  const [error, setError] = useState('');

  const scheduleOptions = [
    {
      key: 'immediate',
      label: isQueue ? '马上排队' : isBatch ? '排队模式' : isPrepare ? '立即生成' : '立即提交',
      hint: isQueue ? '清掉预定时间，立刻进入连续队列' : isBatch ? '全部进入同一开始时间，按队列连续执行' : isPrepare ? '现在提交到即梦生成' : '移除计划时间，回到待提交',
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
    return isBatch ? new Date(Date.now() + 10000).toISOString() : null;
  };

  const [applying, setApplying] = useState(false);
  const handleApply = async () => {
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
    setApplying(true);
    try {
      await onApply?.({ scheduledAt, intervalMinutes, plannedSubmitCount });
    } finally {
      setApplying(false);
    }
  };

  return (
    <div className="modal-backdrop schedule-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="schedule-modal" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
        <header className="schedule-modal-head">
          <div>
            <span>{isQueue ? `排队模式 ${taskCount} 个任务` : isBatch ? `批量排布 ${taskCount} 个任务` : isPrepare ? '准备生成' : '单个任务排期'}</span>
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
              <span>排布间隔</span>
              <select value={intervalMinutes} onChange={(e) => setIntervalMinutes(Number(e.target.value))}>
                <option value={0}>连续排队</option>
                <option value={15}>15 分钟</option>
                <option value={30}>30 分钟</option>
                <option value={45}>45 分钟</option>
                <option value={60}>1 小时</option>
                <option value={90}>1.5 小时</option>
              </select>
            </label>
          ) : null}
          <label>
            <span>成功候选</span>
            <input
              type="number"
              min="1"
              max="10"
              value={plannedSubmitCount}
              onChange={(e) => setPlannedSubmitCount(e.target.value)}
            />
          </label>
        </div>

        {error ? <p className="schedule-error">{error}</p> : null}

        <footer className="schedule-modal-actions">
          <button type="button" className="outline-button" onClick={onClose}>取消</button>
          <button type="button" className="gradient-button" onClick={handleApply} disabled={applying}>
            {applying ? <><Loader2 size={14} className="spin" /> 处理中...</> : <><CalendarClock size={14} /> {isQueue ? '确认排队' : isBatch ? '确认排布' : isPrepare ? (scheduleMode === 'immediate' ? '立即生成' : '确认定时生成') : '确认安排'}</>}
          </button>
        </footer>
      </section>
    </div>
  );
}

function SettingsView({ cli, settingsForm, setSettingsForm, checkCli, checkCliUpdate, cliUpdate, cliUpdateStatus, saveSettings, installCli, installCliStatus, loginCli, loginCliStatus }) {
  const aiModelConfigs = settingsForm.ai_model_configs?.length ? settingsForm.ai_model_configs : [defaultAiModelConfig];
  const activeAiModelId = settingsForm.active_ai_model_id || aiModelConfigs[0]?.id || defaultAiModelConfig.id;
  const patchAiModel = (index, patch) => {
    const next = aiModelConfigs.map((config, i) => (i === index ? { ...config, ...patch } : config));
    setSettingsForm({ ...settingsForm, ai_model_configs: next, active_ai_model_id: activeAiModelId });
  };
  const addAiModel = () => {
    const id = `openai-${Date.now()}`;
    setSettingsForm({
      ...settingsForm,
      ai_model_configs: [...aiModelConfigs, { ...defaultAiModelConfig, id, name: `OpenAI 配置 ${aiModelConfigs.length + 1}` }],
      active_ai_model_id: id,
    });
  };
  const removeAiModel = (id) => {
    const next = aiModelConfigs.filter((c) => c.id !== id);
    const fallback = next[0] || defaultAiModelConfig;
    setSettingsForm({
      ...settingsForm,
      ai_model_configs: next.length ? next : [fallback],
      active_ai_model_id: activeAiModelId === id ? fallback.id : activeAiModelId,
    });
  };
  const normalizedImageSettings = normalizeImageModelSettingsForm(settingsForm);
  const imageModelConfigs = normalizedImageSettings.image_model_configs;
  const activeImageModelId = normalizedImageSettings.active_image_model_id;
  const activeImageModel = normalizedImageSettings.image_model_config;
  const patchImageModel = (index, patch) => {
    setSettingsForm(patchImageModelConfig(settingsForm, index, patch));
  };
  const addImageModel = () => {
    const id = `image-openai-${Date.now()}`;
    const nextConfig = { ...defaultImageModelConfig, id, name: `图片模型 ${imageModelConfigs.length + 1}` };
    setSettingsForm(normalizeImageModelSettingsForm({
      ...settingsForm,
      image_model_configs: [...imageModelConfigs, nextConfig],
      active_image_model_id: id,
      image_model_config: nextConfig,
    }));
  };
  const setActiveImageModel = (id, configs = imageModelConfigs) => {
    const selected = configs.find((config) => config.id === id) || configs[0] || defaultImageModelConfig;
    setSettingsForm(normalizeImageModelSettingsForm({
      ...settingsForm,
      image_model_configs: configs,
      active_image_model_id: selected.id,
      image_model_config: selected,
    }));
  };
  const removeImageModel = (id) => {
    const next = imageModelConfigs.filter((config) => config.id !== id);
    const fallback = next[0] || defaultImageModelConfig;
    const selected = activeImageModelId === id ? fallback : (next.find((config) => config.id === activeImageModelId) || fallback);
    setSettingsForm(normalizeImageModelSettingsForm({
      ...settingsForm,
      image_model_configs: next.length ? next : [fallback],
      active_image_model_id: selected.id,
      image_model_config: selected,
    }));
  };

  return (
    <form className="settings-page" onSubmit={saveSettings}>
      <div className="settings-main">

        {/* 1 CLI 配置 */}
        <NumberedSection number={1} title="CLI 配置">
          <label>
            CLI 路径（只读）
            <div className="settings-cli-path-row">
              <input value={cli.path || '未检测到'} readOnly />
              <StatusPill variant={cli.available ? 'ok' : 'bad'}>
                {cli.available ? '检测成功' : '未检测到'}
              </StatusPill>
            </div>
          </label>
          <label>
            CLI 版本
            <input value={cli.available ? (cli.version || '未返回版本信息') : '未检测到'} readOnly />
          </label>
          <div className="settings-hint">
            {cli.available ? `Commit：${cli.commit || '未知'} · 构建时间：${cli.build_time || '未知'}` : '检测到 CLI 后会显示版本、Commit 和构建时间。'}
          </div>
          <div className="settings-hint">
            {cli.installed_release_version
              ? `已保存官方版本：${cli.installed_release_version}${cli.installed_release_date ? ` · ${cli.installed_release_date}` : ''}`
              : `尚未保存官方版本号${cli.installed_release_path ? `（${cli.installed_release_path}）` : ''}`}
          </div>
          {cliUpdate ? (
            <div className="settings-hint">
              官方最新：{cliUpdate.latest_version || '未知'}
              {cliUpdate.latest_release_date ? ` · ${cliUpdate.latest_release_date}` : ''}
              {cliUpdate.update_available ? ' · 有新版本' : cliUpdate.comparable ? ' · 已是最新' : ' · 无法精确比较'}
              {cliUpdate.latest_release_notes ? ` · ${cliUpdate.latest_release_notes}` : ''}
            </div>
          ) : null}
          <div className="button-cluster" style={{ justifyContent: 'flex-start', gap: 8 }}>
            <button className="outline-button" type="button" onClick={checkCli}><RefreshCcw size={12} /> 重新检测</button>
            <button className="outline-button" type="button" onClick={checkCliUpdate} disabled={cliUpdateStatus === 'checking'}>
              <RefreshCcw size={12} /> {cliUpdateStatus === 'checking' ? '检查中…' : '检查更新'}
            </button>
            <button className="outline-button" type="button" onClick={installCli} disabled={installCliStatus === 'installing'}>
              <Download size={12} /> {installCliStatus === 'installing' ? '安装/更新中…' : '安装/更新'}
            </button>
            {installCliStatus === 'success' && <StatusPill variant="ok">安装/更新成功</StatusPill>}
            {installCliStatus === 'failed' && <StatusPill variant="bad">安装/更新失败</StatusPill>}
          </div>
          <div className="button-cluster" style={{ justifyContent: 'flex-start', gap: 8 }}>
            <button className="outline-button" type="button" onClick={() => loginCli(false)} disabled={!cli.available || loginCliStatus === 'logging'}>
              <User size={12} /> {loginCliStatus === 'logging' ? '登录中…' : 'CLI 登录'}
            </button>
            <button className="outline-button" type="button" onClick={() => loginCli(true)} disabled={!cli.available || loginCliStatus === 'logging'}>
              Headless 登录
            </button>
            {loginCliStatus === 'success' && <StatusPill variant="ok">登录流程完成</StatusPill>}
            {loginCliStatus === 'failed' && <StatusPill variant="bad">登录失败</StatusPill>}
          </div>
          <label>
            macOS 安装命令
            <CopyableInput
              value={settingsForm.mac_install_command || ''}
              onChange={(e) => setSettingsForm({ ...settingsForm, mac_install_command: e.target.value })}
            />
          </label>
          <label>
            Windows PowerShell 安装命令
            <CopyableInput
              value={settingsForm.windows_install_command || ''}
              placeholder="填入官方 PowerShell 安装/更新命令"
              onChange={(e) => setSettingsForm({ ...settingsForm, windows_install_command: e.target.value })}
            />
          </label>
        </NumberedSection>

        {/* 2 AI 模型配置 */}
        <NumberedSection number={2} title="AI 模型配置" subtitle="文字 AI，用于自动生成任务标题">
          <div className="setting-group-head">
            <label style={{ flex: 1, margin: 0 }}>
              当前使用模型
              <select
                value={activeAiModelId}
                onChange={(e) => setSettingsForm({ ...settingsForm, active_ai_model_id: e.target.value, ai_model_configs: aiModelConfigs })}
              >
                {aiModelConfigs.map((c) => <option key={c.id} value={c.id}>{c.name || c.model || c.id}</option>)}
              </select>
            </label>
            <button className="outline-button" type="button" onClick={addAiModel} style={{ alignSelf: 'flex-end' }}>
              <Plus size={12} /> 新增模型
            </button>
          </div>
          <div className="ai-model-list">
            {aiModelConfigs.map((config, index) => {
              const isActive = config.id === activeAiModelId;
              return (
                <div className={`ai-model-card${isActive ? ' active' : ''}`} key={config.id}>
                  <div className="ai-model-card-head">
                    <strong>{[config.name, config.model].filter(Boolean).join(' / ') || '未命名模型'}</strong>
                    <div className="ai-model-card-actions">
                      {isActive && <StatusPill variant="info">当前使用</StatusPill>}
                      <AiModelTestButton config={config} />
                      {!isActive && (
                        <button type="button" className="icon-ghost mini" title="设为当前模型"
                          onClick={() => setSettingsForm({ ...settingsForm, active_ai_model_id: config.id, ai_model_configs: aiModelConfigs })}>
                          <Star size={12} />
                        </button>
                      )}
                      <button type="button" className="icon-ghost mini" title="删除" disabled={aiModelConfigs.length <= 1}
                        onClick={() => removeAiModel(config.id)}>
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </div>
                  {isActive && (
                    <div className="ai-model-grid">
                      <label>名称<input value={config.name || ''} onChange={(e) => patchAiModel(index, { name: e.target.value })} /></label>
                      <label>模式
                        <select value={config.api_mode || 'responses'} onChange={(e) => patchAiModel(index, { api_mode: e.target.value })}>
                          <option value="responses">OpenAI Responses</option>
                          <option value="chat">Chat Completions</option>
                        </select>
                      </label>
                      <label>Base URL<input value={config.base_url || ''} onChange={(e) => patchAiModel(index, { base_url: e.target.value })} /></label>
                      <label>Model<input value={config.model || ''} onChange={(e) => patchAiModel(index, { model: e.target.value })} /></label>
                      <label className="ai-model-secret">
                        API Key
                        <PasswordInput value={config.api_key || ''} placeholder="sk-..." onChange={(e) => patchAiModel(index, { api_key: e.target.value })} />
                      </label>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </NumberedSection>

        {/* 3 自动查询设置 */}
        <NumberedSection number={3} title="自动查询设置">
          <ToggleSwitch
            checked={settingsForm.auto_query_enabled ?? true}
            onChange={(v) => setSettingsForm({ ...settingsForm, auto_query_enabled: v })}
            label="提交后自动查询结果"
          />
          <label>
            轮询间隔（秒）
            <div className="settings-input-hint-row">
              <input type="number" min="10" max="300" value={settingsForm.poll_interval_seconds ?? 60}
                onChange={(e) => setSettingsForm({ ...settingsForm, poll_interval_seconds: Number(e.target.value) })} />
              <span className="settings-range-hint">10-300</span>
            </div>
          </label>
          <label>
            日志保留条数
            <div className="settings-input-hint-row">
              <input type="number" min="50" max="10000" value={settingsForm.log_retention_count ?? 500}
                onChange={(e) => setSettingsForm({ ...settingsForm, log_retention_count: Number(e.target.value) })} />
              <span className="settings-range-hint">50-10000</span>
            </div>
          </label>
          <ToggleSwitch
            checked={settingsForm.prevent_sleep ?? true}
            onChange={(v) => setSettingsForm({ ...settingsForm, prevent_sleep: v })}
            label="预定任务期间防止系统睡眠"
            hint="开启后可能增加耗电，但可提升准点提交概率。macOS 使用 caffeinate，Windows 使用 SetThreadExecutionState。"
          />
        </NumberedSection>

        {/* 4 并发等待策略 */}
        <NumberedSection number={4} title="并发等待策略">
          <label>
            并发重试间隔（秒）
            <div className="settings-input-hint-row">
              <input type="number" min="30" value={settingsForm.concurrency_retry_delay_seconds || 300}
                onChange={(e) => setSettingsForm({ ...settingsForm, concurrency_retry_delay_seconds: e.target.value })} />
              <span className="settings-range-hint">30-3600</span>
            </div>
          </label>
        </NumberedSection>

        {/* 5 图片生成模型 - full width */}
        <NumberedSection number={5} title="图片生成模型" subtitle='用于"生图" Tab' className="numbered-section--full">
          <div className="setting-group-head">
            <label style={{ flex: 1, margin: 0 }}>
              当前使用模型
              <select value={activeImageModel.id} onChange={(e) => setActiveImageModel(e.target.value)}>
                {imageModelConfigs.map((config) => (
                  <option key={config.id} value={config.id}>{config.name || config.model || config.id}</option>
                ))}
              </select>
            </label>
            <button className="outline-button" type="button" onClick={addImageModel} style={{ alignSelf: 'flex-end' }}>
              <Plus size={12} /> 新增模型
            </button>
          </div>
          <div className="ai-model-list">
            {imageModelConfigs.map((config, index) => {
              const isActive = config.id === activeImageModel.id;
              return (
                <div className={`ai-model-card${isActive ? ' active' : ''}`} key={config.id}>
                  <div className="ai-model-card-head">
                    <strong>{[config.name, config.model].filter(Boolean).join(' / ') || '未命名图片模型'}</strong>
                    <div className="ai-model-card-actions">
                      {isActive && <StatusPill variant="info">当前使用</StatusPill>}
                      {!isActive && (
                        <button type="button" className="icon-ghost mini" title="设为当前模型" onClick={() => setActiveImageModel(config.id)}>
                          <Star size={12} />
                        </button>
                      )}
                      <button type="button" className="icon-ghost mini" title="删除" disabled={imageModelConfigs.length <= 1}
                        onClick={() => removeImageModel(config.id)}>
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </div>
                  {isActive && (
                    <div className="ai-model-grid">
                      <label>名称<input value={config.name || ''} onChange={(e) => patchImageModel(index, { name: e.target.value })} /></label>
                      <label>Base URL<input value={config.base_url || ''} onChange={(e) => patchImageModel(index, { base_url: e.target.value })} placeholder="https://api.openai.com/v1" /></label>
                      <label>模型名称<input value={config.model || ''} onChange={(e) => patchImageModel(index, { model: e.target.value })} placeholder="gpt-image-1 / dall-e-3" /></label>
                      <label className="ai-model-secret">
                        API Key
                        <PasswordInput value={config.api_key || ''} placeholder="sk-..." onChange={(e) => patchImageModel(index, { api_key: e.target.value })} />
                      </label>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </NumberedSection>

      </div>
      <div className="settings-form-footer">
        <button type="submit" className="gradient-button"><Save size={14} /> 保存设置</button>
      </div>
    </form>
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
            <Thumb asset={asset} label={asset.name || `图片 ${index + 1}`} onClick={() => { setPreviewSrc(resolveMediaSrc(asset.stored_path)); setPreviewAlt(asset.name); }} />
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
    setPreviewSrc(resolveMediaSrc(path));
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
            <Thumb asset={asset} label={asset.name} onClick={() => { setPreviewSrc(resolveMediaSrc(asset.stored_path)); setPreviewAlt(asset.name); }} />
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

const IMAGE_SIZE_OPTIONS = [
  { value: '1024x1024', label: '1:1 · 1024×1024' },
  { value: '1024x1536', label: '2:3 · 1024×1536' },
  { value: '1536x1024', label: '3:2 · 1536×1024' },
];

function imageGenItemSrc(item) {
  if (!item?.storedPath) return '';
  return resolveMediaSrc(item.storedPath);
}

function imageGenQueryMetaText(item) {
  if (!item?.lastQueryAt) return '等待首次查询…';
  const status = item.lastQueryStatus || 'unknown';
  return `最近查询：${new Date(item.lastQueryAt).toLocaleTimeString()} · ${status}`;
}

function normalizeMentions(list) {
  return list.map((item) => item.dataUrl || '');
}

function ImageGenView({
  settingsForm,
  history,
  previewItem,
  setPreviewItem,
  state,
  assetById,
  importTempImages,
  pasteClipboardImage,
  pasteSystemClipboardImage,
  refreshState,
  setFeedback,
}) {
  const [imagegenForm, setImagegenForm] = useState(() => createEmptyImageGenForm());
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState('');
  const [copyMsg, setCopyMsg] = useState('');
  const [queryingImageIds, setQueryingImageIds] = useState({});
  const [regeneratingImageIds, setRegeneratingImageIds] = useState({});
  const [promptExpanded, setPromptExpanded] = useState(false);
  const [imageModalSrc, setImageModalSrc] = useState(null);

  useEffect(() => { setPromptExpanded(false); }, [previewItem?.id]);

  const activeImageModel = normalizeImageModelSettingsForm(settingsForm).image_model_config;
  const hasConfig = Boolean(activeImageModel?.api_key?.trim());
  const promptIsEmpty = !imagegenForm.prompt?.trim();

  // 预览项跟着 history 实时刷新（pending → completed 自动更新画面）
  const livePreviewItem = useMemo(() => {
    if (!previewItem) return null;
    const fresh = history.find((h) => h.id === previewItem.id);
    return fresh || previewItem;
  }, [previewItem, history]);

  // 用 ref 持有最新 refreshState，避免因函数引用变化导致 effect 反复重建
  const refreshStateRef = useRef(refreshState);
  refreshStateRef.current = refreshState;

  // 只用 pendingIds 字符串作为 dep，而非整个 history 对象，避免 refreshState 后 history 引用
  // 变化导致 effect cleanup→cancelled=true 把轮询提前终止
  const pendingIds = useMemo(
    () => history.filter((h) => h.status === 'pending').map((h) => h.id).join(','),
    [history],
  );

  useEffect(() => {
    if (!pendingIds) return;
    const ids = pendingIds.split(',');
    let cancelled = false;
    let timer = null;
    const tick = async () => {
      for (const id of ids) {
        if (cancelled) return;
        try {
          await invoke('query_image_task_command', { historyId: id });
        } catch (err) {
          console.warn('[imagegen] query task failed', id, err);
        }
      }
      if (!cancelled) {
        await refreshStateRef.current();
        if (!cancelled) {
          timer = setTimeout(tick, 3000);
        }
      }
    };
    timer = setTimeout(tick, 3000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [pendingIds]);

  const mentionItems = useMemo(() => {
    const items = buildMentionItems({
      roles: state?.roles || [],
      assetById: assetById || new Map(),
      tempImagePaths: imagegenForm.temp_image_paths,
      tempImageAssetIds: imagegenForm.temp_image_asset_ids,
    });
    return filterMentionItemsForImageGen(items);
  }, [state?.roles, assetById, imagegenForm.temp_image_paths, imagegenForm.temp_image_asset_ids]);

  const referenceAssets = useMemo(
    () => (imagegenForm.image_asset_ids || []).map((id) => assetById?.get(id)).filter(Boolean),
    [imagegenForm.image_asset_ids, assetById],
  );

  const handleEditorUpdate = useCallback(({ plainText, refs }) => {
    if (refs?.imageAssetIds?.length) {
      console.log('[imagegen] @mention imageAssetIds:', refs.imageAssetIds);
    }
    setImagegenForm((current) => {
      const mentionIds = refs?.imageAssetIds || [];
      const currentIds = current.image_asset_ids || [];
      const toAdd = mentionIds.filter((id) => !currentIds.includes(id));
      if (toAdd.length > 0) {
        console.log('[imagegen] adding ids:', toAdd, 'new total:', [...currentIds, ...toAdd]);
      }
      if (toAdd.length === 0) {
        // 仅文本变化，避免动 image_asset_ids 数组引用（防止编辑器联动重置）
        if (current.prompt === plainText) return current;
        return { ...current, prompt: plainText };
      }
      return {
        ...current,
        prompt: plainText,
        image_asset_ids: [...currentIds, ...toAdd].slice(0, IMAGEGEN_MAX_REFERENCES),
      };
    });
  }, []);

  const handlePasteImageForEditor = useCallback(async (file) => {
    const asset = await pasteClipboardImage(file);
    setImagegenForm((current) => addReferenceAsset(current, asset));
    return asset;
  }, [pasteClipboardImage]);

  const handlePasteSystemImageForEditor = useCallback(async () => {
    const asset = await pasteSystemClipboardImage();
    setImagegenForm((current) => addReferenceAsset(current, asset));
    return asset;
  }, [pasteSystemClipboardImage]);

  async function handleAddTempImage() {
    try {
      const imported = await importTempImages();
      if (!imported.length) return;
      setImagegenForm((current) => {
        let form = current;
        for (const asset of imported) {
          form = addReferenceAsset(form, asset);
        }
        return form;
      });
    } catch (err) {
      setError(String(err));
    }
  }

  async function generate() {
    const promptText = imagegenForm.prompt?.trim();
    if (!promptText) return;
    setGenerating(true);
    setError('');
    try {
      const refIds = (imagegenForm.image_asset_ids || []).length > 0
        ? imagegenForm.image_asset_ids
        : undefined;
      const item = await invoke('generate_image_command', {
        prompt: promptText,
        size: imagegenForm.size,
        referenceAssetIds: refIds,
      });
      await refreshState();
      setPreviewItem(item);
    } catch (err) {
      setError(String(err));
    } finally {
      setGenerating(false);
    }
  }

  async function downloadItem(item) {
    try {
      const path = await invoke('download_imagegen_image_command', { historyId: item.id });
      setError('');
      setFeedback(`已保存图片：${path}`);
    } catch (err) {
      setError(`下载图片失败：${String(err)}`);
    }
  }

  async function copyItem(item) {
    try {
      await invoke('copy_imagegen_image_command', { historyId: item.id });
      setCopyMsg(item.id);
      setTimeout(() => setCopyMsg(''), 2000);
    } catch (err) {
      setError(`复制图片失败：${String(err)}`);
    }
  }

  async function retryQueryItem(item) {
    setQueryingImageIds((prev) => ({ ...prev, [item.id]: true }));
    setError('');
    try {
      const updated = await invoke('retry_query_image_task_command', { historyId: item.id });
      await refreshState();
      setPreviewItem(updated);
    } catch (err) {
      await refreshState();
      setError(`重新查询失败：${String(err)}`);
    } finally {
      setQueryingImageIds((prev) => ({ ...prev, [item.id]: false }));
    }
  }

  async function regenerateItem(item) {
    setRegeneratingImageIds((prev) => ({ ...prev, [item.id]: true }));
    setError('');
    try {
      const updated = await invoke('regenerate_image_command', { historyId: item.id });
      await refreshState();
      setPreviewItem(updated);
    } catch (err) {
      await refreshState();
      setError(`重新生成失败：${String(err)}`);
    } finally {
      setRegeneratingImageIds((prev) => ({ ...prev, [item.id]: false }));
    }
  }

  async function deleteItem(id) {
    try {
      await invoke('delete_imagegen_history_item_command', { id });
      await refreshState();
      if (previewItem?.id === id) setPreviewItem(null);
    } catch (err) {
      setError(String(err));
    }
  }

  async function clearHistory() {
    try {
      await invoke('clear_imagegen_history_command');
      await refreshState();
      setPreviewItem(null);
    } catch (err) {
      setError(String(err));
    }
  }

  function handleImageGenContextMenu(event) {
    if (shouldSuppressImageContextMenu(event.target)) {
      event.preventDefault();
    }
  }

  return (
    <div onContextMenu={handleImageGenContextMenu}>
    <div className="imagegen-layout">
      <div className="imagegen-main">
        <div className="panel imagegen-panel">
          <PanelHeading title="图片生成" />
          {!hasConfig && (
            <p className="imagegen-no-config">
              <AlertCircle size={14} /> 请先在设置中配置「图片生成模型」API Key
            </p>
          )}
          <div className="imagegen-form">
            <div className="imagegen-input-row">
              <div className="imagegen-ref-sidebar">
                {referenceAssets.map((asset, idx) => (
                  <div key={asset.id} className="imagegen-ref-thumb-wrap">
                    <img
                      src={resolveMediaSrc(asset.stored_path)}
                      alt={asset.name}
                      className="imagegen-ref-thumb"
                    />
                    <button
                      type="button"
                      className="imagegen-ref-remove"
                      title="移除参考图"
                      onClick={() => setImagegenForm((current) => removeReferenceAsset(current, asset.id, assetById))}
                    >
                      <X size={10} />
                    </button>
                  </div>
                ))}
                {referenceAssets.length < IMAGEGEN_MAX_REFERENCES && (
                  <button type="button" className="imagegen-ref-add-btn" title="添加参考图" onClick={handleAddTempImage}>
                    <ImagePlus size={15} />
                  </button>
                )}
              </div>
              <PromptMentionEditor
                value={imagegenForm.prompt}
                promptDoc={imagegenForm.prompt_doc}
                mentionItems={mentionItems}
                maxLength={8000}
                placeholder="描述想要生成的图片，输入 @ 可引用素材图…"
                onUpdate={handleEditorUpdate}
                onPasteImage={handlePasteImageForEditor}
                onPasteSystemImage={handlePasteSystemImageForEditor}
                tempImagePaths={imagegenForm.temp_image_paths}
              />
            </div>
            <div className="imagegen-controls">
              <select
                value={imagegenForm.size}
                onChange={(e) => setImagegenForm((f) => ({ ...f, size: e.target.value }))}
              >
                {IMAGE_SIZE_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>{o.label}</option>
                ))}
              </select>
              <button
                type="button"
                className="gradient-button"
                onClick={generate}
                disabled={generating || promptIsEmpty || !hasConfig}
              >
                {generating
                  ? <><Loader2 size={14} className="spin" /> 生成中…</>
                  : <><Sparkles size={14} /> 生成</>}
              </button>
            </div>
            {error && <p className="imagegen-error"><AlertCircle size={13} /> {error}</p>}
          </div>

          {livePreviewItem && (
            <div className="imagegen-preview">
              <div className="imagegen-preview-header">
                <span className="imagegen-preview-title">预览{livePreviewItem.status === 'pending' ? ' · 生成中' : livePreviewItem.status === 'failed' ? ' · 失败' : ''}</span>
                <button type="button" className="icon-ghost" title="关闭" onClick={() => setPreviewItem(null)}>
                  <X size={14} />
                </button>
              </div>
              {livePreviewItem.status === 'pending' ? (
                <div className="imagegen-preview-img imagegen-preview-pending">
                  <Loader2 size={32} className="spin" />
                  <span style={{marginTop:8, fontSize:12, color:'var(--muted)'}}>异步生成中，请稍候…</span>
                  <span style={{marginTop:4, fontSize:12, color:'var(--muted)'}}>{imageGenQueryMetaText(livePreviewItem)}</span>
                </div>
              ) : livePreviewItem.status === 'failed' ? (
                <div className="imagegen-preview-img imagegen-preview-pending">
                  <AlertCircle size={32} style={{color:'#e03c3c'}} />
                  <span style={{marginTop:8, fontSize:12, color:'#e03c3c', textAlign:'center', padding:'0 12px'}}>{livePreviewItem.error || '生成失败'}</span>
                </div>
              ) : (
                <img
                  src={imageGenItemSrc(livePreviewItem)}
                  alt={livePreviewItem.prompt}
                  className="imagegen-preview-img"
                />
              )}
              {livePreviewItem.status === 'completed' && (
                <div className="imagegen-preview-actions">
                  <button type="button" className="outline-button" onClick={() => copyItem(livePreviewItem)}>
                    <Copy size={13} /> {copyMsg === livePreviewItem.id ? '已复制！' : '复制图片'}
                  </button>
                  <button type="button" className="outline-button" onClick={() => setImageModalSrc(imageGenItemSrc(livePreviewItem))}>
                    <ZoomIn size={13} /> 查看大图
                  </button>
                  <button type="button" className="outline-button" onClick={() => downloadItem(livePreviewItem)}>
                    <Download size={13} /> 下载
                  </button>
                  <button type="button" className="outline-button" onClick={() => regenerateItem(livePreviewItem)} disabled={regeneratingImageIds[livePreviewItem.id]}>
                    {regeneratingImageIds[livePreviewItem.id]
                      ? <><Loader2 size={13} className="spin" /> 生成中…</>
                      : <><Sparkles size={13} /> 重新生成</>}
                  </button>
                </div>
              )}
              {livePreviewItem.status === 'failed' && livePreviewItem.taskId && (
                <div className="imagegen-preview-actions">
                  <button type="button" className="outline-button" onClick={() => retryQueryItem(livePreviewItem)} disabled={queryingImageIds[livePreviewItem.id]}>
                    {queryingImageIds[livePreviewItem.id]
                      ? <><Loader2 size={13} className="spin" /> 查询中…</>
                      : <><RefreshCcw size={13} /> 重新查询</>}
                  </button>
                  <button type="button" className="outline-button" onClick={() => regenerateItem(livePreviewItem)} disabled={regeneratingImageIds[livePreviewItem.id]}>
                    {regeneratingImageIds[livePreviewItem.id]
                      ? <><Loader2 size={13} className="spin" /> 生成中…</>
                      : <><Sparkles size={13} /> 重新生成</>}
                  </button>
                </div>
              )}
              <p className="imagegen-preview-meta">{livePreviewItem.size} · {new Date(livePreviewItem.createdAt).toLocaleString()}</p>
              {livePreviewItem.prompt && (
                <div className="imagegen-preview-prompt-wrap">
                  <div className="imagegen-preview-prompt-header">
                    <span className="imagegen-preview-refs-label">提示词</span>
                    <button
                      type="button"
                      className="icon-ghost mini"
                      title="复制提示词"
                      onClick={() => { navigator.clipboard.writeText(livePreviewItem.prompt).catch(() => {}); setCopyMsg(`prompt_${livePreviewItem.id}`); setTimeout(() => setCopyMsg(''), 2000); }}
                    >
                      {copyMsg === `prompt_${livePreviewItem.id}` ? <span style={{fontSize:10}}>已复制</span> : <Copy size={11} />}
                    </button>
                  </div>
                  <div className={`imagegen-preview-prompt${promptExpanded ? ' expanded' : ''}`}>
                    {livePreviewItem.prompt}
                  </div>
                  {livePreviewItem.prompt.length > 150 && (
                    <button type="button" className="imagegen-prompt-toggle" onClick={() => setPromptExpanded((v) => !v)}>
                      {promptExpanded ? '收起' : '展开全部'}
                    </button>
                  )}
                </div>
              )}
              {livePreviewItem.referenceAssetIds?.length > 0 && (
                <div className="imagegen-preview-refs">
                  <span className="imagegen-preview-refs-label">参考图</span>
                  <div className="imagegen-ref-list">
                    {livePreviewItem.referenceAssetIds.map((id) => {
                      const asset = assetById?.get(id);
                      if (!asset) return null;
                      return (
                        <div key={id} className="imagegen-ref-thumb-wrap">
                          <img src={resolveMediaSrc(asset.stored_path)} alt={asset.name} className="imagegen-ref-thumb" />
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="panel imagegen-history">
        <PanelHeading
          title={`历史记录（${history.length}）`}
          action={history.length > 0
            ? <button type="button" className="outline-button" onClick={clearHistory}>清空</button>
            : null}
        />
        {history.length === 0 && <p className="empty-cell">暂无记录</p>}
        <div className="imagegen-history-list">
          {history.map((item) => (
            <div
              key={item.id}
              className={`imagegen-history-item${previewItem?.id === item.id ? ' active' : ''}`}
            >
              <div className="imagegen-thumb-wrap">
                {item.status === 'pending' ? (
                  <div className="imagegen-thumb imagegen-thumb-pending"><Loader2 size={18} className="spin" /></div>
                ) : item.status === 'failed' ? (
                  <div className="imagegen-thumb imagegen-thumb-failed" title={item.error || '生成失败'}><AlertCircle size={18} /></div>
                ) : (
                  <>
                    <img src={imageGenItemSrc(item)} alt={item.prompt} className="imagegen-thumb" />
                    <button
                      type="button"
                      className="imagegen-thumb-zoom"
                      title="查看大图"
                      onClick={(e) => { e.stopPropagation(); setImageModalSrc(imageGenItemSrc(item)); }}
                    >
                      <ZoomIn size={14} />
                    </button>
                  </>
                )}
                {item.referenceAssetIds?.length > 0 && (
                  <span className="imagegen-ref-badge">{item.referenceAssetIds.length}</span>
                )}
              </div>
              <div className="imagegen-history-meta imagegen-history-meta-clickable" onClick={() => setPreviewItem(item)}>
                <span className="imagegen-history-prompt">{item.prompt.length > 60 ? item.prompt.slice(0, 60) + '…' : item.prompt}</span>
                <span className="imagegen-history-time">
                  {new Date(item.createdAt).toLocaleString()} · {item.size}
                  {item.status === 'pending' ? ` · ${imageGenQueryMetaText(item)}` : ''}
                </span>
              </div>
              <div className="imagegen-history-actions" onClick={(e) => e.stopPropagation()}>
                {item.status === 'completed' && (
                  <>
                    <button type="button" className="icon-ghost mini" title="复制" onClick={(e) => { e.preventDefault(); e.stopPropagation(); copyItem(item); }}>
                      <Copy size={12} />
                    </button>
                    <button type="button" className="icon-ghost mini" title="下载" onClick={(e) => { e.preventDefault(); e.stopPropagation(); downloadItem(item); }}>
                      <Download size={12} />
                    </button>
                    <button type="button" className="icon-ghost mini" title="重新生成" disabled={regeneratingImageIds[item.id]} onClick={(e) => { e.preventDefault(); e.stopPropagation(); regenerateItem(item); }}>
                      {regeneratingImageIds[item.id] ? <Loader2 size={12} className="spin" /> : <Sparkles size={12} />}
                    </button>
                    <button type="button" className="icon-ghost mini" title="保存为角色" onClick={(e) => { e.preventDefault(); e.stopPropagation(); handleSaveAsRole(item.storedPath); }}>
                      <User size={12} />
                    </button>
                  </>
                )}
                {item.status === 'failed' && item.taskId && (
                  <>
                    <button type="button" className="icon-ghost mini" title="重新查询" disabled={queryingImageIds[item.id]} onClick={(e) => { e.preventDefault(); e.stopPropagation(); retryQueryItem(item); }}>
                      {queryingImageIds[item.id] ? <Loader2 size={12} className="spin" /> : <RefreshCcw size={12} />}
                    </button>
                    <button type="button" className="icon-ghost mini" title="重新生成" disabled={regeneratingImageIds[item.id]} onClick={(e) => { e.preventDefault(); e.stopPropagation(); regenerateItem(item); }}>
                      {regeneratingImageIds[item.id] ? <Loader2 size={12} className="spin" /> : <Sparkles size={12} />}
                    </button>
                  </>
                )}
                <button type="button" className="icon-ghost mini" title="删除" onClick={(e) => { e.preventDefault(); e.stopPropagation(); deleteItem(item.id); }}>
                  <Trash2 size={12} />
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
    <ImageModal src={imageModalSrc} alt="" onClose={() => setImageModalSrc(null)} onCopy={() => { const item = history.find(h => imageGenItemSrc(h) === imageModalSrc); if (item) return copyItem(item); }} />
    </div>
  );
}

class AppErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null, info: null };
  }
  componentDidCatch(error, info) {
    this.setState({ error, info });
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div style={{ padding: 24, fontFamily: 'Menlo, monospace', color: '#1f2944' }}>
        <h2>应用启动失败</h2>
        <pre style={{ whiteSpace: 'pre-wrap' }}>{String(this.state.error?.stack || this.state.error)}</pre>
        <pre style={{ whiteSpace: 'pre-wrap' }}>{this.state.info?.componentStack || ''}</pre>
      </div>
    );
  }
}

createRoot(document.getElementById('root')).render(<AppErrorBoundary><App /></AppErrorBoundary>);
