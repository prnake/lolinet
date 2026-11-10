// 16px 线性图标，1.5 描边，currentColor。只画这个控制台用得到的。
import type { SVGProps } from "react";

const paths = {
  overview: "M2.5 2.5h4.5v4.5H2.5zM9 2.5h4.5v4.5H9zM2.5 9h4.5v4.5H2.5zM9 9h4.5v4.5H9z",
  nodes: "M3 2.5l9.5 3.6-4.3 1.2-1.3 4.3z",
  key: "M10 2.5a3.5 3.5 0 1 1-2.9 5.4L2.5 12.5v1h2v-1.5H6V10.5h1.5l.6-.6M10.5 5.5h.01",
  pair: "M2.5 2.5h4v4h-4zM9.5 2.5h4v4h-4zM2.5 9.5h4v4h-4zM9.5 9.5h1.5M13.5 9.5v1.5M9.5 12h1.5v1.5h2.5M12 12v.01",
  design: "M8 2v12M2.8 5l10.4 6M2.8 11l10.4-6",
  copy: "M5.5 5.5V3a.5.5 0 0 1 .5-.5h7a.5.5 0 0 1 .5.5v7a.5.5 0 0 1-.5.5h-2.5M3 5.5h7a.5.5 0 0 1 .5.5v7a.5.5 0 0 1-.5.5H3a.5.5 0 0 1-.5-.5V6a.5.5 0 0 1 .5-.5z",
  check: "M3 8.5l3 3 7-7",
  refresh: "M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v2.6h-2.6",
  chevron: "M6 3.5l4.5 4.5L6 12.5",
  close: "M4 4l8 8M12 4l-8 8",
  search: "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM10.6 10.6L14 14",
  lock: "M4 7.5h8v6H4zM5.5 7.5V5.5a2.5 2.5 0 0 1 5 0v2",
  arrow: "M2.5 8h11M9.5 4l4 4-4 4",
  terminal: "M2.5 3h11v10h-11zM5 6.5l2 1.5-2 1.5M8.5 10h2.5",
  relay: "M8 2.5v11M3 5.5l5-3 5 3M3 10.5l5 3 5-3M3 5.5v5M13 5.5v5",
  device: "M2.5 3.5h11v7h-11zM6 13.5h4M8 10.5v3",
  gateway: "M2.5 8h3M10.5 8h3M5.5 5h5v6h-5z",
  logout: "M6.5 2.5h-4v11h4M10 5l3 3-3 3M13 8H6",
  plus: "M8 3v10M3 8h10",
  external: "M9.5 2.5h4v4M13.5 2.5L7.5 8.5M11.5 9.5v4h-9v-9h4",
  warn: "M8 2.5l6 10.5H2zM8 6.5v3M8 11.2v.01",
  bolt: "M9 1.5L3.5 9H8l-1 5.5L12.5 7H8z",
  sun: "M8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.06 1.06M11.54 11.54l1.06 1.06M3.4 12.6l1.06-1.06M11.54 4.46l1.06-1.06",
  moon: "M13.5 9.5A5.5 5.5 0 1 1 6.5 2.5a4.5 4.5 0 0 0 7 7z",
  auto: "M8 2.5a5.5 5.5 0 1 0 0 11zM8 2.5a5.5 5.5 0 0 1 0 11",
  settings: "M2.5 4.5h5M10.5 4.5h3M2.5 11.5h2M7.5 11.5h6M9 3v3M6 10v3",
} as const;

export type IconName = keyof typeof paths;

export function Icon({ name, size = 16, ...rest }: { name: IconName; size?: number } & SVGProps<SVGSVGElement>) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      <path d={paths[name]} />
    </svg>
  );
}
