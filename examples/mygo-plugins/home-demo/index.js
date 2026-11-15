// home() 返回一个 Browse：
// { title, subtitle, sections: [{ title, tracks: [...], playlists: [...] }] }
// 宿主把 tracks 渲染成可播放的行，把 playlists / albums 渲染成卡片。

export async function home() {
  return {
    title: "首页示例",
    subtitle: "这个页面的内容完全由插件提供",
    sections: [
      {
        title: "编辑推荐",
        tracks: [
          {
            id: "demo-1",
            name: "示例歌曲一",
            artists: [{ name: "示例歌手" }],
            album: { name: "示例专辑" },
            duration: 215,
          },
          {
            id: "demo-2",
            name: "示例歌曲二",
            artists: [{ name: "示例歌手" }],
            album: { name: "示例专辑" },
            duration: 187,
          },
        ],
        playlists: [
          { id: "pl-1", name: "每日推荐", imageUrl: "" },
          { id: "pl-2", name: "华语新歌", imageUrl: "" },
        ],
      },
      {
        title: "新碟上架",
        tracks: [],
        albums: [
          { id: "al-1", name: "示例专辑 A", imageUrl: "" },
          { id: "al-2", name: "示例专辑 B", imageUrl: "" },
        ],
      },
    ],
  };
}

export async function search() {
  return { tracks: [], more: false };
}

export async function stream({ item }) {
  return { url: "https://example.com/" + item.id + ".mp3" };
}
