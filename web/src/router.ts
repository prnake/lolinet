import { useEffect, useState } from "react";

// hash 路由：agent 和 nginx 都只需要回 index.html，不用配 history fallback。
export function useRoute(): string[] {
  const [hash, setHash] = useState(location.hash);
  useEffect(() => {
    const f = () => setHash(location.hash);
    window.addEventListener("hashchange", f);
    return () => window.removeEventListener("hashchange", f);
  }, []);
  return (hash.replace(/^#\/?/, "") || "overview").split("/").map(decodeURIComponent);
}

export const go = (path: string) => {
  location.hash = "/" + path;
};
