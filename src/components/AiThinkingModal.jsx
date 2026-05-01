import { useEffect, useRef, useState } from 'react';

const THINKING_MESSAGES = [
  '正在解析提示词语义…',
  '提取关键帧特征…',
  '推导创意意图…',
  '整合上下文信息…',
  'AI 模型推理中…',
  '生成最优结果…',
  '校验输出质量…',
];

export function AiThinkingModal({ open, label = 'AI 处理中', description, error }) {
  const [msgIndex, setMsgIndex] = useState(0);
  const [mounted, setMounted] = useState(false);
  const [particles, setParticles] = useState([]);
  const timerRef = useRef(null);
  const particleRef = useRef(null);

  useEffect(() => {
    if (open) {
      setMounted(true);
      setMsgIndex(0);
      setParticles(
        Array.from({ length: 18 }, (_, i) => ({
          id: i,
          x: Math.random() * 100,
          y: Math.random() * 100,
          size: Math.random() * 2 + 1,
          dur: Math.random() * 3 + 2,
          delay: Math.random() * 2,
        }))
      );
    } else {
      const t = setTimeout(() => setMounted(false), 400);
      return () => clearTimeout(t);
    }
  }, [open]);

  useEffect(() => {
    if (!open) { clearInterval(timerRef.current); return; }
    timerRef.current = setInterval(() => {
      setMsgIndex((i) => (i + 1) % THINKING_MESSAGES.length);
    }, 1400);
    return () => clearInterval(timerRef.current);
  }, [open]);

  if (!mounted) return null;

  return (
    <div className={`ai-modal-overlay${open ? ' open' : ''}`}>
      <div className={`ai-modal-card${error ? ' ai-modal-error' : ''}`}>
        {/* 网格背景 */}
        <div className="ai-modal-grid-bg" />
        {/* 角落装饰 */}
        <span className="ai-modal-corner ai-modal-corner-tl" />
        <span className="ai-modal-corner ai-modal-corner-tr" />
        <span className="ai-modal-corner ai-modal-corner-bl" />
        <span className="ai-modal-corner ai-modal-corner-br" />
        {/* 扫描线 */}
        {!error && <div className="ai-modal-scan" />}
        {/* 粒子 */}
        {!error && (
          <svg className="ai-modal-particles" viewBox="0 0 100 100" preserveAspectRatio="none">
            {particles.map((p) => (
              <circle
                key={p.id}
                cx={p.x}
                cy={p.y}
                r={p.size * 0.5}
                className="ai-modal-particle"
                style={{ animationDuration: `${p.dur}s`, animationDelay: `${p.delay}s` }}
              />
            ))}
          </svg>
        )}

        {/* 中心光球 */}
        <div className={`ai-modal-orb${error ? ' ai-modal-orb-err' : ''}`}>
          <div className="ai-modal-orb-ring r1" />
          <div className="ai-modal-orb-ring r2" />
          <div className="ai-modal-orb-ring r3" />
          <div className="ai-modal-orb-core" />
        </div>

        {/* 文字区 */}
        <div className="ai-modal-label">{error ? '处理失败' : label}</div>

        {error ? (
          <div className="ai-modal-error-text">{error}</div>
        ) : (
          <>
            <div className="ai-modal-msg" key={msgIndex}>{THINKING_MESSAGES[msgIndex]}</div>
            {description ? <div className="ai-modal-desc">{description}</div> : null}
            <div className="ai-modal-dots">
              <span /><span /><span />
            </div>
          </>
        )}
      </div>
    </div>
  );
}
