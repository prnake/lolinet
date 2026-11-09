// 阿里云区域 -> 城市名和坐标。
// x / y：平面拓扑（控制台）的示意坐标，1100×520 画布，不是真实投影：保留南北东西关系，拉开深圳和香港。
// lat / lng：地球（公开探针）上的位置。深圳和香港只隔 30 公里，在地球上会叠在一起，各往外挪了半度左右（示意，不是精确位置）。
// side：标签写在光标左边还是右边，避免压到相邻节点的连线。
export type Region = { city: string; en: string; x: number; y: number; lat: number; lng: number; side: "left" | "right" };

export const REGIONS: Record<string, Region> = {
  "cn-beijing": { city: "北京", en: "BEIJING", x: 430, y: 104, lat: 39.9, lng: 116.4, side: "right" },
  "cn-shanghai": { city: "上海", en: "SHANGHAI", x: 610, y: 214, lat: 31.2, lng: 121.5, side: "right" },
  "cn-shenzhen": { city: "深圳", en: "SHENZHEN", x: 360, y: 322, lat: 23.0, lng: 113.5, side: "left" },
  "cn-hongkong": { city: "香港", en: "HONG KONG", x: 520, y: 372, lat: 21.9, lng: 114.7, side: "right" },
  "ap-southeast-1": { city: "新加坡", en: "SINGAPORE", x: 250, y: 452, lat: 1.35, lng: 103.8, side: "left" },
  "us-west-1": { city: "硅谷", en: "SILICON VALLEY", x: 950, y: 190, lat: 37.4, lng: -121.9, side: "right" },
};

export const regionOf = (code?: string): Region | undefined => (code ? REGIONS[code] : undefined);

/** 设备手动写的位置（probe/device.py --location）能认的城市；认不出就按最近的接入点放。 */
export const CITIES: Record<string, { lat: number; lng: number }> = {
  北京: { lat: 39.9, lng: 116.4 },
  上海: { lat: 31.2, lng: 121.5 },
  杭州: { lat: 30.3, lng: 120.2 },
  南京: { lat: 32.1, lng: 118.8 },
  广州: { lat: 23.1, lng: 113.3 },
  深圳: { lat: 22.5, lng: 114.1 },
  香港: { lat: 22.3, lng: 114.2 },
  成都: { lat: 30.7, lng: 104.1 },
  武汉: { lat: 30.6, lng: 114.3 },
  西安: { lat: 34.3, lng: 108.9 },
  台北: { lat: 25.0, lng: 121.5 },
  东京: { lat: 35.7, lng: 139.7 },
  首尔: { lat: 37.6, lng: 127.0 },
  新加坡: { lat: 1.35, lng: 103.8 },
  硅谷: { lat: 37.4, lng: -121.9 },
  旧金山: { lat: 37.8, lng: -122.4 },
  洛杉矶: { lat: 34.1, lng: -118.2 },
  纽约: { lat: 40.7, lng: -74.0 },
  伦敦: { lat: 51.5, lng: -0.1 },
};
