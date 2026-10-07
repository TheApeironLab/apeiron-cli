import WebSocket from "ws";
import { mkdir, readFile, writeFile, chmod } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { startInitServer } from "./init/server";

export function gatewayUrl(value: string): URL {
  const url = new URL(value);
  if (
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/" ||
    (url.protocol !== "https:" &&
      !(
        url.protocol === "http:" &&
        ["localhost", "127.0.0.1"].includes(url.hostname)
      ))
  )
    throw new Error(
      "Gateway must be an HTTPS origin (loopback HTTP is allowed for local development).",
    );
  return url;
}
export function validTunnelRequest(
  value: unknown,
): value is {
  type: "request";
  id: string;
  method: "GET" | "POST";
  path: string;
  body: string;
  headers: Record<string, string>;
} {
  if (!value || typeof value !== "object") return false;
  const m = value as Record<string, unknown>;
  return (
    m.type === "request" &&
    typeof m.id === "string" &&
    /^[0-9a-f-]{36}$/.test(m.id) &&
    ["GET", "POST"].includes(String(m.method)) &&
    typeof m.path === "string" &&
    /^(?:|favicon\.ico|api\/[a-z0-9./-]+)$/.test(m.path) &&
    !m.path.includes("..") &&
    typeof m.body === "string" &&
    Buffer.byteLength(m.body) <= 16384
  );
}
interface Connection {
  gateway: string;
  deployment_id: string;
  agent_token: string;
  slug: string;
  domain: string;
}
export async function runRegister(args: string[]): Promise<number> {
  if (args.includes("--help")) {
    console.log(
      "apeiron register --gateway <https://gateway> --enrollment-token <one-time-token>\napeiron register --connection <saved-file>\nKeep this process running while using the remote deployment wizard. Ctrl+C disconnects and stops the wizard.",
    );
    return 0;
  }
  try {
    const flags = new Map<string, string>();
    for (let i = 0; i < args.length; i += 2) {
      const key = args[i]!;
      const value = args[i + 1];
      if (
        !["--gateway", "--enrollment-token", "--connection"].includes(key) ||
        !value ||
        value.startsWith("--") ||
        flags.has(key)
      )
        throw new Error("Invalid arguments; run apeiron register --help.");
      flags.set(key, value);
    }
    let state: Connection;
    let statePath: string;
    if (flags.has("--connection")) {
      if (flags.size !== 1)
        throw new Error("--connection cannot be combined with other flags.");
      statePath = resolve(flags.get("--connection")!);
      state = JSON.parse(await readFile(statePath, "utf8"));
    } else {
      if (flags.size !== 2)
        throw new Error("--gateway and --enrollment-token are required.");
      const gateway = gatewayUrl(flags.get("--gateway")!).origin;
      const token = flags.get("--enrollment-token")!;
      if (!/^[A-Za-z0-9_-]{43}$/.test(token))
        throw new Error("Invalid enrollment token.");
      const response = await fetch(gateway + "/v1/agent/enroll", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token }),
        redirect: "error",
        signal: AbortSignal.timeout(15000),
      });
      if (!response.ok)
        throw new Error(
          "Cannot enroll. Generate a new command in the gateway and retry.",
        );
      state = {
        ...((await response.json()) as Omit<Connection, "gateway">),
        gateway,
      };
      if (!/^[0-9a-f-]{36}$/.test(state.deployment_id))
        throw new Error("Invalid enrollment response.");
      statePath = join(
        homedir(),
        ".apeiron",
        "gateway",
        state.deployment_id + ".json",
      );
      await mkdir(dirname(statePath), { recursive: true, mode: 0o700 });
      await writeFile(statePath, JSON.stringify(state, null, 2), {
        mode: 0o600,
      });
      await chmod(statePath, 0o600);
    }
    gatewayUrl(state.gateway);
    if (
      !/^[0-9a-f-]{36}$/.test(state.deployment_id) ||
      !/^[A-Za-z0-9_-]{43}$/.test(state.agent_token) ||
      !/^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$/.test(state.slug)
    )
      throw new Error("Invalid saved connection.");
    const setup = await startInitServer({
      path: join(dirname(statePath), state.deployment_id + ".setup.json"),
      gateway: { slug: state.slug },
    });
    const local = new URL(setup.url);
    const endpoint = new URL("/v1/agent/tunnel", state.gateway);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    let stopping = false;
    let socket: WebSocket | undefined;
    const stop = () => {
      stopping = true;
      socket?.close();
      void setup.stop();
    };
    process.once("SIGINT", stop);
    process.once("SIGTERM", stop);
    void setup.closed.then(() => {
      stopping = true;
      socket?.close();
    });
    console.log(
      `gateway: ${state.gateway}\nslug: ${state.slug}\nconnection: ${statePath}\nKeep this command running. Open the deployment wizard from your gateway page.`,
    );
    try {
      while (!stopping) {
        await new Promise<void>((resolveSocket) => {
          socket = new WebSocket(endpoint, {
            headers: { Authorization: `Bearer ${state.agent_token}` },
          });
          const current = socket;
          current.onopen = () => console.log("gateway: connected");
          current.onerror = () => {};
          current.onclose = (event) => {
            if (event.code === 4001) {
              console.error("gateway: this connection was replaced");
              stopping = true;
            }
            resolveSocket();
          };
          current.onmessage = async (event) => {
            let message: unknown;
            try {
              message = JSON.parse(String(event.data));
            } catch {
              return;
            }
            if (!validTunnelRequest(message)) return;
            let status = 502;
            let headers: Record<string, string> = {};
            let body = Buffer.from("Setup request failed");
            try {
              const response = await fetch(new URL(message.path, local), {
                method: message.method,
                headers: {
                  Origin: local.origin,
                  "Content-Type": "application/json",
                  "Sec-Fetch-Site": "same-origin",
                },
                body: message.method === "POST" ? message.body : undefined,
                redirect: "error",
                signal: AbortSignal.timeout(290000),
              });
              status = response.status;
              if (
                Number(response.headers.get("content-length") ?? 0) >
                8 * 1024 * 1024
              )
                throw new Error("Response too large");
              body = Buffer.from(await response.arrayBuffer());
              if (body.length > 8 * 1024 * 1024)
                throw new Error("Response too large");
              for (const name of [
                "content-type",
                "content-disposition",
                "content-security-policy",
                "x-content-type-options",
                "referrer-policy",
              ]) {
                const value = response.headers.get(name);
                if (value) headers[name] = value;
              }
            } catch {
              status = 502;
              headers = { "content-type": "text/plain; charset=utf-8" };
              body = Buffer.from("部署向导请求失败、超时或响应超过 8 MiB。");
            }
            if (current.readyState === WebSocket.OPEN)
              current.send(
                JSON.stringify({
                  type: "response",
                  id: message.id,
                  status,
                  headers,
                  body: body.toString("base64"),
                }),
              );
          };
        });
        if (!stopping) {
          console.log("gateway: disconnected; retrying in 3 seconds");
          await Bun.sleep(3000);
        }
      }
    } finally {
      process.removeListener("SIGINT", stop);
      process.removeListener("SIGTERM", stop);
      await setup.stop();
    }
    return 0;
  } catch (error) {
    console.error(
      error instanceof Error ? error.message : "Cannot connect to gateway",
    );
    return 2;
  }
}
