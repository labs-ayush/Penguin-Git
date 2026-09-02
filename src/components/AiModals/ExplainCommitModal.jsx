import { useState, useEffect } from "react";
import ReactMarkdown from "react-markdown";
import DOMPurify from "dompurify";
import * as git from "../../services/tauriBridge";
import { useRepoStore } from "../../store/repoStore";
import "./AiModals.css";

export default function ExplainCommitModal({ hash, onClose }) {
  const activeRepoId = useRepoStore((s) => s.activeRepoId);
  const slice = useRepoStore((s) => s.repos[activeRepoId]);
  const repo = slice?.repo;

  const [explanation, setExplanation] = useState("");
  const [loading, setLoading] = useState(true);
  const [isCached, setIsCached] = useState(false);
  const [error, setError] = useState(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!hash || !repo) return;
    let cancelled = false;

    const fetchExplanation = async () => {
      setLoading(true);
      setError(null);
      const cacheKey = `penguingit_explain_commit_${hash}`;

      // Check localStorage cache first
      const cached = localStorage.getItem(cacheKey);
      if (cached) {
        setExplanation(cached);
        setIsCached(true);
        setLoading(false);
        return;
      }

      try {
        const result = await git.aiExplainCommit(repo.path, hash);
        if (!cancelled) {
          localStorage.setItem(cacheKey, result);
          setExplanation(result);
          setIsCached(false);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err.message || String(err));
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    fetchExplanation();

    return () => {
      cancelled = true;
    };
  }, [hash, repo]);

  if (!hash) return null;

  const handleCopy = () => {
    navigator.clipboard.writeText(explanation);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="ai-modal-overlay" onClick={onClose}>
      <div className="ai-modal" onClick={(e) => e.stopPropagation()}>
        <div className="ai-modal-header">
          <h3>
            <span>🔍 Commit Explanation ({hash.slice(0, 7)})</span>
            {isCached && <span className="cache-badge">⚡ Cache Hit</span>}
          </h3>
          <button className="settings-close-btn" onClick={onClose}>
            ✕
          </button>
        </div>

        <div className="ai-modal-body">
          {loading ? (
            <div>Analyzing commit diff with AI…</div>
          ) : error ? (
            <div style={{ color: "var(--accent-red, #f87171)" }}>Error: {error}</div>
          ) : (
            <div className="ai-modal-markdown">
              <ReactMarkdown>{DOMPurify.sanitize(explanation)}</ReactMarkdown>
            </div>
          )}
        </div>

        <div className="ai-modal-footer">
          {copied && <span className="copy-toast">✓ Copied to clipboard</span>}
          <button
            type="button"
            className="btn-secondary"
            disabled={loading || !explanation}
            onClick={handleCopy}
          >
            Copy
          </button>
          <button type="button" className="btn-primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
