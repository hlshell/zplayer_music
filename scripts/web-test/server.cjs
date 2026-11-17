// scripts/web-test/server.cjs
// 极简静态服务器，带 Slint wasm 需要的 COOP/COEP 头。
// 用法：node server.cjs <root-dir> [port]
// 只依赖 Node 内置模块，零第三方依赖。

const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.resolve(process.argv[2] || "dist");
const port = Number(process.argv[3] || 8899);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
};

const server = http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || "/").split("?")[0]);
  const rel = urlPath === "/" ? "index.html" : urlPath.replace(/^\//, "");
  const file = path.join(root, rel);

  // 必须防止路径穿越
  if (!file.startsWith(path.resolve(root))) {
    res.writeHead(403);
    res.end("forbidden");
    return;
  }

  fs.stat(file, (err, stat) => {
    if (err || !stat.isFile()) {
      res.writeHead(404);
      res.end("not found: " + rel);
      return;
    }
    fs.readFile(file, (e, buf) => {
      if (e) {
        res.writeHead(500);
        res.end("io error");
        return;
      }
      const ext = path.extname(file).toLowerCase();
      res.writeHead(200, {
        "Content-Type": TYPES[ext] || "application/octet-stream",
        // Slint web / wasm 多线程需要这两个头
        "Cross-Origin-Opener-Policy": "same-origin",
        "Cross-Origin-Embedder-Policy": "require-corp",
        "Cache-Control": "no-store",
      });
      res.end(buf);
    });
  });
});

server.listen(port, () => {
  console.log(`[web-test] serving ${root} on http://127.0.0.1:${port}`);
});

module.exports = { server, root, port };
