import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useEditor, EditorContent } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Mention from '@tiptap/extension-mention';
import { ReactRenderer } from '@tiptap/react';
import tippy from 'tippy.js';
import {
  extractMentionRefsFromTiptapDoc,
  getPromptTextFromTiptapEditor,
  promptTextToTiptapDoc,
  shouldBlockPromptTextInput,
  getAllowedPromptPasteText,
  shouldSyncExternalPromptValue,
} from '../prompt-editor-utils.js';
import { collectPromptMentions } from '../mention-utils.js';
import MaterialMentionPicker from './MaterialMentionPicker.jsx';
import { convertFileSrc } from '@tauri-apps/api/core';

/**
 * PromptMentionEditor — Tiptap-based prompt editor with @mention support.
 *
 * Props:
 *   value            — plain text prompt (from taskForm.prompt)
 *   mentionItems     — array from buildMentionItems()
 *   maxLength        — max character count (default 1000)
 *   placeholder      — placeholder text
 *   onUpdate         — (plainText, mentionRefs) => void — single atomic update callback
 *   onPasteImage     — (file: File) => Promise<asset>  — clipboard image upload
 *   onPasteSystemImage — () => Promise<asset>           — system clipboard fallback
 *   tempImagePaths   — current temp_image_paths array (for label generation)
 */
export default function PromptMentionEditor({
  value,
  mentionItems,
  maxLength = 1000,
  placeholder = '',
  onUpdate,
  onPasteImage,
  onPasteSystemImage,
  tempImagePaths,
}) {
  const editorInitializedRef = useRef(false);
  const isInternalUpdateRef = useRef(false);
  const lastSyncedValueRef = useRef(value || '');
  const [promptText, setPromptText] = useState(value || '');
  // Ref so paste handlers always read the latest count without stale closure
  const tempImagePathsRef = useRef(tempImagePaths || []);
  useEffect(() => {
    tempImagePathsRef.current = tempImagePaths || [];
  }, [tempImagePaths]);

  // Ref to track current text length for maxLength blocking in handleTextInput
  const textLengthRef = useRef(0);

  // ── Tiptap editor ──

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        heading: false,
        bulletList: false,
        orderedList: false,
        blockquote: false,
        codeBlock: false,
        horizontalRule: false,
        listItem: false,
      }),
      Mention.extend({
        addAttributes() {
          return {
            ...this.parent?.(),
            type: { default: null },
            roleId: { default: '' },
            assetId: { default: '' },
          };
        },
      }).configure({
        HTMLAttributes: { class: 'prompt-mention-node' },
        renderText({ node }) {
          return `@${node.attrs.label}`;
        },
        renderHTML({ node }) {
          const type = node.attrs.type || 'unknown';
          const suffix = (type === 'image' || type === 'temp_image') ? '[图片]'
            : type === 'audio' ? '[音频]'
            : '';
          return [
            'span',
            {
              class: `prompt-mention-node ${type}`,
              'data-type': type,
              'data-id': node.attrs.id || '',
              'data-role-id': node.attrs.roleId || '',
              'data-asset-id': node.attrs.assetId || '',
            },
            `@${node.attrs.label}${suffix}`,
          ];
        },
        deleteTriggerWithBackspace: true,
        suggestion: {
          char: '@',
          // 返回全部 items，让 MaterialMentionPicker 自行过滤
          items: () => mentionItems,
          render: () => {
            let reactRenderer;
            let popup;
            let latestClientRect = null;
            let getAnchorRect = null;

            return {
              onStart(props) {
                latestClientRect = props.clientRect;
                const editorDom = props.editor.view.dom;
                const getFormPanel = () => editorDom.closest('.create-form-panel') || editorDom.closest('.prompt-editor') || editorDom;
                const getPageBody = () => editorDom.closest('.create-page-body') || getFormPanel();
                const getPickerMetrics = () => {
                  const formPanel = getFormPanel();
                  const pageBody = getPageBody();
                  const fp = formPanel.getBoundingClientRect();
                  const bp = pageBody.getBoundingClientRect();
                  const viewportRight = window.innerWidth - 24;
                  const left = Math.max(24, fp.left);
                  const availableInPage = Math.max(fp.width, bp.right - left);
                  const viewportAvailable = Math.max(720, viewportRight - left);
                  const targetWidth = Math.min(1220, Math.max(1040, availableInPage));
                  const width = Math.min(targetWidth, viewportAvailable);
                  return { left, width };
                };
                getAnchorRect = () => {
                  const { left, width } = getPickerMetrics();
                  const caret = latestClientRect?.();
                  const promptEl = editorDom.closest('.prompt-editor') || editorDom;
                  const fallbackBottom = promptEl.getBoundingClientRect().bottom;
                  const top = caret ? caret.bottom : fallbackBottom;
                  const right = left + width;
                  return { width, height: 0, top, bottom: top, left, right, x: left, y: top, toJSON() { return this; } };
                };
                const panelWidth = getPickerMetrics().width || undefined;

                reactRenderer = new ReactRenderer(MaterialMentionPicker, {
                  props: {
                    mentionItems: props.items,
                    query: props.query,
                    onInsert: (item) => props.command(item),
                    onClose: () => popup?.[0]?.hide(),
                    panelWidth,
                  },
                  editor: props.editor,
                });

                popup = tippy('body', {
                  getReferenceClientRect: getAnchorRect,
                  appendTo: () => document.body,
                  content: reactRenderer.element,
                  showOnCreate: true,
                  interactive: true,
                  trigger: 'manual',
                  placement: 'bottom-start',
                  maxWidth: 'none',
                  offset: [0, 4],
                  popperOptions: { strategy: 'fixed' },
                });
              },
              onUpdate(props) {
                latestClientRect = props.clientRect;
                reactRenderer?.updateProps({
                  mentionItems: props.items,
                  query: props.query,
                });
                if (getAnchorRect) popup?.[0]?.setProps({ getReferenceClientRect: getAnchorRect });
              },
              onKeyDown(props) {
                if (props.event.key === 'Escape') {
                  popup?.[0]?.hide();
                  return true;
                }
                return reactRenderer?.ref?.onKeyDown(props.event) || false;
              },
              onExit() {
                reactRenderer?.destroy();
                popup?.[0]?.destroy();
              },
            };
          },
          command: ({ editor: ed, range, props: item }) => {
            ed.chain().focus().deleteRange(range).run();
            ed.chain().focus().insertContent({
              type: 'mention',
              attrs: {
                id: item.key,
                label: item.label,
                type: item.type,
                roleId: item.roleId || '',
                assetId: item.assetId || '',
              },
            }).run();
            ed.chain().focus().insertContent(' ').run();
          },
        },
      }),
    ],
    content: '',
    editorProps: {
      attributes: {
        class: 'prompt-tiptap-editor',
        'data-placeholder': placeholder,
      },
      handleTextInput: (view, from, to, text) => {
        // Block input that would push plain text length beyond maxLength.
        // Read from the current ProseMirror doc instead of textLengthRef:
        // deleting a mention can otherwise leave the ref stale and make the editor feel locked.
        const doc = view.state.doc;
        const currentLength = doc.textBetween(0, doc.content.size, '\n', '\n').length;
        const selectedLength = from === to ? 0 : doc.textBetween(from, to, '\n', '\n').length;
        return shouldBlockPromptTextInput({ maxLength, currentLength, selectedLength, from, to, text });
      },
      handlePaste: (view, event) => {
        const items = Array.from(event.clipboardData?.items || []);
        const imageItem = items.find(
          (item) => item.kind === 'file' && item.type.startsWith('image/')
        );
        const pastedText = event.clipboardData?.getData('text') || '';

        if (imageItem && onPasteImage) {
          const file = imageItem.getAsFile();
          if (file) {
            event.preventDefault();
            handleImagePaste(file);
            return true;
          }
        }

        if (!imageItem && onPasteSystemImage) {
          const hasText = (event.clipboardData?.getData('text') || '') !== '';
          if (!hasText) {
            event.preventDefault();
            handleSystemImagePaste();
            return true;
          }
        }

        if (pastedText) {
          const { from, to } = view.state.selection;
          const doc = view.state.doc;
          const currentLength = doc.textBetween(0, doc.content.size, '\n', '\n').length;
          const selectedLength = from === to ? 0 : doc.textBetween(from, to, '\n', '\n').length;
          const allowedText = getAllowedPromptPasteText({
            maxLength,
            currentLength,
            selectedLength,
            text: pastedText,
          });
          if (allowedText !== pastedText) {
            event.preventDefault();
            if (allowedText) {
              view.dispatch(view.state.tr.insertText(allowedText, from, to));
            }
            return true;
          }
        }

        return false;
      },
    },
    onUpdate: ({ editor: ed }) => {
      if (isInternalUpdateRef.current) return;
      const json = ed.getJSON();
      const text = getPromptTextFromTiptapEditor(ed);
      lastSyncedValueRef.current = text;
      setPromptText(text);
      textLengthRef.current = text.length;
      const refs = extractMentionRefsFromTiptapDoc(json);
      onUpdate?.(text, refs);
    },
  });

  // ── Initialize content from value prop (once) ──

  useEffect(() => {
    if (!editor || editorInitializedRef.current) return;
    if (value) {
      const doc = promptTextToTiptapDoc(value, mentionItems);
      isInternalUpdateRef.current = true;
      editor.commands.setContent(doc);
      isInternalUpdateRef.current = false;
      lastSyncedValueRef.current = value;
      const text = getPromptTextFromTiptapEditor(editor);
      setPromptText(text);
      textLengthRef.current = text.length;
    }
    editorInitializedRef.current = true;
  }, [editor, value, mentionItems]);

  useEffect(() => {
    if (!editor || !editorInitializedRef.current) return;
    const externalValue = value || '';
    if (!shouldSyncExternalPromptValue({
      editorText: getPromptTextFromTiptapEditor(editor),
      externalValue,
      lastExternalValue: lastSyncedValueRef.current,
      isInternalUpdate: isInternalUpdateRef.current,
    })) {
      return;
    }
    const doc = promptTextToTiptapDoc(externalValue, mentionItems);
    isInternalUpdateRef.current = true;
    editor.commands.setContent(doc);
    isInternalUpdateRef.current = false;
    lastSyncedValueRef.current = externalValue;
    const text = getPromptTextFromTiptapEditor(editor);
    setPromptText(text);
    textLengthRef.current = text.length;
  }, [editor, value, mentionItems]);

  // ── Clipboard image paste handlers ──

  const insertTempImageMention = useCallback((asset) => {
    if (!editor) return;
    const existingLabels = new Set((mentionItems || []).filter((item) => item.type === 'temp_image').map((item) => item.label));
    const label = createUniqueTempImageLabel(existingLabels);
    editor.chain().focus().insertContent({
      type: 'mention',
      attrs: {
        id: `temp:${asset.id}`,
        label,
        type: 'temp_image',
        roleId: '',
        assetId: asset.id,
      },
    }).run();
    editor.chain().focus().insertContent(' ').run();
  }, [editor, mentionItems]);

  const handleImagePaste = useCallback(async (file) => {
    if (!onPasteImage || !editor) return;
    try {
      const asset = await onPasteImage(file);
      if (asset) insertTempImageMention(asset);
    } catch (err) {
      console.error('Clipboard image paste failed:', err);
    }
  }, [onPasteImage, editor, insertTempImageMention]);

  const handleSystemImagePaste = useCallback(async () => {
    if (!onPasteSystemImage || !editor) return;
    try {
      const asset = await onPasteSystemImage();
      if (asset) insertTempImageMention(asset);
    } catch (_err) {
      // System clipboard may not contain image — silent
    }
  }, [onPasteSystemImage, editor, insertTempImageMention]);

  // ── Prompt mentions for matched/unmatched summary tags ──

  const promptMentions = useMemo(
    () => collectPromptMentions(promptText, mentionItems),
    [promptText, mentionItems]
  );

  // ── Render ──

  return (
    <div className="prompt-editor">
      <EditorContent editor={editor} className="prompt-tiptap-editor" />
      <span className="field-count">{promptText.length}/{maxLength}</span>
      {promptMentions.length ? (
        <div className="prompt-mentions">
          {promptMentions.map((m, i) => {
            const suffix = (m.type === 'image' || m.type === 'temp_image') ? '[图片]'
              : m.type === 'audio' ? '[音频]'
              : '';
            return (
              <span key={i} className={`mention-tag ${m.matched ? 'matched' : 'unmatched'}`}>
                {m.text}{suffix}
              </span>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

function createUniqueTempImageLabel(existingLabels = new Set()) {
  for (let i = 0; i < 20; i += 1) {
    const suffix = Math.floor(100000 + Math.random() * 900000);
    const label = `图片${suffix}`;
    if (!existingLabels.has(label)) return label;
  }
  return `图片${Date.now().toString().slice(-6)}`;
}
