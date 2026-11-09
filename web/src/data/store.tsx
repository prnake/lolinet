import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { liveBackend, type Backend } from "./api";
import { demoBackend } from "./demo";
import { buildMesh, type Mesh } from "./model";
import type { AgentInfo, Snapshot } from "./types";

const POLL_MS = 3000;

type Phase = "connecting" | "ready" | "offline";

type MeshState = {
  phase: Phase;
  backend: Backend;
  info: AgentInfo | null;
  snap: Snapshot | null;
  mesh: Mesh | null;
  /** 最近一次成功拉到快照的时间（ms） */
  updatedAt: number;
  /** 最近一次拉取失败的原因；成功后清空 */
  error: string | null;
  refresh: () => void;
  enterDemo: () => void;
  retry: () => void;
};

const Ctx = createContext<MeshState | null>(null);

export function MeshProvider({ children }: { children: ReactNode }) {
  const [demo, setDemo] = useState(() => new URLSearchParams(location.search).has("demo"));
  const backend = demo ? demoBackend : liveBackend;
  const [phase, setPhase] = useState<Phase>("connecting");
  const [info, setInfo] = useState<AgentInfo | null>(null);
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [updatedAt, setUpdatedAt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const inflight = useRef(false);

  const pull = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const s = await backend.snapshot();
      setSnap(s);
      setUpdatedAt(Date.now());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      inflight.current = false;
    }
  }, [backend]);

  useEffect(() => {
    let dead = false;
    setPhase("connecting");
    backend
      .info()
      .then(async (i) => {
        if (dead) return;
        setInfo(i);
        await pull();
        if (!dead) setPhase("ready");
      })
      .catch(() => !dead && setPhase("offline"));
    return () => {
      dead = true;
    };
  }, [backend, pull, attempt]);

  useEffect(() => {
    if (phase !== "ready") return;
    const id = setInterval(() => document.visibilityState === "visible" && pull(), POLL_MS);
    return () => clearInterval(id);
  }, [phase, pull]);

  const mesh = useMemo(() => (snap ? buildMesh(snap) : null), [snap]);

  const value: MeshState = {
    phase,
    backend,
    info,
    snap,
    mesh,
    updatedAt,
    error,
    refresh: pull,
    enterDemo: () => {
      history.replaceState(null, "", `${location.pathname}?demo${location.hash}`);
      setDemo(true);
    },
    retry: () => setAttempt((n) => n + 1),
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useMesh() {
  const v = useContext(Ctx);
  if (!v) throw new Error("useMesh outside MeshProvider");
  return v;
}

/** 秒级时钟：倒计时、「3 秒前」之类。 */
export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** 一次性加载 + 手动刷新。 */
export function useLoad<T>(fn: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let dead = false;
    setLoading(true);
    fn()
      .then((d) => !dead && (setData(d), setError(null)))
      .catch((e) => !dead && setError(e))
      .finally(() => !dead && setLoading(false));
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, loading, reload: () => setTick((t) => t + 1), setData };
}
