// scripts/web-test/run.js
// 完整流程：构建 wasm → wasm-bindgen → 起静态服务器 → Playwright 冒烟 → 关服务器。
// 用法：node run.js                (默认输出到 scripts/web-test/dist)
//      node run.js /tmp/my-dist   (指定已有 dist 目录，跳过构建/绑定)

import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, existsSync, readFileSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "../..");
const DIST_DIR = process.argv[2] ? resolve(process.argv[2]) : resolve(__dirname, "dist");
const PORT = Number(process.env.WEB_TEST_PORT || 8899);

function runCmd(cmd, args, opts = {}) {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: "inherit", shell: process.platform === "win32", ...opts });
    p.on("exit", (code) => resolve(code));
    p.on("error", (e) => {
      console.error(`failed to spawn ${cmd}:`, e.message);
      resolve(1);
    });
  });
}

async function step(label, fn) {
  console.log(`\n===== ${label} =====`);
  const code = await fn();
  if (code !== 0) {
    console.error(`步骤 [${label}] 失败，退出码 ${code}`);
    process.exit(code);
  }
}

// ---- 1. 构建 wasm + 生成绑定（如果 dist 里没有 zplayer-music.js） ----
const needBuild = !existsSync(join(DIST_DIR, "zplayer-music.js"));
if (needBuild) {
  await step("1/3 cargo build --target wasm32", () =>
    runCmd("cargo", [
      "build", "--release",
      "--target", "wasm32-unknown-unknown",
      "--no-default-features", "--features", "web",
    ], { cwd: REPO_ROOT, timeout: 600_000 })
  );

  mkdirSync(DIST_DIR, { recursive: true });

  await step("2/3 wasm-bindgen 生成绑定", () => {
    const wasmPath = join(REPO_ROOT, "target", "wasm32-unknown-unknown", "release", "zplayer-music.wasm");
    return runCmd("wasm-bindgen", [
      "--target", "web", "--no-typescript",
      "--out-dir", DIST_DIR, wasmPath,
    ]);
  });

  // 拷贝 index.html
  const html = readFileSync(join(REPO_ROOT, "web", "index.html"));
  const htmlPath = join(DIST_DIR, "index.html");
  require("node:fs").writeFileSync(htmlPath, html);
  console.log(`index.html → ${htmlPath}`);
} else {
  console.log(`\n===== 1-2/3 跳过（dist 已存在）=====`);
}

// ---- 3. 起服务器 ----
console.log(`\n===== 3/3 启动静态服务器 (端口 ${PORT}) =====`);
const { server } = await import("./server.cjs");

await new Promise((res) => setTimeout(res, 300)); // 等 server.listen 实际绑定

// ---- 4. 跑冒烟测试 ----
const url = `http://127.0.0.1:${PORT}/`;
console.log(`\n===== 冒烟测试 (${url}) =====`);
process.env.WEB_TEST_URL = url;

try {
  const specPath = join(__dirname, "smoke.spec.mjs");
  const result = spawnSync("node", [specPath], { stdio: "inherit", timeout: 30_000 });
  const smokeExit = result.status ?? 1;
  console.log(`\n冒烟退出码: ${smokeExit}`);
  server.close(() => {});
  process.exit(smokeExit);
} catch (e) {
  console.error("冒烟执行异常:", e.message);
  server.close(() => {});
  process.exit(1);
}
