#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";

async function main() {
const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const chromium = process.env.KERNMUX_CHROMIUM ?? "chromium";
const gatewayBinary = process.env.KERNMUX_GATEWAY_BINARY
  ?? path.join(root, "target/debug/kernmux-gateway");
const assets = process.env.KERNMUX_WEB_ASSETS ?? path.join(root, "dist/web");
const screenshotPath = process.env.KERNMUX_BROWSER_SCREENSHOT;
const token = "browser-smoke-token-0123456789abcdef";
const temporary = await mkdtemp(path.join(os.tmpdir(), "kernmux-browser-smoke-"));
const socketPath = path.join(temporary, "kernmuxd.sock");
const tokenPath = path.join(temporary, "gateway.token");
const profilePath = path.join(temporary, "chromium");
const requests = [];
let deleted = false;
let gateway;
let browser;
let daemon;

try {
  await writeFile(tokenPath, token, { mode: 0o600 });
  daemon = await startFixtureDaemon(socketPath, requests, () => deleted, () => {
    deleted = true;
  });
  const port = await availablePort();
  gateway = spawn(gatewayBinary, [], {
    cwd: root,
    env: {
      ...process.env,
      KERNMUXD_SOCKET: socketPath,
      KERNMUX_GATEWAY_ASSETS: assets,
      KERNMUX_GATEWAY_BIND: `127.0.0.1:${port}`,
      KERNMUX_GATEWAY_ORIGINS: `http://127.0.0.1:${port}`,
      KERNMUX_GATEWAY_TOKEN_FILE: tokenPath,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const gatewayErrors = collectOutput(gateway.stderr);
  await waitForHttp(`http://127.0.0.1:${port}/`, gateway, gatewayErrors);

  const launched = await launchChromium(chromium, profilePath);
  browser = launched.process;
  const cdp = new Cdp(launched.webSocketUrl);
  await cdp.open();
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", {
    targetId,
    flatten: true,
  });
  await Promise.all([
    cdp.send("Page.enable", {}, sessionId),
    cdp.send("Runtime.enable", {}, sessionId),
    cdp.send("Log.enable", {}, sessionId),
    cdp.send("Network.enable", {}, sessionId),
  ]);
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 1280,
    height: 800,
    deviceScaleFactor: 1,
    mobile: false,
  }, sessionId);
  const browserErrors = [];
  const networkResponses = [];
  cdp.on("Runtime.exceptionThrown", sessionId, (event) => {
    const details = event.exceptionDetails;
    browserErrors.push(details?.exception?.description ?? details?.text ?? "uncaught browser exception");
  });
  cdp.on("Log.entryAdded", sessionId, (event) => {
    if (event.entry?.level === "error") browserErrors.push(event.entry.text);
  });
  cdp.on("Runtime.consoleAPICalled", sessionId, (event) => {
    const message = (event.args ?? []).map((arg) => arg.value ?? arg.description ?? "").join(" ");
    if (event.type === "error" || message.includes("panicked")) {
      browserErrors.push(`browser console ${event.type}: ${message}`);
    }
  });
  cdp.on("Network.loadingFailed", sessionId, (event) => {
    if (!event.canceled) browserErrors.push(`network failure: ${event.errorText}`);
  });
  cdp.on("Network.responseReceived", sessionId, (event) => {
    if (event.response.url.includes("/api/")) {
      networkResponses.push({ url: event.response.url, status: event.response.status });
    }
  });

  const loaded = cdp.once("Page.loadEventFired", sessionId);
  await cdp.send("Page.navigate", {
    url: `http://127.0.0.1:${port}/#token=${token}`,
  }, sessionId);
  await loaded;
  try {
    await waitFor(async () => {
      const dimensions = await evaluate(cdp, sessionId, `(() => {
        const canvas = document.querySelector("canvas");
        return canvas && canvas.clientWidth > 0 && canvas.clientHeight > 0
          ? [canvas.clientWidth, canvas.clientHeight]
          : null;
      })()`);
      return dimensions;
    }, "management canvas did not render");
  } catch (error) {
    const documentState = await evaluate(cdp, sessionId,
      "({readyState: document.readyState, body: document.body.innerHTML})");
    throw new Error(`${error.message}; browser errors=${JSON.stringify(browserErrors)}; document=${JSON.stringify(documentState)}`);
  }
  await waitFor(() => networkResponses.filter((response) => response.status === 200).length >= 2,
    `initial API responses did not complete: ${JSON.stringify(networkResponses)}`);
  await delay(500);

  assert.equal(await evaluate(cdp, sessionId, "location.hash"), "");
  assert.equal(await evaluate(cdp, sessionId, "localStorage.length"), 0);
  assert.equal(await evaluate(cdp, sessionId, "sessionStorage.length"), 0);
  const initialDimensions = await evaluate(cdp, sessionId, `(() => {
    const canvas = document.querySelector("canvas");
    return [canvas.clientWidth, canvas.clientHeight, innerWidth, innerHeight];
  })()`);
  assert.equal(initialDimensions[0], initialDimensions[2]);
  assert.equal(initialDimensions[1], initialDimensions[3]);
  assert.deepEqual(initialDimensions, [1280, 800, 1280, 800]);

  if (screenshotPath) {
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" }, sessionId);
    await writeFile(screenshotPath.replace(/\.png$/, "-initial.png"), Buffer.from(data, "base64"));
  }

  if (screenshotPath) {
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" }, sessionId);
    await writeFile(screenshotPath, Buffer.from(data, "base64"));
  }

  // Exercise the authenticated browser-to-gateway mutation path independently
  // of canvas coordinates; component interaction is covered by the UI model.
  const accepted = await evaluate(cdp, sessionId, `(async () => {
    const response = await fetch("/api/1.0/instances/1", {
      method: "DELETE",
      headers: {
        "Authorization": "Bearer ${token}",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ expected_generation: 7 }),
    });
    return { status: response.status, body: await response.json() };
  })()`);
  assert.equal(accepted.status, 202);
  assert.equal(accepted.body.operation.id, "op-delete-1");
  const completed = await evaluate(cdp, sessionId, `(async () => {
    const response = await fetch("/api/1.0/operations/op-delete-1", {
      headers: { "Authorization": "Bearer ${token}" },
    });
    return { status: response.status, body: await response.json() };
  })()`);
  assert.equal(completed.status, 200);
  assert.equal(completed.body.data.state, "succeeded");
  const mutation = requests.find((request) =>
    request.method === "DELETE" && request.path === "/1.0/instances/1");
  assert.deepEqual(JSON.parse(mutation.body), { expected_generation: 7 });

  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 900,
    height: 650,
    deviceScaleFactor: 1,
    mobile: false,
  }, sessionId);
  await waitFor(async () => {
    const dimensions = await evaluate(cdp, sessionId, `(() => {
      const canvas = document.querySelector("canvas");
      return [canvas.clientWidth, canvas.clientHeight, innerWidth, innerHeight];
    })()`);
    return dimensions.every((value, index) => value === [900, 650, 900, 650][index]);
  }, "management canvas did not follow the viewport");

  if (screenshotPath) {
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" }, sessionId);
    await writeFile(screenshotPath, Buffer.from(data, "base64"));
  }
  assert.deepEqual(browserErrors, []);
  await cdp.close();
  console.log(`browser smoke passed (${requests.length} API requests)`);
} finally {
  await terminate(browser);
  await terminate(gateway);
  if (daemon) await new Promise((resolve) => daemon.close(resolve));
  await rm(temporary, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
}

function startFixtureDaemon(socket, observed, isDeleted, markDeleted) {
  return new Promise((resolve, reject) => {
    const server = net.createServer((connection) => {
      let buffer = Buffer.alloc(0);
      connection.on("data", (chunk) => {
        buffer = Buffer.concat([buffer, chunk]);
        const split = buffer.indexOf("\r\n\r\n");
        if (split < 0) return;
        const head = buffer.subarray(0, split).toString("utf8");
        const length = Number(/\r\nContent-Length: (\d+)/i.exec(`\r\n${head}`)?.[1] ?? 0);
        if (buffer.length < split + 4 + length) return;
        const [method, target] = head.split("\r\n", 1)[0].split(" ");
        const body = buffer.subarray(split + 4, split + 4 + length).toString("utf8");
        observed.push({ method, path: target, body });
        const response = fixtureResponse(method, target, isDeleted, markDeleted);
        const encoded = Buffer.from(JSON.stringify(response.body));
        connection.write(
          `HTTP/1.1 ${response.status} ${response.status === 202 ? "Accepted" : "OK"}\r\n`
          + "Content-Type: application/json\r\n"
          + `Content-Length: ${encoded.length}\r\nConnection: close\r\n\r\n`,
        );
        connection.end(encoded);
      });
    });
    server.once("error", reject);
    server.listen(socket, () => resolve(server));
  });
}

function fixtureResponse(method, target, isDeleted, markDeleted) {
  if (method === "GET" && target === "/1.0") {
    return { status: 200, body: result(isDeleted() ? 8 : 7, hostSnapshot(isDeleted())) };
  }
  if (method === "GET" && target === "/1.0/images") {
    return { status: 200, body: result(isDeleted() ? 8 : 7, []) };
  }
  if (method === "DELETE" && target === "/1.0/instances/1") {
    return { status: 202, body: { kind: "accepted", operation: operation("queued") } };
  }
  if (method === "GET" && target === "/1.0/operations/op-delete-1") {
    markDeleted();
    return { status: 200, body: result(8, operation("succeeded")) };
  }
  return {
    status: 404,
    body: { kind: "error", error: { code: "not_found", message: "not found", retryable: false } },
  };
}

function result(generation, data) {
  return { kind: "result", generation, data };
}

function operation(state) {
  return {
    id: "op-delete-1",
    kind: "delete_instance",
    state,
    expected_generation: 7,
    observed_generation: state === "succeeded" ? 8 : undefined,
    affected_resources: [],
    created_at: "2026-08-31T00:00:00Z",
    completed_at: state === "succeeded" ? "2026-08-31T00:00:01Z" : undefined,
  };
}

function hostSnapshot(withoutInstance) {
  return {
    generation: withoutInstance ? 8 : 7,
    health: "healthy",
    kernel: { release: "7.0.0-mk", multikernel_enabled: true },
    capabilities: ["multikernel", "instance_lifecycle"],
    topology: {
      architecture: "x86_64",
      cpus: [0, 1, 2, 3].map((logical_id) => ({
        logical_id,
        hardware_id: logical_id,
        package_id: 0,
        core_id: Math.floor(logical_id / 2),
        thread_index: logical_id % 2,
        numa_node: 0,
        online: true,
      })),
      numa_nodes: [{
        id: 0,
        logical_cpu_ids: [0, 1, 2, 3],
        total_memory_bytes: 17179869184,
        available_memory_bytes: 8589934592,
      }],
    },
    memory: {
      total_bytes: 17179869184,
      host_reserved_bytes: 8589934592,
      assignable_bytes: 8589934592,
      assigned_bytes: withoutInstance ? 0 : 4294967296,
    },
    resource_pool: {
      cpu_hardware_ids: [2, 3],
      available_cpu_hardware_ids: withoutInstance ? [2, 3] : [],
      memory_regions: [],
      devices: [],
      available_device_ids: [],
    },
    instances: withoutInstance ? [] : [{
      id: 1,
      name: "lab",
      generation: 7,
      state: "ready",
      resources: { cpu_hardware_ids: [2, 3], memory_bytes: 4294967296, device_ids: [] },
      image: { present: false },
    }],
    transactions: [],
    operations: [],
  };
}

async function availablePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => server.listen(0, "127.0.0.1", resolve).once("error", reject));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

function collectOutput(stream) {
  let output = "";
  stream?.setEncoding("utf8");
  stream?.on("data", (chunk) => { output += chunk; });
  return () => output;
}

async function waitForHttp(url, child, errors) {
  await waitFor(async () => {
    if (child.exitCode !== null) throw new Error(`gateway exited: ${errors()}`);
    try {
      return (await fetch(url)).ok;
    } catch {
      return false;
    }
  }, "gateway did not become ready");
}

async function launchChromium(binary, profile) {
  const headed = process.env.KERNMUX_BROWSER_HEADED === "1";
  const graphics = headed
    ? ["--ozone-platform=wayland", "--enable-features=Vulkan", "--use-angle=vulkan", "--disable-vulkan-surface"]
    : ["--headless=new", "--use-angle=swiftshader"];
  const child = spawn(binary, [
    ...graphics,
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--window-size=1280,800",
    "--no-first-run",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-sync",
    "--enable-unsafe-webgpu",
    "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });
  child.stderr.setEncoding("utf8");
  return new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(() => reject(new Error(`Chromium did not start: ${output}`)), 15000);
    child.stderr.on("data", (chunk) => {
      output += chunk;
      const match = /DevTools listening on (ws:\/\/[^\s]+)/.exec(output);
      if (match) {
        clearTimeout(timeout);
        resolve({ process: child, webSocketUrl: match[1] });
      }
    });
    child.once("exit", (code) => reject(new Error(`Chromium exited with ${code}: ${output}`)));
  });
}

class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.sequence = 1;
    this.pending = new Map();
    this.listeners = new Map();
  }

  open() {
    return new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
      this.socket.addEventListener("message", (message) => this.message(message));
    });
  }

  send(method, params = {}, sessionId = undefined) {
    const id = this.sequence++;
    this.socket.send(JSON.stringify({ id, method, params, sessionId }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }

  on(method, sessionId, listener) {
    const key = `${sessionId ?? ""}:${method}`;
    const listeners = this.listeners.get(key) ?? [];
    listeners.push(listener);
    this.listeners.set(key, listeners);
  }

  once(method, sessionId) {
    return new Promise((resolve) => {
      const listener = (params) => {
        const key = `${sessionId ?? ""}:${method}`;
        this.listeners.set(key, (this.listeners.get(key) ?? []).filter((item) => item !== listener));
        resolve(params);
      };
      this.on(method, sessionId, listener);
    });
  }

  message(message) {
    const payload = JSON.parse(message.data);
    if (payload.id) {
      const pending = this.pending.get(payload.id);
      this.pending.delete(payload.id);
      if (payload.error) pending?.reject(new Error(payload.error.message));
      else pending?.resolve(payload.result);
      return;
    }
    const key = `${payload.sessionId ?? ""}:${payload.method}`;
    for (const listener of this.listeners.get(key) ?? []) listener(payload.params);
  }

  async close() {
    this.socket.close();
  }
}

async function evaluate(cdp, sessionId, expression) {
  const response = await cdp.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  }, sessionId);
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.text);
  return response.result.value;
}

async function click(cdp, sessionId, x, y) {
  await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 }, sessionId);
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 }, sessionId);
}

async function waitFor(probe, message, timeout = 15000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await probe();
    if (value) return value;
    await delay(50);
  }
  throw new Error(message);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function terminate(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    delay(2000),
  ]);
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
}

await main();
