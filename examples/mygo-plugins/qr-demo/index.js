// 扫码登录接口示例：
//   loginQr()          → { key, qrImage?, qrUrl?, tip? }
//   loginPoll({key})   → { status: waiting|scanned|done|expired|failed, message? }
// 真实插件应该在这里请求自己的二维码接口，并把登录态存进 host.storage。

const QR_IMAGE =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAMgAAADICAIAAAAiOjnJAAADdUlEQVR4nO3dQW7kIBRAwXiU+1+5c4EhEhJPgFO1nGm5neSJxRfGz9clPp/Pf//9eZ70Ord/7y7/dt8A7yQsEsIiISwSwiIhLBLCIvE9+o/RHKVWz2lOmwOt+j2f9veyYpEQFglhkRAWCWGREBYJYZEYzrFGVs2B6vnNafucVu3fqr93ZPZ+rFgkhEVCWCSERUJYJIRFQlgkpudYb3XLPOwWViwSwiIhLBLCIiEsEsIiISwSf26ONTuXWjXfmr2f21mxSAiLhLBICIuEsEgIi4SwSEzPsXadwzSrfi6vPv99lV1/LysWCWGREBYJYZEQFglhkRAWiWs2A9XP/dXznr+2f8uKRUJYJIRFQlgkhEVCWCSEReK5ZX/V7XbNz3axYpEQFglhkRAWCWGREBYJYZFYdj7Wqufpdp2rfsv1d+0bm2XFIiEsEsIiISwSwiIhLBLCIjG9H6t+ru32+6md9lzkiBWLhLBICIuEsEgIi4SwSAiLxHA4Ue9Put1p87aRXfMtKxYJYZEQFglhkRAWCWGREBaJZcOVXc/N3X7O+6z6PYyrvteKRUJYJIRFQlgkhEVCWCSERSI/533XeVez6vnZrFt+DyNWLBLCIiEsEsIiISwSwiIhLBLL5linvY9v1z6tXfvPZtVzOysWCWGREBYJYZEQFglhkRAWien3Fdb7q1bNvU6bV81e55b534gVi4SwSAiLhLBICIuEsEgIi8RwP9bt50iN7Pp5T7u+87G4krBICIuEsEgIi4SwSAiLxGufKzzNW8/TGrFikRAWCWGREBYJYZEQFglhkZh+rnDk9nlV/bzhruvsmpNZsUgIi4SwSAiLhLBICIuEsEictYnnF6edh77rPYy3vFfRikVCWCSERUJYJIRFQlgkhEViuB/rtHPGV31+ds60ax62ivOxeBVhkRAWCWGREBYJYZEQFoll7yucVc/JVj1/t2rf1ejz9XyrnvONWLFICIuEsEgIi4SwSAiLhLBILDsf6zSnnXs+Uu8/G1k1Rxxdx4pFQlgkhEVCWCSERUJYJIRF4vo51q59VKvcct7V7OetWCSERUJYJIRFQlgkhEVCWCSm51invZew3ndVnyO16ntPO3feikVCWCSERUJYJIRFQlgkhEXijofvvvr50GnnsI+cdv8jViwSwiIhLBLCIiEsEsIiISwSP184UZzEuFicAAAAAElFTkSuQmCC";

export async function loginQr() {
  host.storage.set("polls", 0);
  return {
    key: "demo-" + Date.now(),
    qrImage: QR_IMAGE,
    qrUrl: "https://example.com/demo-login",
    tip: "示例二维码（不联网，点「刷新二维码」会重新生成）",
  };
}

export async function loginPoll({ key }) {
  const count = Number(host.storage.get("polls") || 0) + 1;
  host.storage.set("polls", count);
  if (!key) return { status: "expired", message: "缺少 key" };
  if (count >= 4) {
    host.storage.set("session", { token: "demo-token", at: Date.now() });
    return { status: "done", message: "已写入演示登录态" };
  }
  if (count >= 2) return { status: "scanned", message: "已扫码，等待确认…" };
  return { status: "waiting", message: "等待扫码…" };
}

export async function search() {
  return { tracks: [], more: false };
}
