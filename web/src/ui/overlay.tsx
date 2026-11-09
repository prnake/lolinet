import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { Icon } from "./icons";

export function Drawer({ open, onClose, children, label }: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  label: string;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="drawer" role="dialog" aria-label={label}>
      <div className="drawer__scrim" onClick={onClose} />
      <aside className="drawer__panel">
        <button className="drawer__close" onClick={onClose} aria-label="关闭">
          <Icon name="close" />
        </button>
        {children}
      </aside>
    </div>
  );
}

type Toast = { id: number; text: ReactNode; tone: "plain" | "eye" | "bad" };
const ToastCtx = createContext<(text: ReactNode, tone?: Toast["tone"]) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Toast[]>([]);
  const push = useCallback((text: ReactNode, tone: Toast["tone"] = "plain") => {
    const id = Date.now() + Math.random();
    setItems((xs) => [...xs.slice(-2), { id, text, tone }]);
    setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== id)), 3200);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="toasts" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={`toast toast--${t.tone}`}>
            {t.tone === "eye" && <Icon name="check" size={14} />}
            {t.tone === "bad" && <Icon name="warn" size={14} />}
            {t.text}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

export const useToast = () => useContext(ToastCtx);
