// scripts/web-test/server.cjs
// 极简静态服务器（零第三方依赖），为 ZPlayer wasm 提供正确的 .wasm Content-Type。
// 用法：node server.cjs <root-dir> [port]
// 只依赖 Node 内置模块，零第三方依赖。

const http = require("http");
const https = require("https");
const { URL } = require("url");
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

// 转发目标 URL 的代理：服务端直连，彻底绕开浏览器 CORS / ORB 限制。
// 用法：GET /proxy?url=<encodeURIComponent(target)>
function proxyHandler(req, res) {
  const u = new URL(req.url, "http://localhost");
  const target = u.searchParams.get("url");
  if (!target) {
    res.writeHead(400, { "Access-Control-Allow-Origin": "*" });
    res.end("missing url");
    return;
  }
  let parsed;
  try {
    parsed = new URL(target);
  } catch (e) {
    res.writeHead(400, { "Access-Control-Allow-Origin": "*" });
    res.end("bad url");
    return;
  }
  // 只允许 http/https，且只转发 gdstudio 与封面图 CDN（防滥用）
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    res.writeHead(403, { "Access-Control-Allow-Origin": "*" });
    res.end("bad protocol");
    return;
  }
  const lib = parsed.protocol === "https:" ? https : http;
  const opts = {
    method: req.method,
    hostname: parsed.hostname,
    port: parsed.port || (parsed.protocol === "https:" ? 443 : 80),
    path: parsed.pathname + parsed.search,
    headers: { "User-Agent": "Mozilla/5.0 ZPlayer-WebTest/1.0" },
  };
  const upstream = lib.request(opts, (upRes) => {
    const chunks = [];
    upRes.on("data", (c) => chunks.push(c));
    upRes.on("end", () => {
      const buf = Buffer.concat(chunks);
      const ct = upRes.headers["content-type"] || "application/octet-stream";
      res.writeHead(upRes.statusCode || 200, {
        "Content-Type": ct,
        "Access-Control-Allow-Origin": "*",
        "Cache-Control": "no-store",
      });
      res.end(buf);
    });
  });
  upstream.on("error", (e) => {
    res.writeHead(502, { "Access-Control-Allow-Origin": "*" });
    res.end("proxy error: " + e.message);
  });
  upstream.setTimeout(20000, () => {
    upstream.destroy(new Error("timeout"));
  });
  upstream.end();
}

const server = http.createServer((req, res) => {
  // CORS 预检
  if (req.method === "OPTIONS") {
    res.writeHead(204, {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "*",
    });
    res.end();
    return;
  }

  const urlPath = (req.url || "/").split("?")[0];

  // 代理端点（服务端转发，绕开浏览器 CORS/ORB）
  if (urlPath === "/proxy") {
    proxyHandler(req, res);
    return;
  }

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
