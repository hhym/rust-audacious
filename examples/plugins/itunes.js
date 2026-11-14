// MusicFree 标准插件示例：iTunes 搜索（公开接口，无需 key）。
// 放进 <配置目录>/audacious-rs/plugins/ 即可在“在线搜索”里使用。
const axios = require("axios");

module.exports = {
    platform: "iTunes",
    version: "0.1.0",
    author: "audacious-rs",
    defaultSearchType: "music",

    async search(query, page, type) {
        if (type !== "music") {
            return { isEnd: true, data: [] };
        }
        const limit = 25;
        const resp = await axios.get("https://itunes.apple.com/search", {
            params: {
                term: query,
                media: "music",
                entity: "song",
                limit: limit,
                offset: (page > 0 ? page - 1 : 0) * limit,
            },
            timeout: 20000,
        });
        const results = (resp.data && resp.data.results) || [];
        const data = results
            .filter(function (r) { return r.previewUrl; })
            .map(function (r) {
                return {
                    id: String(r.trackId),
                    title: r.trackName,
                    artist: r.artistName,
                    album: r.collectionName || "",
                    artwork: String(r.artworkUrl100 || "").replace("100x100", "300x300"),
                    duration: r.trackTimeMillis ? r.trackTimeMillis / 1000 : undefined,
                    url: r.previewUrl,
                };
            });
        return { isEnd: true, data: data };
    },

    async getMediaSource(item) {
        // iTunes 的搜索结果里已经带 previewUrl，直接复用
        return { url: item.url };
    },
};
