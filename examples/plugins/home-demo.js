// MusicFree 标准插件示例：首页（getTopLists），不联网。
// 宿主会把每个 list 渲染成一段，data 里的条目渲染成卡片。
module.exports = {
    platform: "HomeDemo",
    version: "0.1.0",
    author: "audacious-rs",
    defaultSearchType: "music",

    async getTopLists() {
        return [
            {
                title: "热门榜单",
                data: [
                    { id: "top-1", title: "华语金曲榜", coverImg: "" },
                    { id: "top-2", title: "欧美热歌榜", coverImg: "" },
                    { id: "top-3", title: "日韩流行榜", coverImg: "" },
                ],
            },
            {
                title: "新歌速递",
                data: [
                    { id: "new-1", title: "本周新歌", coverImg: "" },
                    { id: "new-2", title: "独立音乐", coverImg: "" },
                ],
            },
        ];
    },

    async search(query) {
        return {
            isEnd: true,
            data: [
                {
                    id: "demo-" + encodeURIComponent(query),
                    title: query + "（示例）",
                    artist: "示例歌手",
                    album: "示例专辑",
                    duration: 180,
                    url: "https://example.com/demo.mp3",
                },
            ],
        };
    },
};
