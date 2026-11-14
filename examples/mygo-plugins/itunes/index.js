// MyGo 标准示例插件：用 iTunes 公开搜索接口，无需 key。
//
// 导出 search / stream / lyrics 就够了：宿主按名字找得到哪个就用哪个。

const API = "https://itunes.apple.com/search";

function toTrack(item) {
  return {
    id: String(item.trackId ?? item.collectionId ?? item.artistId),
    name: item.trackName || item.collectionName || item.artistName,
    artists: [{ id: String(item.artistId || ""), name: item.artistName || "" }],
    album: {
      id: String(item.collectionId || ""),
      name: item.collectionName || "",
      imageUrl: item.artworkUrl100 || "",
    },
    // mygo 的时长单位是毫秒
    duration: Number(item.trackTimeMillis || 0),
    url: item.trackViewUrl || "",
  };
}

export async function search({ query, kind, offset }) {
  const url =
    `${API}?term=${encodeURIComponent(query)}&entity=song&limit=25` +
    (offset ? `&offset=${offset}` : "");
  const res = await fetch(url);
  if (!res.ok) throw new Error(`iTunes 返回 ${res.status}`);
  const body = await res.json();
  const tracks = (body.results || []).filter((r) => r.previewUrl).map(toTrack);
  // 记下 id → 试听地址，stream 只拿得到 id
  const map = host.storage.get("previews") || {};
  for (const item of tracks) {
    const raw = (body.results || []).find((r) => String(r.trackId) === item.id);
    if (raw && raw.previewUrl) map[item.id] = raw.previewUrl;
  }
  host.storage.set("previews", map);
  return { tracks, more: tracks.length >= 25 };
}

export function stream({ id }) {
  const map = host.storage.get("previews") || {};
  const url = map[id];
  if (!url) throw Object.assign(new Error("没有这个歌曲的试听地址，请重新搜索一次"), { code: "unavailable" });
  return { url, type: "m4a" };
}

export function lyrics() {
  return { lrc: "" };
}
