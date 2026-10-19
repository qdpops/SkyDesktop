// Каталог нод без Cloudflare: тот же API, что у Worker'а, но состояние
// в памяти. Годится и для локальной разработки, и для self-hosted пула —
// список восстанавливается из heartbeat'ов нод за полминуты после перезапуска.
//
//   NODE_SECRET=dev PORT=8787 node server/directory/dev-server.mjs
//
// Проверка регистрации:
//   DIRECTORY=http://127.0.0.1:8787 NODE_SECRET=dev ROLE=rendezvous \
//   HOST=127.0.0.1 ./server/directory/register-node.sh

import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const SECRET = process.env.NODE_SECRET ?? "dev";
const PORT = Number(process.env.PORT ?? 8787);
// В контейнере слушать надо все интерфейсы: наружу выпускает сам Docker.
const HOST = process.env.HOST ?? "127.0.0.1";
const POOL_KEY = process.env.POOL_KEY ?? "";
const NODE_TTL = 90_000;
const MAX_CLOCK_SKEW = 300;

/** Где хранится привязка «машина → адрес». */
const DATA_DIR = process.env.DATA_DIR ?? "./data";
const IDS_FILE = join(DATA_DIR, "ids.json");
/** Адреса девятизначные — как во всей экосистеме. */
const ID_MIN = 100_000_000;
const ID_SPAN = 900_000_000;

/** address -> { role, host, port, region, version, expiresAt } */
const nodes = new Map();

/**
 * Выданные адреса: отпечаток машины → адрес.
 *
 * Адрес закрепляется за железом один раз и дальше не меняется: иначе после
 * перезапуска к компьютеру нельзя подключиться по тому адресу, который у
 * пользователя записан.
 */
const issued = new Map();
/** Обратная проверка, чтобы один адрес не достался двум машинам. */
const taken = new Map();

function loadIds() {
  try {
    const raw = JSON.parse(readFileSync(IDS_FILE, "utf8"));
    for (const [machine, id] of Object.entries(raw)) {
      issued.set(machine, id);
      taken.set(id, machine);
    }
    console.log(`восстановлено адресов: ${issued.size}`);
  } catch (err) {
    if (err.code !== "ENOENT") console.warn(`не удалось прочитать ${IDS_FILE}: ${err.message}`);
  }
}

function saveIds() {
  try {
    mkdirSync(dirname(IDS_FILE), { recursive: true });
    // Пишем через временный файл: обрыв записи не должен обнулить привязки.
    const tmp = `${IDS_FILE}.tmp`;
    writeFileSync(tmp, JSON.stringify(Object.fromEntries(issued), null, 2));
    renameSync(tmp, IDS_FILE);
  } catch (err) {
    console.error(`не удалось сохранить ${IDS_FILE}: ${err.message}`);
  }
}

/**
 * Выдаёт адрес для машины: тот же самый при каждом обращении.
 *
 * Первый вариант считается от отпечатка, поэтому даже при потере файла
 * машина чаще всего получит прежний адрес. Занятые адреса обходим перебором.
 */
function issueId(machine) {
  const known = issued.get(machine);
  if (known) return known;

  const digest = createHash("sha256").update(machine).digest();
  const seed = digest.readUInt32BE(0) * 0x1_0000_0000 + digest.readUInt32BE(4);
  let candidate = ID_MIN + Number(BigInt(seed) % BigInt(ID_SPAN));

  while (taken.has(String(candidate))) {
    candidate = ID_MIN + ((candidate - ID_MIN + 1) % ID_SPAN);
  }

  const id = String(candidate);
  issued.set(machine, id);
  taken.set(id, machine);
  saveIds();
  console.log(`выдан адрес ${id} машине ${machine.slice(0, 12)}…`);
  return id;
}

loadIds();

const address = (n) => `${n.host}:${n.port}`;

function alive(role) {
  const now = Date.now();
  return [...nodes.values()]
    .filter((n) => n.role === role && n.expiresAt > now)
    .map(address)
    .sort();
}

function send(res, status, body) {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  res.end(text);
}

function verify(message, signatureHex) {
  if (!/^[0-9a-f]+$/i.test(signatureHex)) return false;
  const expected = createHmac("sha256", SECRET).update(message).digest();
  const given = Buffer.from(signatureHex, "hex");
  return given.length === expected.length && timingSafeEqual(given, expected);
}

createServer((req, res) => {
  if (req.method === "GET" && req.url === "/nodes") {
    return send(res, 200, {
      rendezvous: alive("rendezvous"),
      relay: alive("relay"),
      key: POOL_KEY,
    });
  }
  if (req.method === "GET" && req.url === "/relays") {
    return send(res, 200, { relay: alive("relay") });
  }
  if (req.method === "POST" && req.url === "/id") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      let payload;
      try {
        payload = JSON.parse(body);
      } catch {
        return send(res, 400, { error: "bad json" });
      }
      // Отпечаток железа задаёт клиент; подписи здесь нет — адрес и так
      // публичный, а привязка к машине защищает от чужого захвата.
      const machine = String(payload.machine ?? "").trim();
      if (machine.length < 8 || machine.length > 200) {
        return send(res, 400, { error: "machine fingerprint is required" });
      }
      send(res, 200, { id: issueId(machine) });
    });
    return;
  }
  if (req.method === "POST" && req.url === "/register") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const ts = Number(req.headers["x-skydesktop-timestamp"]);
      const sig = String(req.headers["x-skydesktop-signature"] ?? "");
      if (!Number.isFinite(ts) || Math.abs(Date.now() / 1000 - ts) > MAX_CLOCK_SKEW) {
        return send(res, 401, { error: "stale or missing timestamp" });
      }
      if (!verify(`${ts}.${body}`, sig)) {
        return send(res, 401, { error: "bad signature" });
      }
      let payload;
      try {
        payload = JSON.parse(body);
      } catch {
        return send(res, 400, { error: "bad json" });
      }
      const node = {
        role: payload.role,
        host: String(payload.host ?? "").trim(),
        port: Number(payload.port),
        region: payload.region ?? "",
        version: payload.version ?? "",
        expiresAt: Date.now() + NODE_TTL,
      };
      if (!["rendezvous", "relay"].includes(node.role) || !node.host || !node.port) {
        return send(res, 400, { error: "role, host and port are required" });
      }
      nodes.set(`${node.role}:${address(node)}`, node);
      console.log(`зарегистрирована нода ${node.role} ${address(node)}`);
      send(res, 200, { ok: true, ttl: NODE_TTL / 1000 });
    });
    return;
  }
  send(res, 404, { error: "not found" });
}).listen(PORT, HOST, () => {
  console.log(`каталог нод слушает http://${HOST}:${PORT}`);
});
