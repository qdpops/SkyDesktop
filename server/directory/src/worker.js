/**
 * Каталог нод SkyDesktop.
 *
 * Ноды регистрируются сами и подтверждают жизнь heartbeat'ом; запись живёт
 * в KV с TTL, поэтому пропавшая нода выпадает из списка без чьего-либо участия.
 * Клиенты читают список анонимно.
 *
 * Через каталог не идёт трафик удалённого доступа: сервер-посредник и реле
 * работают по сырому TCP/UDP, который обычный прокси Cloudflare не проксирует.
 * Здесь только справочник.
 *
 *   POST /register   — регистрация и heartbeat ноды (подпись HMAC)
 *   GET  /nodes      — список для клиентов SkyDesktop
 *   GET  /relays     — список реле (для hbbs)
 */

/** Сколько живёт запись без heartbeat. Минимум KV — 60 секунд. */
const NODE_TTL = 90;

/** Допустимое расхождение часов ноды и каталога. */
const MAX_CLOCK_SKEW = 300;

const ROLES = new Set(["rendezvous", "relay"]);

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method === "POST" && url.pathname === "/register") {
      return register(request, env);
    }
    if (request.method === "GET" && url.pathname === "/nodes") {
      return nodes(env);
    }
    if (request.method === "GET" && url.pathname === "/relays") {
      return relays(env);
    }
    return json({ error: "not found" }, 404);
  },
};

async function register(request, env) {
  const body = await request.text();
  const timestamp = request.headers.get("x-skydesktop-timestamp") ?? "";
  const signature = request.headers.get("x-skydesktop-signature") ?? "";

  const age = Math.abs(Math.floor(Date.now() / 1000) - Number(timestamp));
  if (!Number.isFinite(age) || age > MAX_CLOCK_SKEW) {
    return json({ error: "stale or missing timestamp" }, 401);
  }
  if (!(await verify(env.NODE_SECRET, `${timestamp}.${body}`, signature))) {
    return json({ error: "bad signature" }, 401);
  }

  let payload;
  try {
    payload = JSON.parse(body);
  } catch {
    return json({ error: "bad json" }, 400);
  }

  const role = String(payload.role ?? "");
  const host = String(payload.host ?? "").trim();
  const port = Number(payload.port);
  if (!ROLES.has(role) || !host || !Number.isInteger(port) || port < 1 || port > 65535) {
    return json({ error: "role, host and port are required" }, 400);
  }

  const node = {
    role,
    host,
    port,
    region: String(payload.region ?? ""),
    version: String(payload.version ?? ""),
    seenAt: Math.floor(Date.now() / 1000),
  };

  await env.NODES.put(`node:${role}:${host}:${port}`, JSON.stringify(node), {
    expirationTtl: NODE_TTL,
  });

  return json({ ok: true, ttl: NODE_TTL });
}

async function nodes(env) {
  const [rendezvous, relay] = await Promise.all([
    listRole(env, "rendezvous"),
    listRole(env, "relay"),
  ]);

  return json(
    {
      rendezvous: rendezvous.map(address),
      relay: relay.map(address),
      // Общий публичный ключ пула. Клиент применяет его, только если он задан.
      key: env.POOL_KEY ?? "",
    },
    200,
    // Список меняется медленно, а запросов от клиентов много.
    { "cache-control": "public, max-age=30" },
  );
}

async function relays(env) {
  const list = await listRole(env, "relay");
  return json({ relay: list.map(address) });
}

async function listRole(env, role) {
  const found = [];
  let cursor;
  do {
    const page = await env.NODES.list({ prefix: `node:${role}:`, cursor });
    for (const key of page.keys) {
      const raw = await env.NODES.get(key.name);
      if (raw) {
        try {
          found.push(JSON.parse(raw));
        } catch {
          // Битую запись просто пропускаем: TTL уберёт её сам.
        }
      }
    }
    cursor = page.list_complete ? undefined : page.cursor;
  } while (cursor);

  // Порядок должен быть устойчивым, иначе клиент будет считать список изменённым.
  found.sort((a, b) => address(a).localeCompare(address(b)));
  return found;
}

const address = (node) => `${node.host}:${node.port}`;

async function verify(secret, message, signatureHex) {
  if (!secret || !/^[0-9a-f]+$/i.test(signatureHex)) {
    return false;
  }
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"],
  );
  const signature = Uint8Array.from(
    signatureHex.match(/../g).map((byte) => parseInt(byte, 16)),
  );
  return crypto.subtle.verify("HMAC", key, signature, new TextEncoder().encode(message));
}

function json(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", ...headers },
  });
}
