// scripts/web-test/smoke.spec.mjs
// ZPlayer Web 冒烟测试：验证 wasm 能加载、init() 能跑完、canvas 有渲染。
// 无框架依赖，`node smoke.spec.mjs` 即跑。退出码 0 = 通过，非 0 = 失败。

import { chromium } from "playwright";

const URL = process.env.WEB_TEST_URL || "http://127.0.0.1:8899/";
const TIMEOUT_MS = Number(process.env.WEB_TEST_TIMEOUT_MS || 15000);

let exitCode = 0;
function report(tag, ok, detail) {
  const sym = ok ? "✅" : "❌";
  console.log(`${sym} [${tag}] ${detail ?? ""}`.trim());
  if (!ok) exitCode = 1;
}

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

const consoleErrors = [];
const pageErrors = [];
const gdResponses = [];   // gdstudio JSONP 响应：{url, status}
const coverResponses = []; // music.126.net 封面图响应：{url, status}

page.on("console", (msg) => {
  if (msg.type() === "error") consoleErrors.push(msg.text());
});
page.on("pageerror", (err) => pageErrors.push(String(err)));
page.on("requestfailed", (req) => {
  consoleErrors.push(`requestfailed: ${req.url()} → ${req.failure()?.errorText || "unknown"}`);
});
// 追踪 gdstudio（JSONP <script>）与封面图 CDN 的响应状态，用于断言数据真的到了
page.on("response", (resp) => {
  const u = resp.url();
  if (u.includes("music-api.gdstudio.xyz")) {
    gdResponses.push({ url: u, status: resp.status() });
  } else if (u.includes("music.126.net") || u.includes("p1.music.126") || u.includes("p2.music.126")) {
    coverResponses.push({ url: u, status: resp.status() });
  }
});

let initErrorMsg = null;
page.on("domcontentloaded", () => {
  // index.html 里失败时会注入红底 div 显示 "初始化失败"
  page
    .locator("text=/初始化失败/")
    .first()
    .isVisible()
    .then((v) => {
      if (v) initErrorMsg = "DOM 上出现了初始化失败提示";
    })
    .catch(() => {});
});

const tStart = Date.now();
try {
  await page.goto(URL, { waitUntil: "networkidle", timeout: TIMEOUT_MS });
} catch (e) {
  report("page.goto", false, `超时或网络错误: ${e.message}`);
  await browser.close();
  process.exit(1);
}
const tElapsed = Date.now() - tStart;

// 等待 canvas 出现（Slint 渲染管线异步，必须等它）
try {
  await page.waitForSelector("canvas", { timeout: TIMEOUT_MS });
  report("canvas 出现", true, `${tElapsed}ms 内可见`);
} catch (e) {
  report("canvas 出现", false, "15s 内未出现 canvas 元素");
}

// 取 canvas 尺寸
const canvasSize = await page.evaluate(() => {
  const c = document.querySelector("canvas");
  if (!c) return null;
  return { w: c.clientWidth, h: c.clientHeight, bw: c.width, bh: c.height };
});
if (canvasSize) {
  const ok = canvasSize.w > 0 && canvasSize.h > 0 && canvasSize.bw > 0 && canvasSize.bh > 0;
  report("canvas 尺寸", ok, JSON.stringify(canvasSize));
} else {
  report("canvas 尺寸", false, "canvas 未找到");
}

// 等待首页数据加载：gdstudio JSONP 至少 1 个 200 响应（证明数据通道打通）
// 首页启动会并发拉 5 个歌单详情 + search("热门")，正常应在数秒内返回。
const dataWaitStart = Date.now();
while (gdResponses.filter((r) => r.status === 200).length === 0 && Date.now() - dataWaitStart < 12000) {
  await new Promise((r) => setTimeout(r, 300));
}
const gdOk = gdResponses.filter((r) => r.status === 200).length;
const gdFail = gdResponses.filter((r) => r.status >= 400 || r.status === 0).length;
report(
  "gdstudio JSONP 数据",
  gdOk > 0,
  gdOk > 0
    ? `${gdOk} 个成功响应（失败 ${gdFail}）`
    : `无成功响应；共收到 ${gdResponses.length} 个，失败 ${gdFail}`
);

// 封面图加载：证明 search → pic → 126.net 全链路打通（软断言，封面失败不阻塞主流程）
const coverOk = coverResponses.filter((r) => r.status === 200).length;
if (coverOk > 0) {
  report("封面图加载", true, `${coverOk} 张封面图成功加载`);
} else {
  console.log(`  ⚠️  封面图未加载到（${coverResponses.length} 次请求），可能首页数据未完整返回`);
}

// DOM 上的初始化失败提示
report("DOM 初始化失败提示", !initErrorMsg, initErrorMsg ?? "无");

// Console 错误：JSONP 已消除 CORS，因此 CORS/ERR_FAILED 现在算真实错误
const realConsoleErrors = consoleErrors.filter((e) => {
  if (e.includes("Using exceptions for control flow")) return false;
  if (e.includes("AbortError")) return false;
  return true;
});
report("console error", realConsoleErrors.length === 0, realConsoleErrors.join(" | ") || "无");

// pageerror（未捕获异常）
report("pageerror", pageErrors.length === 0, pageErrors.join(" | ") || "无");

// 最终确认：canvas 上是否有像素渲染（非全白/非透明）
// Slint web 在 wasm32 端用 WebGL（skia 或 femtovg），不是 2d ctx。
// 用 readPixels 直接从 webgl framebuffer 读；失败再 fallback 2d。
const pixelCheck = await page.evaluate(async () => {
  const c = document.querySelector("canvas");
  if (!c) return { ok: false, reason: "no canvas" };

  // 先试 webgl（Slint wasm 端的主路径）
  let ctx = c.getContext("webgl", { preserveDrawingBuffer: true }) ||
            c.getContext("webgl2", { preserveDrawingBuffer: true });
  if (ctx) {
    const w = c.width || c.clientWidth || 10;
    const h = c.height || c.clientHeight || 10;
    const buf = new Uint8Array(w * h * 4);
    ctx.readPixels(0, 0, w, h, ctx.RGBA, ctx.UNSIGNED_BYTE, buf);
    let nonWhite = 0;
    const n = buf.length / 4;
    for (let i = 0; i < n; i++) {
      const r = buf[i * 4], g = buf[i * 4 + 1], b = buf[i * 4 + 2];
      if (r < 245 || g < 245 || b < 245) nonWhite++;
    }
    return { ok: nonWhite > n * 0.05, nonWhite, total: n, ctxType: ctx instanceof WebGL2RenderingContext ? "webgl2" : "webgl" };
  }

  // fallback：2d ctx
  const c2d = c.getContext("2d");
  if (!c2d) return { ok: false, reason: "no 2d or webgl ctx" };
  const w = c.width || 10, h = c.height || 10;
  const d = c2d.getImageData(0, 0, w, h).data;
  let nonWhite = 0;
  const n = d.length / 4;
  for (let i = 0; i < n; i++) {
    const r = d[i * 4], g = d[i * 4 + 1], b = d[i * 4 + 2];
    if (r < 245 || g < 245 || b < 245) nonWhite++;
  }
  return { ok: nonWhite > n * 0.05, nonWhite, total: n, ctxType: "2d" };
});
report("canvas 有实际渲染", pixelCheck.ok, pixelCheck.ok ? `ctx=${pixelCheck.ctxType} 非白像素 ${pixelCheck.nonWhite}/${pixelCheck.total}` : `几乎全白：${JSON.stringify(pixelCheck)}`);

await browser.close();
console.log(`\n退出码: ${exitCode}`);
process.exit(exitCode);
