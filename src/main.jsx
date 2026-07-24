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
import { applyEditorUpdate, applyMentionRefsToTaskForm } from './prompt-editor-utils.js';
import {
  deriveCurrentExecutionRecord,
  deriveCurrentQueryRecords,
  deriveTaskHistory,
  buildGenerationStats,
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
import LaneStrip from './components/LaneStrip.jsx';
import {
  getLaneStatuses,
  getTaskRouteInfo,
  formatTaskNextTime,
  deriveNextAction,
  deriveTimelineEvents,
  selectKeyTimelineRecords,
  laneLabel,
  getSharedWaitingTasks,
} from './lane-utils.js';
import {
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
  Image,
  ImagePlus,
  ListChecks,
  Loader2,
  MoreHorizontal,
  Pause,
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
  X,
  Zap,
  ZoomIn,
} from 'lucide-react';
import {
  filterTasks,
  sortTasks,
  paginateTasks,
  formatPaginationLabel,
  deriveTaskDispatchInfo,
  canDeleteTask,
  getTaskResultItems,
  getTaskHitResources,
  getCommandPreviewPresentation,
  deriveTaskDetailMetrics,
  getTaskDetailSectionOrder,
} from './queue-view-utils.js';
import {
  buildBatchQueuePlan,
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
import { resolveMediaSrc } from './media-src.js';
import './styles.css';

const PromptMentionEditor = React.lazy(() => import('./components/PromptMentionEditor.jsx'));

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
  taskPriorities: {},
  logs: [],
};

const tauriRuntimeAvailable = typeof window !== 'undefined' && Boolean(window.__TAURI_INTERNALS__?.metadata?.currentWindow);

function App() {
  const [activeView, setActiveView] = useState('queue');
  const [state, setState] = useState(emptyState);
  useEffect(() => {
    document.getElementById('boot-screen')?.remove();
  }, []);
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
  const resumeRefreshInFlightRef = useRef(null);
  const [lastTickAt, setLastTickAt] = useState(null);
  const [selectedTaskId, setSelectedTaskId] = useState('');
  const [selectedRoleId, setSelectedRoleId] = useState('');
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

  async function refreshStateIfChanged({ force = false } = {}) {
    if (resumeRefreshInFlightRef.current) return resumeRefreshInFlightRef.current;

    const request = (async () => {
      if (!force) {
        try {
          const sig = await invoke('get_state_signature');
          if (sig === lastStateSignatureRef.current) return false;
        } catch {
          // 签名读取失败时退化为完整刷新，保证前台状态最终一致。
        }
      }
      await refreshStateRef.current?.();
      return true;
    })();

    resumeRefreshInFlightRef.current = request;
    try {
      return await request;
    } finally {
      if (resumeRefreshInFlightRef.current === request) {
        resumeRefreshInFlightRef.current = null;
      }
    }
  }

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
    refreshStateIfChanged({ force: true });
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
    const timer = window.setInterval(() => {
      refreshStateIfChanged();
    }, 30000);
    return () => {
      window.clearInterval(timer);
    };
  }, []);

  // 记录前端生命周期，辅助判断是进程退出、WebView 暂停，还是后端调度仍在运行。
  useEffect(() => {
    if (!tauriRuntimeAvailable) return undefined;
    let visibleRefreshFrame = 0;
    const logLifecycle = (eventType, message) => {
      invoke('record_lifecycle_event_command', {
        eventType,
        message,
        detail: `visibility=${document.visibilityState}`,
      }).catch(() => {});
    };
    function scheduleVisibleStateRefresh() {
      if (document.hidden) return;
      if (visibleRefreshFrame) window.cancelAnimationFrame(visibleRefreshFrame);
      visibleRefreshFrame = window.requestAnimationFrame(() => {
        visibleRefreshFrame = 0;
        refreshStateIfChanged();
      });
    }
    logLifecycle('frontend_ready', '前端页面就绪');
    const handleFocus = () => {
      logLifecycle('frontend_focus', '窗口聚焦');
      scheduleVisibleStateRefresh();
    };
    const handleBlur = () => logLifecycle('frontend_blur', '窗口失焦');
    const handleVisibility = () => {
      if (document.hidden) {
        logLifecycle('frontend_hidden', '页面隐藏');
      } else {
        logLifecycle('frontend_visible', '页面恢复可见');
        scheduleVisibleStateRefresh();
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
      if (visibleRefreshFrame) window.cancelAnimationFrame(visibleRefreshFrame);
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
          series: form.series.trim(),
          description: form.description.trim(),
          disabled: Boolean(form.disabled),
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
          standard_lane_enabled: settingsForm.standard_lane_enabled !== false,
          fast_lane_enabled: settingsForm.fast_lane_enabled !== false,
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
                <React.Suspense fallback={<div className="prompt-editor-loading"><Loader2 size={16} /> 正在载入编辑器</div>}>
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
                </React.Suspense>
                <div className="info-strip">
                  <Sparkles size={13} />
                  输入 @ 可引用具体图片（如 @女主厨师服）、音频或临时图片。
                </div>
              </div>

              <div className="schedule-hint-card">
                <CalendarClock size={16} />
                <div>
                  <strong>默认不定时</strong>
                  <span>保存后进入任务中心，可单选或多选任务后再指定开始时间、立即提交或批量排队。</span>
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
  roleEditor,
  setRoleEditor,
}) {
  const filteredRoles = state.roles;

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
        <div className="role-card-grid">
          {filteredRoles.map((role) => {
            const media = getRoleMedia(role, assetById);
            const isSelected = role.id === selectedRoleId;
            return (
              <button
                key={role.id}
                type="button"
                className={`role-card ${isSelected ? 'selected' : ''}${role.disabled ? ' disabled' : ''}`}
                onClick={() => setSelectedRoleId(role.id)}
              >
                <div className="role-card-avatar">
                  <Thumb asset={media.images[0]} label={role.name} />
                </div>
                <div className="role-card-body">
                  <div className="role-card-head">
                    <strong>{role.name}</strong>
                    {isSelected ? <span className="role-default-badge">默认</span> : null}
                    {role.disabled ? <span className="role-disabled-badge">已停用</span> : null}
                    <button type="button" className="role-card-more" onClick={(e) => { e.stopPropagation(); }}>
                      <MoreHorizontal size={15} />
                    </button>
                  </div>
                  {role.series ? <p className="role-card-series">系列：{role.series}</p> : null}
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
                {selectedRole.disabled ? <span className="role-disabled-badge">已停用</span> : null}
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
                {selectedRole.series ? <span>系列：{selectedRole.series}</span> : null}
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

const TaskCard = React.memo(function TaskCard({ task, index, selected, selectedForBatch, batchSelectable, queuePriority = 0, assetById, roles, onSelect, onToggleSelection }) {
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
  const routeInfo = useMemo(() => getTaskRouteInfo(task), [task]);
  const routeTime = useMemo(() => formatTaskNextTime(task), [task]);
  const handleClick = useCallback(() => onSelect(task.id), [onSelect, task.id]);

  const isDone = task.status === 'succeeded';
  const isFailed = task.status === 'failed';

  return (
    <div className={`qc-task-row${selected ? ' selected' : ''}`} onClick={handleClick} role="button" tabIndex={0}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleClick(); }}>
      <button
        type="button"
        className={`qc-task-check${selectedForBatch ? ' checked' : ''}`}
        title={selectedForBatch ? '取消选择' : '选择任务'}
        disabled={!batchSelectable}
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
        <span className="qc-task-title">
          {queuePriority > 0 ? <em className="qc-task-priority">{'★'.repeat(queuePriority)}</em> : null}
          {task.title || '未命名任务'}
        </span>
        <span className="qc-task-sub">{task.params?.model_version || ''}{task.params?.ratio ? ` · ${task.params.ratio}` : ''}</span>
        {['queued', 'scheduled', 'retry_wait'].includes(task.status) ? (
          <span className="qc-task-dispatch-sub">{dispatchInfo.compactText}</span>
        ) : null}
      </div>
      <div className="qc-task-right">
        <StatusBadge task={task} />
        <div className={`qc-route${routeInfo.kind === 'fast' ? ' fast' : ''}`}>
          <b>{routeInfo.assigned ? `${routeInfo.label}车道` : routeInfo.label}</b>
          {isDone || isFailed ? null : (
            <span>{routeTime || dispatchInfo.nextText || '—'}</span>
          )}
        </div>
      </div>
    </div>
  );
});

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

function parseAttemptQueueInfo(stdout) {
  try {
    return JSON.parse(stdout || '')?.queue_info || null;
  } catch {
    return null;
  }
}

// ── QueueView ────────────────────────────────────────────────────────────────

function QueueView({
  tasks,
  settings,
  assetById,
  state,
  queryTask,
  pendingTaskOps = {},
  pendingExecutionOps = {},
  queryExecutionRecord = async () => {},
  processQueueOnce = async () => {},
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
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(12);
  const [searchQuery, setSearchQuery] = useState('');
  const [resourcePreview, setResourcePreview] = useState(null);
  const [selectedBatchIds, setSelectedBatchIds] = useState([]);
  const [selectedExecutionId, setSelectedExecutionId] = useState(null);
  const [scheduleModal, setScheduleModal] = useState(null);
  const [commandPreviewModal, setCommandPreviewModal] = useState(null);
  const [recordsModal, setRecordsModal] = useState(null);
  const [previewResultValue, setPreviewResultValue] = useState('');
  const [pendingLaneKind, setPendingLaneKind] = useState('');
  const [pendingProbeLaneKind, setPendingProbeLaneKind] = useState('');
  const [pendingPriority, setPendingPriority] = useState(false);
  const [pendingBatchPause, setPendingBatchPause] = useState(false);
  const [pendingBatchDelete, setPendingBatchDelete] = useState(false);

  // ── derived data ─────────────────────────────────────────────────────────
  const filteredSorted = useMemo(
    () => sortTasks(filterTasks(tasks, { searchQuery })),
    [tasks, searchQuery]
  );
  const sharedWaitingTasks = useMemo(() => getSharedWaitingTasks(tasks), [tasks]);
  const generationStats = useMemo(() => buildGenerationStats(tasks), [tasks]);
  const paged = useMemo(() => paginateTasks(filteredSorted, page, pageSize), [filteredSorted, page, pageSize]);

  const selectedTask = useMemo(
    () => tasks.find((t) => t.id === selectedTaskId) || null,
    [tasks, selectedTaskId]
  );
  const selectedQueuePriority = selectedTask ? Number(state.taskPriorities?.[selectedTask.id] || 0) : 0;

  useEffect(() => {
    if (!paged.items.length) return;
    if (!paged.items.some((task) => task.id === selectedTaskId)) {
      setSelectedTaskId(paged.items[0].id);
    }
  }, [paged.items, selectedTaskId, setSelectedTaskId]);

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
  const laneStatuses = useMemo(() => {
    return getLaneStatuses(state.laneStatus, tasks);
  }, [state.laneStatus, tasks]);
  const selectedNextAction = useMemo(
    () => deriveNextAction(selectedTask, Date.now(), {
      laneStatuses,
      schedulerTickSeconds: 30,
    }),
    [selectedTask, laneStatuses]
  );
  const allQueryAttempts = useMemo(
    () => deriveCurrentQueryRecords(selectedTask, selectedExecutionId).slice().reverse(),
    [selectedTask, selectedExecutionId]
  );
  const timelineEvents = useMemo(() => deriveTimelineEvents(selectedTask), [selectedTask]);
  const keyTimelineRecords = useMemo(
    () => selectKeyTimelineRecords(timelineEvents, allQueryAttempts, 4),
    [timelineEvents, allQueryAttempts]
  );
  const commandText = useMemo(
    () => executionView?.command_preview?.join(' \\\n  ') || '',
    [executionView]
  );
  const commandPresentation = useMemo(
    () => getCommandPreviewPresentation(commandText),
    [commandText]
  );
  const selectedSubmitId = selectedTask?.submit_id || currentExecution?.submit_id || '';
  const hitResources = useMemo(() => getTaskHitResources(executionView, assetById), [executionView, assetById]);
  const resultItems = useMemo(() => getTaskResultItems(executionView), [executionView]);
  const selectedRouteInfo = useMemo(() => getTaskRouteInfo(executionView), [executionView]);
  const selectedLaneStatus = selectedRouteInfo.kind
    ? laneStatuses.find((lane) => lane.queueKind === selectedRouteInfo.kind)
    : null;
  const detailMetrics = useMemo(() => deriveTaskDetailMetrics(executionView, {
    sharedPoolCount: sharedWaitingTasks.length,
    nextCheckSeconds: 30,
    actualLaneLabel: selectedRouteInfo.assigned ? selectedRouteInfo.label : '',
    nextQueryAt: selectedLaneStatus?.nextCheckAt || '',
  }), [executionView, sharedWaitingTasks.length, selectedRouteInfo, selectedLaneStatus?.nextCheckAt]);
  const detailSectionOrder = useMemo(
    () => getTaskDetailSectionOrder(executionView?.status || selectedTask?.status),
    [executionView?.status, selectedTask?.status]
  );
  const detailHealth = (() => {
    const status = executionView?.status || selectedTask?.status;
    if (status === 'succeeded') return { tone: 'done', label: '已完成' };
    if (status === 'failed' || status === 'schema_error') return { tone: 'fail', label: '需要处理' };
    if (selectedTask?.auto_query_stopped) return { tone: 'fail', label: '查询已停止' };
    if (status === 'retry_wait') return { tone: 'retry', label: '自动重试' };
    if (['submitting', 'submitted', 'querying'].includes(status)) return { tone: 'running', label: '自动运行' };
    return { tone: 'idle', label: '无需干预' };
  })();
  const selectedBatchTasks = useMemo(
    () => selectedBatchIds.map((id) => tasks.find((task) => task.id === id)).filter(Boolean),
    [selectedBatchIds, tasks]
  );
  const schedulableSelectedTasks = useMemo(
    () => selectedBatchTasks.filter(canScheduleTask),
    [selectedBatchTasks]
  );
  const pausableSelectedTasks = useMemo(
    () => selectedBatchTasks.filter((task) => ['scheduled', 'queued', 'retry_wait'].includes(task.status)),
    [selectedBatchTasks]
  );
  const deletableSelectedTasks = useMemo(
    () => selectedBatchTasks.filter(canDeleteTask),
    [selectedBatchTasks]
  );
  const selectablePagedIds = useMemo(
    () => paged.items.filter((task) => canScheduleTask(task) || canDeleteTask(task)).map((task) => task.id),
    [paged.items]
  );

  useEffect(() => {
    setResourcePreview(null);
    setCommandPreviewModal(null);
    setRecordsModal(null);
    setSelectedExecutionId(null);
    setPreviewResultValue('');
  }, [selectedTaskId]);

  useEffect(() => {
    setSelectedBatchIds((ids) => ids.filter((id) => tasks.some(
      (task) => task.id === id && (canScheduleTask(task) || canDeleteTask(task))
    )));
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
  const handleToggleLane = async (queueKind, enabled) => {
    setPendingLaneKind(queueKind);
    try {
      await invoke('set_lane_enabled_command', { queueKind, enabled });
      await refreshState();
      setFeedback(`${laneLabel(queueKind)}车道已${enabled ? '开启' : '关闭'}`);
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingLaneKind('');
    }
  };
  const handleProbeLane = async (lane) => {
    if (!lane || pendingProbeLaneKind) return;
    setPendingProbeLaneKind(lane.queueKind);
    try {
      if (lane.isActive && lane.currentTaskId && lane.submitId) {
        await queryTask(lane.currentTaskId, lane.submitId);
      } else {
        await processQueueOnce();
      }
    } finally {
      setPendingProbeLaneKind('');
    }
  };
  const handleSetQueuePriority = async (priority) => {
    if (!selectedTask || pendingPriority) return;
    setPendingPriority(true);
    try {
      await invoke('set_task_queue_priority_command', {
        taskId: selectedTask.id,
        priority,
      });
      await refreshState();
      setFeedback(priority === 2 ? '已设为下一位' : priority === 1 ? '已设为第二优先' : '已恢复稳定随机排队');
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingPriority(false);
    }
  };
  const handleDeleteTask = (task) => {
    if (!canDeleteTask(task)) {
      setFeedback('任务已开始生成或正在远端排队，暂不可删除');
      return;
    }
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
  const toggleTaskSelection = useCallback((task) => {
    if (!canScheduleTask(task) && !canDeleteTask(task)) return;
    setSelectedBatchIds((ids) => ids.includes(task.id) ? ids.filter((id) => id !== task.id) : [...ids, task.id]);
  }, []);
  const toggleSelectPage = () => {
    setSelectedBatchIds((ids) => {
      const pageAllSelected = selectablePagedIds.length && selectablePagedIds.every((id) => ids.includes(id));
      if (pageAllSelected) return ids.filter((id) => !selectablePagedIds.includes(id));
      return uniqueValues([...ids, ...selectablePagedIds]);
    });
  };
  const openPrepareGenerate = (task) => {
    if (!task || !canScheduleTask(task)) {
      setFeedback('当前任务正在执行或查询，暂不可排队');
      return;
    }
    setScheduleModal({ mode: 'prepare', taskIds: [task.id], title: `排队「${task.title || '未命名任务'}」` });
  };
  const openBatchSchedule = () => {
    const taskIds = schedulableSelectedTasks.map((task) => task.id);
    if (!taskIds.length) {
      setFeedback('请先选择可批量排队的任务');
      return;
    }
    setScheduleModal({
      mode: 'batch',
      taskIds,
      title: `批量排队 ${taskIds.length} 个任务`,
    });
  };
  const pauseSelectedTasks = async () => {
    const taskIds = pausableSelectedTasks.map((task) => task.id);
    if (!taskIds.length || pendingBatchPause) return;
    setPendingBatchPause(true);
    try {
      await invoke('pause_tasks_command', { taskIds });
      setFeedback(`已暂停 ${taskIds.length} 个任务`);
      setSelectedBatchIds([]);
      await refreshState();
    } catch (error) {
      setFeedback(String(error));
    } finally {
      setPendingBatchPause(false);
    }
  };
  const deleteSelectedTasks = () => {
    const taskIds = deletableSelectedTasks.map((task) => task.id);
    if (!taskIds.length || pendingBatchDelete) return;
    askConfirm({
      title: '批量删除任务',
      body: `确认删除已选的 ${taskIds.length} 个任务？本地视频文件和角色素材不会被删除。`,
      confirmText: `删除 ${taskIds.length} 个任务`,
      onConfirm: async () => {
        setPendingBatchDelete(true);
        try {
          await invoke('delete_tasks_command', { taskIds });
          setFeedback(`已删除 ${taskIds.length} 个任务`);
          setSelectedBatchIds([]);
          setSelectedTaskId('');
          await refreshState();
        } finally {
          setPendingBatchDelete(false);
        }
      },
    });
  };
  const applySchedulePlan = async ({ scheduledAt, intervalMinutes, plannedSubmitCount }) => {
    if (!scheduleModal?.taskIds?.length) return;
    try {
      const submitCount = Math.max(1, Math.min(10, Number(plannedSubmitCount || 1)));
      if (scheduleModal.mode === 'batch') {
        const plan = buildBatchQueuePlan(scheduleModal.taskIds, { startAt: scheduledAt || null, intervalMinutes });
        await invoke('queue_tasks_with_batch_schedule_command', {
          plan,
          plannedSubmitCount: submitCount,
          alternateFastModel: false,
        });
        setFeedback(`已批量排队：${formatSchedulePlanSummary(plan)}`);
        setSelectedBatchIds([]);
      } else if (scheduleModal.mode === 'prepare') {
        await invoke('set_task_planned_submit_count_command', {
          taskId: scheduleModal.taskIds[0],
          plannedSubmitCount: submitCount,
        });
        const operation = resolvePrepareGenerateOperation({ scheduledAt });
        if (operation.type === 'submit') {
          await rescheduleTask(scheduleModal.taskIds[0], '');
          setFeedback('已加入队列，调度器会自动选择空闲车道');
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

  const handleNewTask = () => {
    setEditingTaskId(null);
    setTaskForm(createEmptyTaskForm());
    setActiveView('create');
  };
  const handleSelectLaneTask = useCallback((taskId) => {
    let taskIndex = filteredSorted.findIndex((task) => task.id === taskId);
    if (taskIndex < 0) {
      setSearchQuery('');
      taskIndex = sortTasks(tasks).findIndex((task) => task.id === taskId);
    }
    if (taskIndex >= 0) setPage(Math.floor(taskIndex / pageSize) + 1);
    setSelectedTaskId(taskId);
  }, [filteredSorted, pageSize, setSelectedTaskId, tasks]);

  const handleSearchChange = useCallback((event) => {
    setSearchQuery(event.target.value);
    setPage(1);
  }, []);

  const clearSearch = useCallback(() => {
    setSearchQuery('');
    setPage(1);
  }, []);

  return (
    <div className="queue-center">
      {/* ── Header ── */}
      <div className="qc-header">
        <div className="qc-header-text">
          <h1 className="qc-title">任务中心</h1>
          <p className="qc-subtitle">统一管理任务保存、单个排期、批量排队、执行状态与结果回看</p>
          <p className="qc-scheduler-hint">
            <Clock3 size={11} /> 有任务时每 30 秒自动检查
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

      {/* ── Lane Strip (双车道状态) ── */}
      <LaneStrip
        laneStatuses={laneStatuses}
        tasks={tasks}
        generationStats={generationStats}
        nowMs={Date.now()}
        onToggleLane={handleToggleLane}
        onProbeLane={handleProbeLane}
        onSelectTask={handleSelectLaneTask}
        assetById={assetById}
        roles={state.roles}
        taskPriorities={state.taskPriorities}
        pendingLaneKind={pendingLaneKind}
        pendingProbeLaneKind={pendingProbeLaneKind}
      />

      {/* ── Two-column body ── */}
      <div className="qc-body-dual">
        {/* ── LEFT: task list ── */}
        <div className="qc-task-list">
          <div className="qc-task-searchbar">
            <label className="qc-task-search">
              <Search size={13} aria-hidden="true" />
              <input
                type="search"
                value={searchQuery}
                onChange={handleSearchChange}
                placeholder="搜索任务名称、提示词或提交 ID"
                aria-label="搜索任务"
              />
              {searchQuery ? (
                <button type="button" onClick={clearSearch} title="清空搜索" aria-label="清空任务搜索">
                  <X size={12} />
                </button>
              ) : null}
            </label>
            <span className="qc-task-search-count">
              {searchQuery.trim() ? `${filteredSorted.length} 个结果` : `共 ${tasks.length} 个`}
            </span>
          </div>
          <div className="qc-task-rows">
            {paged.items.length ? paged.items.map((task, idx) => (
              <TaskCard key={task.id} task={task} index={paged.startIndex + idx + 1}
                selected={task.id === selectedTaskId}
                selectedForBatch={selectedBatchIds.includes(task.id)}
                batchSelectable={canScheduleTask(task) || canDeleteTask(task)}
                queuePriority={Number(state.taskPriorities?.[task.id] || 0)}
                assetById={assetById} roles={state.roles}
                onSelect={setSelectedTaskId}
                onToggleSelection={toggleTaskSelection} />
            )) : (
              <div className="qc-empty">
                {searchQuery.trim() ? <Search size={22} /> : <ClipboardList size={22} />}
                <p>{searchQuery.trim() ? `没有找到“${searchQuery.trim()}”` : '暂无任务，先创建一个'}</p>
                {searchQuery.trim() ? <button type="button" className="qc-mini-link" onClick={clearSearch}>清空搜索</button> : null}
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
              <option value={12}>12 条/页</option>
              <option value={16}>16 条/页</option>
              <option value={24}>24 条/页</option>
            </select>
          </div>
        </div>

        {/* ── RIGHT: actions + selected task panel ── */}
        <div className="qc-detail-column">
          <div className="qc-toolbar">
            <button type="button" className="qc-btn" onClick={toggleSelectPage} disabled={!selectablePagedIds.length}>
              <CheckCircle2 size={13} /> {selectablePagedIds.length && selectablePagedIds.every((id) => selectedBatchIds.includes(id)) ? '取消本页' : '选择本页'}
            </button>
            <button type="button" className="qc-btn" onClick={openBatchSchedule} disabled={!schedulableSelectedTasks.length}>
              <CalendarClock size={13} /> 批量排队{schedulableSelectedTasks.length ? `（${schedulableSelectedTasks.length}）` : ''}
            </button>
            <button type="button" className="qc-btn qc-btn-pause" onClick={pauseSelectedTasks}
              disabled={!pausableSelectedTasks.length || pendingBatchPause}>
              {pendingBatchPause ? <Loader2 size={13} className="spin" /> : <Pause size={13} />}
              {pendingBatchPause ? '暂停中' : `批量暂停${pausableSelectedTasks.length ? `（${pausableSelectedTasks.length}）` : ''}`}
            </button>
            <button type="button" className="qc-btn qc-btn-danger" onClick={deleteSelectedTasks}
              disabled={!deletableSelectedTasks.length || pendingBatchDelete}>
              {pendingBatchDelete ? <Loader2 size={13} className="spin" /> : <Trash2 size={13} />}
              {pendingBatchDelete ? '删除中' : `批量删除${deletableSelectedTasks.length ? `（${deletableSelectedTasks.length}）` : ''}`}
            </button>
          </div>
          <div className="qc-selected">
          {selectedTask ? (
            <div className="qc-selected-body">
              {/* thumbnail + title + note */}
              <div className="qc-selected-top">
                <div className="qc-selected-thumb">
                  {(() => {
                    for (const id of (selectedTask.image_asset_ids || [])) {
                      const a = assetById.get(id);
                      if (a?.stored_path) return <img src={resolveMediaSrc(a.stored_path)} alt="" />;
                    }
                    const role = state.roles.find((r) => r.id === (selectedTask.role_ids || [])[0]);
                    if (role?.asset_ids?.length) {
                      const a = assetById.get(role.asset_ids[0]);
                      if (a?.stored_path) return <img src={resolveMediaSrc(a.stored_path)} alt="" />;
                    }
                    return <Image size={20} />;
                  })()}
                </div>
                <div>
                  <h3 className="qc-selected-title">{selectedTask.title || '未命名任务'}</h3>
                  <p className="qc-selected-note">
                    {selectedTask.params?.ratio || '比例未设置'} · {selectedTask.params?.duration || 15} 秒 · {selectedTask.params?.video_resolution || '720p'} · {selectedRouteInfo.assigned ? `${selectedRouteInfo.label}车道` : selectedRouteInfo.label}
                  </p>
                </div>
                <StatusBadge task={selectedTask} />
              </div>

              <section className="qc-compact-console">
                <div className="qc-console-status-row">
                  <div className="qc-console-current">
                    <span className="qc-console-accent" />
                    <div>
                      <small>当前</small>
                      <b>{selectedNextAction.action || statusLabel(executionView?.status || selectedTask.status)}</b>
                      <p>{selectedNextAction.reason || dispatchInfo.reason || '等待调度器处理'}</p>
                    </div>
                    <span className={`qc-console-health ${detailHealth.tone}`}>{detailHealth.label}</span>
                  </div>
                  <div className="qc-console-metrics">
                    {detailMetrics.map((metric) => (
                      <div key={metric.label} className="qc-console-metric">
                        <span>{metric.label}</span>
                        <b>{metric.value}</b>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="qc-console-tools">
                  <div className="qc-console-priority" role="group" aria-label="排队优先级">
                    <span><Star size={12} /> 排队优先</span>
                    <button type="button" className={selectedQueuePriority === 0 ? 'active' : ''}
                      disabled={pendingPriority} onClick={() => handleSetQueuePriority(0)}>随机</button>
                    <button type="button" className={selectedQueuePriority === 1 ? 'active' : ''}
                      disabled={pendingPriority} onClick={() => handleSetQueuePriority(1)}>★ 第二优先</button>
                    <button type="button" className={selectedQueuePriority === 2 ? 'active' : ''}
                      disabled={pendingPriority} onClick={() => handleSetQueuePriority(2)}>★★ 下一位</button>
                  </div>
                  <div className="qc-console-actions">
                    {selectedTask.status === 'paused' ? (
                      <button type="button" className="qc-btn qc-btn-primary" onClick={() => resumeTask(selectedTask.id, 'immediate')}><Play size={13} /> 立即恢复</button>
                    ) : selectedSubmitId && ['submitted', 'querying'].includes(executionView?.status) ? (
                      <button type="button" className="qc-btn qc-btn-primary" onClick={() => queryTask(selectedTask.id, selectedSubmitId)}
                        disabled={pendingTaskOps[selectedTask.id]?.query}>
                        {pendingTaskOps[selectedTask.id]?.query ? <><Loader2 size={13} className="spin" /> 查询中</> : <><RefreshCcw size={13} /> 查询本次结果</>}
                      </button>
                    ) : executionView?.status === 'succeeded' && resultItems.length ? (
                      <button type="button" className="qc-btn qc-btn-primary" onClick={() => setPreviewResultValue(resultItems[0].value)}><Play size={13} /> 查看结果</button>
                    ) : (
                      <button type="button" className="qc-btn qc-btn-primary" onClick={() => openPrepareGenerate(selectedTask)}
                        disabled={!canScheduleTask(selectedTask) || pendingTaskOps[selectedTask.id]?.submit}>
                        {pendingTaskOps[selectedTask.id]?.submit ? <><Loader2 size={13} className="spin" /> 排队中</> : <><Play size={13} /> 立即排队</>}
                      </button>
                    )}
                    <button type="button" className="qc-btn" onClick={() => handleEditTask(selectedTask)}><Pencil size={13} /> 编辑</button>
                    <button type="button" className="qc-btn" onClick={() => handleDuplicateTask(selectedTask)}><Copy size={13} /> 复制</button>
                    {['scheduled', 'queued', 'retry_wait'].includes(selectedTask.status) ? (
                      <button type="button" className="qc-btn" onClick={() => pauseTask(selectedTask.id)}>暂停</button>
                    ) : null}
                    {selectedTask.status === 'paused' && selectedTask.scheduled_at ? (
                      <button type="button" className="qc-btn" onClick={() => resumeTask(selectedTask.id, 'scheduled')}>按计划恢复</button>
                    ) : null}
                    {selectedTask.status === 'scheduled' ? (
                      <button type="button" className="qc-btn" onClick={() => askConfirm({
                        title: '取消预定', body: '只取消计划时间，任务、执行记录和素材不会被删除。取消后任务回到待生成状态。', confirmText: '取消预定',
                        onConfirm: async () => { await rescheduleTask(selectedTask.id, ''); setFeedback('已取消预定'); },
                      })}><X size={13} /> 取消预定</button>
                    ) : null}
                    <button type="button" className="qc-btn qc-btn-danger" onClick={() => handleDeleteTask(selectedTask)}><Trash2 size={13} /> 删除任务</button>
                  </div>
                </div>
              </section>

              <div className="qc-adaptive-detail">
                {detailSectionOrder.map((sectionKey) => {
                  if (sectionKey === 'timeline') {
                    const allRecordCount = timelineEvents.length + allQueryAttempts.length;
                    return (
                      <section className="qc-process-timeline" key="timeline">
                        <div className="qc-section-inline-head">
                          <h4>过程时间线</h4>
                          {allRecordCount > keyTimelineRecords.length ? (
                            <button type="button" className="qc-mini-link" onClick={() => setRecordsModal({
                              title: selectedTask.title || '未命名任务',
                              events: timelineEvents,
                              queryAttempts: allQueryAttempts,
                            })}>查看全部记录（{allRecordCount}）</button>
                          ) : null}
                        </div>
                        <div className="qc-process-step next">
                          <time>下一步</time>
                          <div><b>{selectedNextAction.action || '等待调度'}</b><span>{selectedNextAction.reason || dispatchInfo.reason || '等待调度器处理'}</span></div>
                        </div>
                        {keyTimelineRecords.map((record) => {
                          if (record.kind === 'event') {
                            return <div className="qc-process-step" key={record.id}><time>{record.event.time}</time><div><b>{record.event.title}</b><span>{record.event.detail}</span></div></div>;
                          }
                          const attempt = record.attempt;
                          const qi = attempt.status === 'querying' ? parseAttemptQueueInfo(attempt.stdout) : null;
                          return (
                            <div className={`qc-process-step ${attempt.status === 'failed' ? 'fail' : ''}`} key={record.id}>
                              <time>{formatDatePart(attempt.finished_at || attempt.started_at, 'time') || '—'}</time>
                              <div><b>查询 {statusLabel(attempt.status)}</b><span>{qi ? `#${qi.queue_idx ?? '-'} / ${qi.queue_length ?? '-'} ${qi.queue_status || ''}` : attempt.error_detail || '等待远端返回'}</span></div>
                            </div>
                          );
                        })}
                      </section>
                    );
                  }
                  if (sectionKey === 'results') {
                    if (!resultItems.length) return null;
                    return (
                      <section className="qc-section qc-results-section" key="results">
                        <h4 className="qc-section-title">生成结果</h4>
                        {resultItems.map((item) => (
                          <div key={`${item.kind}:${item.value}`} className="qc-result-card">
                            {item.kind === 'path' && previewResultValue === item.value ? (
                              <video className="qc-result-video" src={convertFileSrc(item.value)} controls preload="none" />
                            ) : (
                              <div className="qc-result-placeholder">
                                {item.kind === 'url' ? <ExternalLink size={14} /> : <Play size={14} />}
                                <span>{item.kind === 'url' ? '远程视频已就绪，点击链接按需查看' : '视频结果已就绪，按需预览'}</span>
                              </div>
                            )}
                            <div className="qc-result-row">
                              <span className="mono qc-result-path" title={item.value}>{item.label}</span>
                              {item.kind === 'path' ? (
                                <>
                                  <button type="button" className="qc-mini-link qc-result-preview-btn"
                                    onClick={() => setPreviewResultValue((value) => value === item.value ? '' : item.value)}>
                                    {previewResultValue === item.value ? '收起' : '预览'}
                                  </button>
                                  <button type="button" className="icon-ghost mini" title="打开所在目录"
                                    onClick={async () => { try { await invoke('open_result_dir_command', { path: item.value }); } catch (e) { setFeedback(String(e)); } }}><FolderOpen size={12} /></button>
                                </>
                              ) : (
                                <>
                                  <button type="button" className="qc-mini-link qc-result-preview-btn"
                                    onClick={async () => { try { await invoke('open_external_url_command', { url: item.value }); } catch (e) { setFeedback(String(e)); } }}>
                                    打开链接
                                  </button>
                                  <button type="button" className="icon-ghost mini" title="复制链接"
                                    onClick={() => navigator.clipboard?.writeText(item.value).then(() => setFeedback('已复制结果链接')).catch(() => setFeedback('复制失败'))}><Copy size={12} /></button>
                                </>
                              )}
                            </div>
                          </div>
                        ))}
                      </section>
                    );
                  }
                  return (
                    <section className="qc-detail-folds" key="resources">
                      {hitResources.length ? (
                        <div className="qc-detail-static">
                          <div className="qc-detail-static-head"><span>命中资源</span><b>{hitResources.length} 个</b></div>
                          <div className="qc-resource-grid">
                            {hitResources.map(({ type, displayType, label, asset }) => (
                              <button key={`${displayType}:${asset.id}`} type="button" className={`qc-resource-item ${type} ${displayType}`}
                                title={`预览${label}：${asset.name || asset.id.slice(0, 8)}`} onClick={() => setResourcePreview({ type, displayType, asset })}>
                                {type === 'image' && asset.stored_path ? <img src={resolveMediaSrc(asset.stored_path)} alt="" className="qc-resource-thumb" /> : <div className="qc-resource-icon">{type === 'audio' ? <FileAudio size={16} /> : <Image size={16} />}</div>}
                                <span className="qc-resource-tag">{label}</span><span className="qc-resource-name">{asset.name || asset.id.slice(0, 8)}</span>
                              </button>
                            ))}
                          </div>
                        </div>
                      ) : null}
                      {taskHistory.length ? (
                        <details>
                          <summary><span>执行历史</span><b>{taskHistory.length} 次</b></summary>
                          <div className="qc-history-list">
                            {taskHistory.map((item, idx) => (
                              <div key={item.id} className={`qc-history-item${currentExecution?.id === item.id ? ' selected' : ''}`} role="button" tabIndex={0}
                                onClick={() => setSelectedExecutionId(item.id)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setSelectedExecutionId(item.id); } }}>
                                <div className="qc-history-header">
                                  <span className="qc-history-label">{historyItemLabel(item, taskHistory.length - idx)}</span>
                                  <span className={`status-badge ${item.status}`}>{item.status}</span>
                                  {currentExecution?.id === item.id ? <span className="qc-history-current">当前查看</span> : null}
                                  {item.finished_at ? <span className="qc-history-time">{item.finished_at.slice(0, 16).replace('T', ' ')}</span> : null}
                                  <div className="qc-history-item-actions">
                                    {item.submit_id ? <button type="button" className="icon-ghost mini" title={`查询此次结果（${item.submit_id.slice(0, 8)}）`} disabled={pendingExecutionOps[item.id]?.query}
                                      onClick={(event) => { event.stopPropagation(); queryExecutionRecord(selectedTask.id, item.id, item.submit_id); }}>{pendingExecutionOps[item.id]?.query ? <Loader2 size={11} className="spin" /> : <RefreshCcw size={11} />}</button> : null}
                                    <button type="button" className="icon-ghost mini danger" title="删除此条执行记录" onClick={(event) => { event.stopPropagation(); handleDeleteExecutionRecord(selectedTask.id, item.id, historyItemLabel(item, taskHistory.length - idx)); }}><Trash2 size={11} /></button>
                                  </div>
                                </div>
                                {item.error_detail && !isInterruptNotice(item.error_detail) && !item.result_paths.length && !item.result_urls.length ? <p className="qc-error-text" style={{ marginTop: 4 }}>{item.error_detail}</p> : null}
                              </div>
                            ))}
                          </div>
                        </details>
                      ) : null}
                      {commandPresentation.hasCommand ? (
                        <div className="qc-detail-static">
                          <div className="qc-detail-static-head"><span>命令与参数</span><b>完整</b></div>
                          <div className="qc-fold-params">
                            <div><span>模型</span><b>{selectedTask.params?.model_version || '—'}</b></div>
                            {Number(selectedTask.planned_submit_count || 1) > 1 ? <div><span>计划生成</span><b>{selectedTask.planned_submit_count} 次</b></div> : null}
                            {Number(selectedTask.concurrency_retry_count || 0) > 0 ? <div><span>并发重试</span><b>{selectedTask.concurrency_retry_count} 次</b></div> : null}
                          </div>
                          <button type="button" className="qc-command-collapsed" onClick={openCommandPreview}><Command size={13} /><span>{commandPresentation.hint}</span></button>
                        </div>
                      ) : null}
                    </section>
                  );
                })}
              </div>
            </div>
          ) : (
            <div className="qc-empty qc-empty-panel">
              <ClipboardList size={28} />
              <p>选择任务查看详情</p>
            </div>
          )}
          </div>
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
      {recordsModal ? (
        <TaskRecordsModal
          title={recordsModal.title}
          events={recordsModal.events}
          queryAttempts={recordsModal.queryAttempts}
          onClose={() => setRecordsModal(null)}
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

function TaskRecordsModal({ title, events = [], queryAttempts = [], onClose }) {
  return (
    <div className="modal-backdrop command-preview-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="task-records-dialog" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
        <header className="command-preview-dialog-head">
          <div>
            <span>完整记录</span>
            <h3>{title || '未命名任务'}</h3>
          </div>
          <button type="button" className="icon-ghost" onClick={onClose}><X size={16} /></button>
        </header>

        <div className="task-records-dialog-body">
          <section className="task-records-section">
            <div className="task-records-section-head">
              <b>执行记录</b>
              <span>{events.length} 条</span>
            </div>
            {events.length ? (
              <div className="task-records-list">
                {events.map((evt, index) => (
                  <div key={`${evt.time}:${evt.title}:${index}`} className="qc-timeline-event task-records-event">
                    <time>{evt.time}</time>
                    <div>
                      <b>{evt.title}</b>
                      <span>{evt.detail}</span>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <p className="task-records-empty">暂无执行记录</p>
            )}
          </section>

          <section className="task-records-section">
            <div className="task-records-section-head">
              <b>查询记录</b>
              <span>{queryAttempts.length} 条</span>
            </div>
            {queryAttempts.length ? (
              <div className="task-records-list">
                {queryAttempts.map((attempt) => {
                  const qi = attempt.status === 'querying' ? parseAttemptQueueInfo(attempt.stdout) : null;
                  return (
                    <div key={attempt.id} className="qc-recent-query-row compact">
                      <span className={`qc-recent-query-dot ${attempt.status === 'failed' ? 'fail' : attempt.status === 'succeeded' ? 'done' : 'running'}`} />
                      <time>{formatDatePart(attempt.finished_at || attempt.started_at, 'time') || '—'}</time>
                      <b>查询 {statusLabel(attempt.status)}</b>
                      {qi ? <span>#{qi.queue_idx ?? '-'} / {qi.queue_length ?? '-'} {qi.queue_status || ''}</span> : <span>—</span>}
                      {attempt.error_detail ? <span className="qc-recent-query-error">{attempt.error_detail}</span> : null}
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="task-records-empty">暂无查询记录</p>
            )}
          </section>
        </div>

        <footer className="task-records-dialog-actions">
          <button type="button" className="gradient-button" onClick={onClose}>返回</button>
        </footer>
      </section>
    </div>
  );
}


function SchedulePickerModal({ title, mode = 'single', taskCount = 1, onClose, onApply }) {
  const isBatch = mode === 'batch';
  const isPrepare = mode === 'prepare';
  const today = formatDateInputValue(new Date());
  const tomorrowDate = new Date();
  tomorrowDate.setDate(tomorrowDate.getDate() + 1);
  const defaultQuietTime = '02:00';
  const defaultQuietDate = new Date();
  defaultQuietDate.setHours(2, 0, 0, 0);
  const [scheduleMode, setScheduleMode] = useState('immediate');
  const [relativeHours, setRelativeHours] = useState(2);
  const [day, setDay] = useState(defaultQuietDate.getTime() > Date.now() ? 'today' : 'tomorrow');
  const [quietTime, setQuietTime] = useState(defaultQuietTime);
  const [customDate, setCustomDate] = useState(formatDateInputValue(tomorrowDate));
  const [customTime, setCustomTime] = useState('02:00');
  const [intervalMinutes, setIntervalMinutes] = useState(0);
  const [plannedSubmitCount, setPlannedSubmitCount] = useState(1);
  const [error, setError] = useState('');

  const scheduleOptions = [
    {
      key: 'immediate',
      label: '立即排队',
      hint: isBatch ? '清掉预定时间，立刻进入连续队列' : '由调度器自动选择空闲车道',
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
    return null;
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
            <span>{isBatch ? `批量排队 ${taskCount} 个任务` : '任务排队'}</span>
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
            {applying ? <><Loader2 size={14} className="spin" /> 处理中...</> : <><CalendarClock size={14} /> {isBatch ? '确认排队' : isPrepare ? (scheduleMode === 'immediate' ? '立即生成' : '确认定时生成') : '确认安排'}</>}
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
            label="提交后自动查询结果（自适应轮询）"
          />
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
              <input type="number" min="300" value={settingsForm.concurrency_retry_delay_seconds || 300}
                onChange={(e) => setSettingsForm({ ...settingsForm, concurrency_retry_delay_seconds: e.target.value })} />
              <span className="settings-range-hint">300-3600</span>
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
    : ['failed', 'blocked', 'schema_error'].includes(normalized) ? 'fail'
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
            <Field label="系列">
              <input value={roleForm.series} maxLength={80} onChange={(e) => setRoleForm({ ...roleForm, series: e.target.value })} placeholder="例如：两个显眼包、霸总和保镖" />
            </Field>
            <Field label="别名">
              <input value={roleForm.aliases} onChange={(e) => setRoleForm({ ...roleForm, aliases: e.target.value })} placeholder="别名，逗号分隔" />
            </Field>
            <label className="role-disabled-toggle">
              <input
                type="checkbox"
                checked={Boolean(roleForm.disabled)}
                onChange={(e) => setRoleForm({ ...roleForm, disabled: e.target.checked })}
              />
              <span>
                <b>停用角色</b>
                <em>停用后不会出现在资源管理器和 @ 素材选择中。</em>
              </span>
            </label>
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
          <Field label="系列">
            <input value={roleForm.series} maxLength={80} onChange={(e) => setRoleForm({ ...roleForm, series: e.target.value })} placeholder="例如：两个显眼包、霸总和保镖" />
          </Field>
          <Field label="别名">
            <div className="input-with-count">
              <input value={roleForm.aliases} onChange={(e) => setRoleForm({ ...roleForm, aliases: e.target.value })} placeholder="别名，逗号分隔" />
              <span className="field-count">{roleForm.aliases.length}/50</span>
            </div>
          </Field>
          <label className="role-disabled-toggle">
            <input
              type="checkbox"
              checked={Boolean(roleForm.disabled)}
              onChange={(e) => setRoleForm({ ...roleForm, disabled: e.target.checked })}
            />
            <span>
              <b>停用角色</b>
              <em>停用后不会出现在资源管理器和 @ 素材选择中。</em>
            </span>
          </label>
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
    schema_error: '数据异常',
    paused: '已暂停',
  };
  return labels[status] || status;
}

const IMAGE_SIZE_OPTIONS = [
  { value: '1024x1024', label: '1:1 · 1024×1024' },
  { value: '1024x1536', label: '2:3 · 1024×1536' },
  { value: '1536x1024', label: '3:2 · 1536×1024' },
];

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
