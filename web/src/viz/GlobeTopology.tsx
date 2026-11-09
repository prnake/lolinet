import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { GlobeInstance } from "globe.gl";
import { Mark, Segmented } from "../ui";
import { landStrips } from "./landStrips";

// 地球上的拓扑（globe.gl）。陆地是点阵（不画国界），节点是 HTML 光标（和平面拓扑同一只），
// 实线弧 = 接入点之间直连，虚线弧 = 设备接入；选中的节点弧线变猫眼色并流动。
// 颜色全从 tokens.css 读，切主题时重读。浏览器没有 WebGL 时显示 fallback（平面拓扑）。

export type GlobeNode = {
  id: string;
  kind: "relay" | "device";
  lat: number;
  lng: number;
  /** CSS 颜色，可以是 var(--swarm-3) */
  color: string;
  label: string;
  sub: string;
  dim?: boolean;
  side?: "left" | "right";
};

export type GlobeArc = {
  a: string;
  b: string;
  kind: "relay" | "device";
  /** 毫秒，决定实线的浓淡 */
  latency: number | null;
  /** 设备的次要连接（不是最近的那个接入点）：平时不画，选中两端之一时才出来 */
  secondary?: boolean;
};

/** 预设视角：地球上节点分布很不均匀（东亚一簇、美西一个），一个视角照顾不全，给几个切换。 */
export type GlobeView = { id: string; label: string; lat: number; lng: number; altitude: number };

type ArcDatum = GlobeArc & { startLat: number; startLng: number; endLat: number; endLng: number; lit: boolean };

const CURSOR = "M3 2.6l14.6 6.1-6.7 1.9-2 6.8z";
const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function palette(el: HTMLElement) {
  const cs = getComputedStyle(el);
  const v = (n: string) => cs.getPropertyValue(n).trim();
  const tint = v("--tint").split(/\s+/).join(",") || "255,255,255";
  const light = document.documentElement.dataset.theme === "light";
  return {
    light,
    tint: (a: number) => `rgba(${tint},${a})`,
    eye: v("--eye-fg") || "#ceff1c",
    globe: light ? "#e9e9e4" : "#0b0b0e",
    atmosphere: light ? "#c9c9c2" : "#6e6e76",
  };
}

const relayAlpha = (ms: number | null) => (ms === null ? 0.18 : ms <= 40 ? 0.62 : ms <= 100 ? 0.42 : ms <= 200 ? 0.26 : 0.16);

export function GlobeTopology({ nodes, arcs, selected, onSelect, fallback, label, views, initialView }: {
  nodes: GlobeNode[];
  arcs: GlobeArc[];
  selected?: string | null;
  onSelect?: (id: string, kind: GlobeNode["kind"]) => void;
  fallback: ReactNode;
  label: string;
  views: GlobeView[];
  /** 初始视角的 id，不给或认不出就用第一个 */
  initialView?: string | null;
}) {
  const host = useRef<HTMLDivElement>(null);
  const globe = useRef<GlobeInstance | null>(null);
  const els = useRef(new Map<string, HTMLButtonElement>());
  const [failed, setFailed] = useState(false);
  const [ready, setReady] = useState(false);
  const start = views.find((v) => v.id === initialView) ?? views[0];
  const [view, setView] = useState(start.id);
  const initial = useRef(start);
  const latest = useRef({ nodes, arcs, selected, onSelect });
  latest.current = { nodes, arcs, selected, onSelect };

  /** 把当前的节点、弧线、选中状态和主题颜色写进 globe。数据轮询、选中、切主题都走这里。 */
  const apply = useCallback(() => {
    const g = globe.current;
    const el = host.current;
    if (!g || !el) return;
    const { nodes, arcs, selected } = latest.current;
    const p = palette(el);
    const still = reduced();

    const mat = g.globeMaterial() as unknown as { color: { set(c: string): void }; emissive?: { set(c: string): void }; emissiveIntensity?: number; shininess?: number };
    mat.color.set(p.globe);
    mat.emissive?.set(p.globe);
    mat.emissiveIntensity = 0.55;
    mat.shininess = 0;
    g.atmosphereColor(p.atmosphere).hexPolygonColor(() => p.tint(p.light ? 0.28 : 0.2));

    const byId = new Map(nodes.map((n) => [n.id, n]));
    const data: ArcDatum[] = arcs.flatMap((a) => {
      const s = byId.get(a.a);
      const e = byId.get(a.b);
      if (!s || !e) return [];
      return [{ ...a, startLat: s.lat, startLng: s.lng, endLat: e.lat, endLng: e.lng, lit: !!selected && (a.a === selected || a.b === selected) }];
    });
    g.arcsData(data)
      .arcColor((d: object) => {
        const a = d as ArcDatum;
        if (a.lit) return p.eye;
        if (a.secondary) return "rgba(0,0,0,0)";
        return a.kind === "device" ? p.tint(0.55) : p.tint(relayAlpha(a.latency));
      })
      .arcStroke((d: object) => ((d as ArcDatum).lit ? 0.55 : (d as ArcDatum).kind === "device" ? 0.22 : 0.32))
      .arcDashLength((d: object) => ((d as ArcDatum).lit ? 0.32 : (d as ArcDatum).kind === "device" ? 0.06 : 1))
      .arcDashGap((d: object) => ((d as ArcDatum).lit ? 0.14 : (d as ArcDatum).kind === "device" ? 0.04 : 0))
      .arcDashAnimateTime((d: object) => ((d as ArcDatum).lit && !still ? 1800 : 0));

    const sel = selected ? byId.get(selected) : undefined;
    g.ringsData(sel && !still ? [sel] : []).ringColor(() => (t: number) => {
      const c = p.eye.startsWith("#") && p.eye.length === 7 ? p.eye : null;
      if (!c) return p.eye;
      const n = parseInt(c.slice(1), 16);
      return `rgba(${n >> 16},${(n >> 8) & 255},${n & 255},${(1 - t) * 0.8})`;
    });

    // HTML 光标：同一个节点复用同一个元素，轮询时只改文字和状态
    for (const n of nodes) {
      let b = els.current.get(n.id);
      if (!b) {
        b = document.createElement("button");
        b.type = "button";
        b.innerHTML = `<svg class="gnode__cursor" width="18" height="18" viewBox="0 0 20 20" aria-hidden="true"><path d="${CURSOR}" stroke="var(--cursor-edge)" stroke-width="1.6" stroke-linejoin="round"/></svg><span class="gnode__text"><span class="gnode__name"></span><span class="gnode__sub"></span></span>`;
        const id = n.id;
        const kind = n.kind;
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          latest.current.onSelect?.(id, kind);
        });
        els.current.set(n.id, b);
      }
      b.className = `gnode gnode--${n.kind} ${n.side === "left" ? "is-left" : ""} ${n.dim ? "is-dim" : ""} ${selected === n.id ? "is-on" : ""}`;
      b.querySelector("path")!.setAttribute("fill", n.color);
      // 文字都用 textContent：设备报上来的型号也不会被当成 HTML
      b.querySelector(".gnode__name")!.textContent = n.label;
      b.querySelector(".gnode__sub")!.textContent = n.sub;
      b.setAttribute("aria-label", `${n.label} ${n.sub}`);
    }
    for (const id of [...els.current.keys()]) if (!byId.has(id)) els.current.delete(id);
    g.htmlElementsData(nodes.map((n) => ({ id: n.id, lat: n.lat, lng: n.lng })));
  }, []);

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const probe = document.createElement("canvas");
    if (!(probe.getContext("webgl2") || probe.getContext("webgl"))) {
      setFailed(true);
      return;
    }
    let dead = false;
    let ro: ResizeObserver | undefined;
    let mo: MutationObserver | undefined;
    (async () => {
      try {
        const [{ default: Globe }, { feature }, land] = await Promise.all([
          import("globe.gl"),
          import("topojson-client"),
          import("world-atlas/land-110m.json"),
        ]);
        if (dead) return;
        const topo = (land as { default?: unknown }).default ?? land;
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const geo = feature(topo as any, (topo as any).objects.land) as unknown as Parameters<typeof landStrips>[0] extends infer F ? { features: F } : never;
        // 亚欧大陆跨 180° 经线，h3 会报错把整层陆地吃掉，先切成 90° 的竖条
        const strips = landStrips(geo.features);
        const g = new Globe(el, { rendererConfig: { antialias: true, alpha: true } })
          .backgroundColor("rgba(0,0,0,0)")
          .width(el.clientWidth)
          .height(el.clientHeight)
          .showAtmosphere(true)
          .atmosphereAltitude(0.13)
          .hexPolygonsData(strips)
          .hexPolygonResolution(3)
          .hexPolygonMargin(0.5)
          .hexPolygonUseDots(true)
          .arcAltitudeAutoScale(0.35)
          .arcsTransitionDuration(0)
          .ringMaxRadius(3.2)
          .ringPropagationSpeed(2.2)
          .ringRepeatPeriod(1300)
          .htmlLat("lat")
          .htmlLng("lng")
          .htmlAltitude(0.004)
          .htmlTransitionDuration(0)
          .htmlElement((d: object) => els.current.get((d as { id: string }).id) ?? document.createElement("span"))
          .htmlElementVisibilityModifier((node: HTMLElement, visible: boolean) => {
            node.style.opacity = visible ? "" : "0";
            node.style.pointerEvents = visible ? "auto" : "none";
          })
          .pointOfView({ ...initial.current, altitude: initial.current.altitude * (el.clientWidth < 700 ? 1.5 : 1) });
        const c = g.controls() as unknown as { enableZoom: boolean; autoRotate: boolean; rotateSpeed: number };
        c.enableZoom = false; // 页面滚动不被地球吃掉
        c.autoRotate = false;
        c.rotateSpeed = 0.55;
        globe.current = g;
        apply();
        setReady(true);
        ro = new ResizeObserver(() => g.width(el.clientWidth).height(el.clientHeight));
        ro.observe(el);
        mo = new MutationObserver(apply);
        mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
      } catch (e) {
        console.error("globe failed, falling back to flat topology", e);
        if (!dead) setFailed(true);
      }
    })();
    return () => {
      dead = true;
      ro?.disconnect();
      mo?.disconnect();
      globe.current?._destructor();
      globe.current = null;
      el.replaceChildren();
    };
  }, [apply]);

  useEffect(() => {
    if (ready) apply();
  }, [ready, nodes, arcs, selected, apply]);

  const fly = (id: string) => {
    setView(id);
    const v = views.find((x) => x.id === id);
    const el = host.current;
    if (!v || !el || !globe.current) return;
    const { lat, lng, altitude } = v;
    globe.current.pointOfView({ lat, lng, altitude: altitude * (el.clientWidth < 700 ? 1.5 : 1) }, reduced() ? 0 : 900);
  };

  if (failed) return <>{fallback}</>;
  return (
    <div className="globe-wrap">
      <div className="globe" ref={host} role="img" aria-label={label} />
      {ready && views.length > 1 && (
        <div className="globe__views">
          <Segmented size="sm" value={view} onChange={fly} options={views.map((v) => ({ value: v.id, label: v.label }))} />
        </div>
      )}
      {!ready && (
        <div className="globe__loading" aria-hidden="true">
          <Mark size={40} state="connecting" />
        </div>
      )}
    </div>
  );
}
