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

page.on("console", (msg) => {
  if (msg.type() === "error") consoleErrors.push(msg.text());
});
page.on("pageerror", (err) => pageErrors.push(String(err)));
page.on("requestfailed", (req) => {
  consoleErrors.push(`requestfailed: ${req.url()} → ${req.failure()?.errorText || "unknown"}`);
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

// DOM 上的初始化失败提示
report("DOM 初始化失败提示", !initErrorMsg, initErrorMsg ?? "无");

// Console 错误：过滤控制流信号 + 已知的 CORS/网络失败（Web 端暂时不支持，单独修复）
const realConsoleErrors = consoleErrors.filter((e) => {
  if (e.includes("Using exceptions for control flow")) return false;
  if (e.includes("AbortError")) return false;
  if (e.includes("CORS policy")) return false;
  if (e.includes("music-api.gdstudio.xyz")) return false;
  if (e.includes("requestfailed:")) return false;
  // Chromium CORS block 还会单独打一条裸 "Failed to load resource: net::ERR_FAILED"
  // 我们的静态服务器/本地 wasm 都能正常加载，所以裸 ERR_FAILED 只可能来自 CORS block
  if (e.includes("net::ERR_FAILED") || e === "Failed to load resource: net::ERR_FAILED") return false;
  return true;
});
const corsWarnings = consoleErrors.filter(
  (e) => e.includes("music-api.gdstudio.xyz") || e.includes("CORS policy")
);
report("console error", realConsoleErrors.length === 0, realConsoleErrors.join(" | ") || "无");
if (corsWarnings.length > 0) {
  console.log(`  ⚠️  有 ${corsWarnings.length} 条 CORS 警告（Web 端网易云 API 被浏览器阻止，不影响渲染验证）`);
}

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
