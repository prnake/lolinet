// 陆地多边形切条：给 globe.gl 的点阵陆地（h3 polygonToCells）用。
// world-atlas 的亚欧非大陆过了 180° 经线（楚科奇），原始坐标横跨 -180..180，h3 直接报错，
// three-globe 不接这个错，整层陆地都画不出来。这里先把经度展开成连续的，再按 90° 宽的竖条裁开
// （每条都远小于 180°），裁完挪回 [-180, 180]。南极不画。

type Ring = [number, number][];
type Polygon = Ring[];
type Feature = { type: "Feature"; properties: object; geometry: { type: "Polygon"; coordinates: Polygon } };

/** 经度展开：相邻两点跳了 180° 以上就加减 360，让环在平面上连续。 */
function unwrap(ring: Ring): Ring {
  const out: Ring = [];
  let shift = 0;
  ring.forEach(([lng, lat], i) => {
    if (i > 0) {
      const prev = ring[i - 1][0] + shift;
      if (lng + shift - prev > 180) shift -= 360;
      else if (lng + shift - prev < -180) shift += 360;
    }
    out.push([lng + shift, lat]);
  });
  return out;
}

/** Sutherland–Hodgman：只留 lo <= lng <= hi 的部分。 */
function clip(ring: Ring, lo: number, hi: number): Ring {
  let pts = ring;
  for (const [edge, keep] of [
    [lo, (x: number) => x >= lo],
    [hi, (x: number) => x <= hi],
  ] as const) {
    const out: Ring = [];
    for (let i = 0; i < pts.length; i++) {
      const a = pts[i];
      const b = pts[(i + 1) % pts.length];
      const ina = keep(a[0]);
      const inb = keep(b[0]);
      if (ina) out.push(a);
      if (ina !== inb) {
        const t = (edge - a[0]) / (b[0] - a[0]);
        out.push([edge, a[1] + t * (b[1] - a[1])]);
      }
    }
    pts = out;
  }
  return pts;
}

export function landStrips(features: { geometry: { type: string; coordinates: unknown } }[]): Feature[] {
  const polys = features.flatMap((f) =>
    f.geometry.type === "MultiPolygon" ? (f.geometry.coordinates as Polygon[]) : [f.geometry.coordinates as Polygon],
  );
  const out: Feature[] = [];
  for (const poly of polys) {
    if (Math.max(...poly[0].map((p) => p[1])) < -60) continue; // 南极
    const rings = poly.map(unwrap);
    const lngs = rings[0].map((p) => p[0]);
    const min = Math.min(...lngs);
    const max = Math.max(...lngs);
    for (let lo = Math.floor(min / 90) * 90; lo < max; lo += 90) {
      const outer = clip(rings[0], lo, lo + 90);
      if (outer.length < 3) continue;
      const holes = rings.slice(1).map((r) => clip(r, lo, lo + 90)).filter((r) => r.length >= 3);
      // 挪回 [-180, 180]：整条一起挪，条内仍然连续
      const off = lo + 45 > 180 ? -360 : lo + 45 < -180 ? 360 : 0;
      const move = (r: Ring): Ring => {
        const m = r.map(([x, y]) => [x + off, y] as [number, number]);
        m.push(m[0]); // GeoJSON 环首尾相同
        return m;
      };
      out.push({ type: "Feature", properties: {}, geometry: { type: "Polygon", coordinates: [move(outer), ...holes.map(move)] } });
    }
  }
  return out;
}
