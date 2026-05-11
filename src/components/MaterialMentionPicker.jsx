import React, { useState, useCallback, useMemo, useEffect, useRef } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { Search, X, Play, Loader2, Check, Copy, Image as ImageIcon } from 'lucide-react';
import {
  normalizeMentionItems,
  deriveRoleChips,
  filterItems,
  moveGridFocus,
  CATEGORIES,
  CATEGORY_LABELS,
} from '../material-mention-picker-utils.js';
import { Waveform } from './media/Waveform.jsx';

const GRID_COLS = 5;

/**
 * MaterialMentionPicker — 大尺寸素材选择器，替代 Tippy 小菜单。
 *
 * Props:
 *   mentionItems     - 来自 buildMentionItems() 的原始数组
 *   query            - Tiptap 当前 @ query（用于初始搜索同步）
 *   onInsert(item)   - 插入回调，item 为 normalizedItem
 *   onClose()        - 关闭回调
 *
 * forwardRef 暴露 { onKeyDown } 给 Tiptap ReactRenderer.ref
 */
const MaterialMentionPicker = React.forwardRef(function MaterialMentionPicker(
  { mentionItems = [], query = '', onInsert, onClose, panelWidth },
  ref,
) {
  const [search, setSearch] = useState(query);
  const [activeCategory, setActiveCategory] = useState('all');
  const [activeRoleId, setActiveRoleId] = useState('all');
  const [selectedKey, setSelectedKey] = useState(null);
  const [playingKey, setPlayingKey] = useState(null);
  const [imgPreview, setImgPreview] = useState(null);
  const audioRef = useRef(null);

  const normalizedItems = useMemo(() => normalizeMentionItems(mentionItems), [mentionItems]);
  const roleChips = useMemo(() => deriveRoleChips(normalizedItems), [normalizedItems]);

  const filtered = useMemo(
    () => filterItems(normalizedItems, { query: search, category: activeCategory, roleId: activeRoleId }),
    [normalizedItems, search, activeCategory, activeRoleId],
  );

  const selectedItem = useMemo(() => {
    if (selectedKey) {
      const found = filtered.find((i) => i.key === selectedKey);
      if (found) return found;
    }
    return filtered[0] || null;
  }, [filtered, selectedKey]);

  // 过滤结果变化时重置选中
  useEffect(() => {
    if (!filtered.find((i) => i.key === selectedKey)) {
      setSelectedKey(filtered[0]?.key || null);
    }
  }, [filtered]);

  // 同步外部 query → 内部 search
  useEffect(() => {
    setSearch(query);
  }, [query]);

  const stopAudio = useCallback(() => {
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current = null;
    }
    setPlayingKey(null);
  }, []);

  useEffect(() => () => stopAudio(), []);

  const toggleAudio = useCallback((item, e) => {
    e?.stopPropagation();
    if (playingKey === item.key) { stopAudio(); return; }
    stopAudio();
    if (!item.storedPath) return;
    const audio = new Audio(convertFileSrc(item.storedPath));
    audio.onended = () => setPlayingKey(null);
    audio.onerror = () => setPlayingKey(null);
    audioRef.current = audio;
    audio.play().catch(() => setPlayingKey(null));
    setPlayingKey(item.key);
  }, [playingKey, stopAudio]);

  const handleInsert = useCallback((item) => {
    if (!item) return;
    stopAudio();
    onInsert?.(item);
  }, [onInsert, stopAudio]);

  const handleClose = useCallback(() => {
    stopAudio();
    onClose?.();
  }, [onClose, stopAudio]);

  // 切换 category 时停止播放并重置 role chip
  const handleCategoryChange = (cat) => {
    stopAudio();
    setActiveCategory(cat);
    setActiveRoleId('all');
  };

  React.useImperativeHandle(ref, () => ({
    onKeyDown: (event) => {
      if (event.key === 'Escape') { handleClose(); return true; }
      if (event.key === 'Enter') {
        if (selectedItem) { handleInsert(selectedItem); return true; }
        return false;
      }
      if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
        const currentIdx = filtered.findIndex((i) => i.key === selectedItem?.key);
        const cols = activeCategory === 'role_audio' ? 1 : GRID_COLS;
        const newIdx = moveGridFocus(currentIdx, event.key, cols, filtered.length);
        if (filtered[newIdx]) setSelectedKey(filtered[newIdx].key);
        return true;
      }
      return false;
    },
  }), [selectedItem, filtered, handleInsert, handleClose, activeCategory]);

  const panelStyle = panelWidth ? { width: Math.min(panelWidth, 1220) } : undefined;

  return (
    <div className="mmp-panel" style={panelStyle} onMouseDown={(e) => e.stopPropagation()}>
      {/* ── 左侧素材区 ── */}
      <div className="mmp-left">
        {/* 顶部搜索 + 提示 */}
        <div className="mmp-top">
          <div className="mmp-search-wrap">
            <Search size={13} className="mmp-search-icon" />
            <input
              className="mmp-search-input"
              placeholder="搜索可引用素材"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              autoFocus
            />
            {search ? (
              <button type="button" className="mmp-search-clear" onClick={() => setSearch('')}>
                <X size={11} />
              </button>
            ) : null}
          </div>
          <span className="mmp-hint">角色不可直接 @，请引用具体图片 / 音频 / 临时图</span>
        </div>

        {/* 一级 tabs */}
        <div className="mmp-tabs" role="tablist">
          {CATEGORIES.map((cat) => (
            <button
              key={cat}
              type="button"
              role="tab"
              aria-selected={activeCategory === cat}
              className={`mmp-tab${activeCategory === cat ? ' active' : ''}`}
              onClick={() => handleCategoryChange(cat)}
            >
              {CATEGORY_LABELS[cat]}
            </button>
          ))}
        </div>

        {/* 二级角色 chips */}
        {roleChips.length > 1 ? (
          <div className="mmp-role-chips">
            {roleChips.map((chip) => (
              <button
                key={chip.id}
                type="button"
                className={`mmp-role-chip${activeRoleId === chip.id ? ' active' : ''}`}
                onClick={() => setActiveRoleId(chip.id)}
              >
                {chip.label}
              </button>
            ))}
          </div>
        ) : null}

        {/* 素材网格 */}
        <div className="mmp-grid-scroll">
          {filtered.length === 0 ? (
            <div className="mmp-empty">
              <span>未找到匹配的素材</span>
              {(search || activeCategory !== 'all' || activeRoleId !== 'all') ? (
                <button
                  type="button"
                  className="mmp-clear-filter"
                  onClick={() => { setSearch(''); setActiveCategory('all'); setActiveRoleId('all'); }}
                >
                  清除筛选
                </button>
              ) : null}
            </div>
          ) : (
            <div className={`mmp-grid${activeCategory === 'role_audio' ? ' mmp-grid-audio' : ''}`}>
              {filtered.map((item) => {
                const isSelected = selectedItem?.key === item.key;
                const isPlaying = playingKey === item.key;

                if (item.displayType === 'role_audio') {
                  return (
                    <div
                      key={item.key}
                      className={`mmp-audio-card${isSelected ? ' selected' : ''}`}
                      onClick={() => setSelectedKey(item.key)}
                      onDoubleClick={() => handleInsert(item)}
                    >
                      <div className="mmp-audio-card-top">
                        <button
                          type="button"
                          className={`play-round small${isPlaying ? ' playing' : ''}`}
                          onClick={(e) => toggleAudio(item, e)}
                          disabled={!item.storedPath}
                          title={isPlaying ? '暂停' : '试听'}
                        >
                          {isPlaying ? <Loader2 size={11} className="spin" /> : <Play size={11} />}
                        </button>
                        <Waveform active={isPlaying} />
                        {item.durationSeconds ? (
                          <em className="mmp-card-duration">{Math.round(item.durationSeconds)}s</em>
                        ) : null}
                      </div>
                      <div className="mmp-audio-card-bottom">
                        <span className="mmp-card-label" title={item.label}>{item.label}</span>
                        <span className="mmp-card-insert-hint">{item.insertText}</span>
                        <div className="mmp-audio-dur-row">
                          {item.roleName ? <span className="mmp-card-role">{item.roleName}</span> : null}
                          <span className="mmp-card-tag role_audio">音频</span>
                        </div>
                      </div>
                      {isSelected ? <Check size={9} className="mmp-selected-check" /> : null}
                    </div>
                  );
                }

                return (
                  <div
                    key={item.key}
                    className={`mmp-img-card${isSelected ? ' selected' : ''}`}
                    onClick={() => setSelectedKey(item.key)}
                    onDoubleClick={() => handleInsert(item)}
                    title={`双击插入 ${item.insertText}`}
                  >
                    {isSelected ? (
                      <span className="mmp-check-mark"><Check size={9} /></span>
                    ) : null}
                    <div className="mmp-img-thumb">
                      {item.storedPath ? (
                        <img
                          src={convertFileSrc(item.storedPath)}
                          alt={item.label}
                          onError={(e) => { e.currentTarget.style.display = 'none'; }}
                        />
                      ) : (
                        <div className="mmp-img-placeholder"><ImageIcon size={18} /></div>
                      )}
                    </div>
                    <div className="mmp-card-footer">
                      <span className="mmp-card-label" title={item.label}>{item.label}</span>
                      <span className="mmp-card-insert-hint">{item.insertText}</span>
                      <div className="mmp-card-footer-row">
                        {item.roleName ? <span className="mmp-card-role-name">{item.roleName}</span> : null}
                        <span className={`mmp-card-tag ${item.displayType}`}>
                          {item.displayType === 'temp_image' ? '临时图' : '图片'}
                        </span>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* 底部说明 */}
        <div className="mmp-bottom-hint">
          支持 @具体图片、@具体音频、@图片；未命中 @ 引用不会自动附加素材
        </div>
      </div>

      {/* ── 右侧详情栏 ── */}
      <div className="mmp-detail">
        <button type="button" className="mmp-close-btn" onClick={handleClose} title="关闭">
          <X size={15} />
        </button>
        {selectedItem ? (
          <DetailPanel
            item={selectedItem}
            isPlaying={playingKey === selectedItem.key}
            onToggleAudio={toggleAudio}
            onInsert={handleInsert}
            onPreviewImage={setImgPreview}
          />
        ) : (
          <div className="mmp-detail-empty">选择左侧素材<br />查看详情</div>
        )}
      </div>

      {/* 图片大预览 */}
      {imgPreview ? (
        <div
          className="modal-backdrop image-modal-backdrop"
          role="presentation"
          onMouseDown={() => setImgPreview(null)}
        >
          <div className="image-modal-content" onMouseDown={(e) => e.stopPropagation()}>
            <button type="button" className="image-modal-close" onClick={() => setImgPreview(null)}>
              <X size={18} />
            </button>
            <img src={imgPreview} alt="" className="image-modal-img" />
          </div>
        </div>
      ) : null}
    </div>
  );
});

function mimeToFormat(mime) {
  if (!mime) return '';
  const m = mime.toLowerCase();
  if (m.includes('jpeg') || m.includes('jpg')) return 'JPG';
  if (m.includes('png')) return 'PNG';
  if (m.includes('webp')) return 'WebP';
  if (m.includes('mp3') || m.includes('mpeg')) return 'MP3';
  if (m.includes('wav')) return 'WAV';
  if (m.includes('aac')) return 'AAC';
  return mime.split('/').pop().toUpperCase();
}

function formatAssetDate(isoStr) {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function DetailPanel({ item, isPlaying, onToggleAudio, onInsert, onPreviewImage }) {
  const [copied, setCopied] = useState(false);
  const [imgSize, setImgSize] = useState(null);

  useEffect(() => { setImgSize(null); }, [item.key]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(item.insertText);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (_) {}
  };

  return (
    <div className="mmp-detail-content">
      {/* 预览区 */}
      <div className="mmp-detail-preview">
        {item.displayType === 'role_audio' ? (
          <div className="mmp-detail-audio-preview">
            <button
              type="button"
              className={`play-round large${isPlaying ? ' playing' : ''}`}
              onClick={(e) => onToggleAudio(item, e)}
              disabled={!item.storedPath}
              title={isPlaying ? '暂停' : '播放'}
            >
              {isPlaying ? <Loader2 size={18} className="spin" /> : <Play size={18} />}
            </button>
            <Waveform active={isPlaying} />
          </div>
        ) : (
          <div
            className="mmp-detail-img-preview"
            onClick={() => item.storedPath ? onPreviewImage(convertFileSrc(item.storedPath)) : null}
            style={{ cursor: item.storedPath ? 'zoom-in' : 'default' }}
            title={item.storedPath ? '点击放大预览' : ''}
          >
            {item.storedPath ? (
              <img
                src={convertFileSrc(item.storedPath)}
                alt={item.label}
                onLoad={(e) => setImgSize({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
              />
            ) : (
              <div className="mmp-img-placeholder large"><ImageIcon size={28} /></div>
            )}
          </div>
        )}
      </div>

      {/* 元信息 */}
      <div className="mmp-detail-meta">
        <h4 className="mmp-detail-title" title={item.label}>{item.label}</h4>
        <div className="mmp-detail-tags">
          <span className={`mmp-card-tag ${item.displayType}`}>
            {item.displayType === 'role_image' ? '图片'
              : item.displayType === 'role_audio' ? '音频'
                : '临时图'}
          </span>
        </div>
        <dl className="mmp-meta-table">
          {item.roleName ? <><dt>角色</dt><dd>{item.roleName}</dd></> : null}
          {imgSize ? <><dt>分辨率</dt><dd>{imgSize.w} × {imgSize.h}</dd></> : null}
          {item.mime && item.displayType !== 'role_audio' ? <><dt>格式</dt><dd>{mimeToFormat(item.mime)}</dd></> : null}
          {item.durationSeconds ? <><dt>时长</dt><dd>{Math.round(item.durationSeconds)}s</dd></> : null}
          {item.createdAt ? <><dt>创建时间</dt><dd>{formatAssetDate(item.createdAt)}</dd></> : null}
          {item.sourceHint ? <><dt>来源</dt><dd>{item.sourceHint}</dd></> : null}
        </dl>
      </div>

      {/* 可插入文本 */}
      <div className="mmp-insert-text-block">
        <span className="mmp-insert-text-label">可插入文本</span>
        <div className="mmp-insert-text-row">
          <code className="mmp-insert-text">{item.insertText}</code>
          <button
            type="button"
            className="icon-ghost mini"
            title={copied ? '已复制' : '复制'}
            onClick={handleCopy}
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
          </button>
        </div>
      </div>

      {/* 插入按钮 */}
      <button type="button" className="mmp-insert-btn" onClick={() => onInsert(item)}>
        插入 {item.insertText}
      </button>
      <p className="mmp-detail-footnote">仅插入具体资源，不插入角色本身</p>
    </div>
  );
}

export default MaterialMentionPicker;
