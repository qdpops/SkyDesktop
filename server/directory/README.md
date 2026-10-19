# Каталог нод SkyDesktop

Справочник серверов: ноды регистрируются в нём сами, приложение периодически
забирает список. Пропала нода — запись выпадает по TTL, без чьего-либо участия.

Через каталог **не идёт** трафик удалённого доступа. Сервер-посредник (21116)
и реле (21117) работают по сырому TCP/UDP, который обычный прокси Cloudflare
не проксирует, — Cloudflare здесь только справочник и управление.

## API

```
POST /register   регистрация и heartbeat ноды (подпись HMAC)
GET  /nodes      список для клиентов SkyDesktop
GET  /relays     список реле (для hbbs)
```

`GET /nodes` возвращает:

```json
{
  "rendezvous": ["rz1.example.com:21116", "rz2.example.com:21116"],
  "relay": ["relay1.example.com:21117"],
  "key": "общий публичный ключ пула"
}
```

Адрес каталога вшивается в приложение при сборке
(`SKYDESKTOP_DIRECTORY=... scripts/build-skydesktop.sh`), в интерфейсе он не
настраивается.

Приложение кладёт `rendezvous` в настройку ядра `skydesktop-rendezvous-pool`.
Дальше работает штатный механизм ядра: хост регистрируется **сразу на всех**
серверах пула, а для исходящих подключений выбирает самый быстрый по задержке.
Список `relay` показывается справочно — реле назначает сам посредник.

`key` применяется, только если он не пуст. Ключ в пуле обязан быть общим:
клиент сверяет его при подключении к любой ноде.

## Развёртывание

```bash
cd server/directory
npx wrangler kv namespace create NODES     # id вписать в wrangler.toml
npx wrangler secret put NODE_SECRET        # общий секрет для подписи регистраций
npx wrangler deploy
```

В `wrangler.toml` в `POOL_KEY` положите публичный ключ вашего пула.

## Регистрация ноды

На каждой ноде рядом с hbbs или hbbr:

```bash
DIRECTORY=https://nodes.example.com \
NODE_SECRET=... \
ROLE=relay HOST=relay1.example.com PORT=21117 REGION=eu \
./register-node.sh
```

Роли: `rendezvous` (порт по умолчанию 21116) и `relay` (21117). Агент шлёт
heartbeat раз в 30 секунд, запись живёт 90 — двух пропущенных хватает, чтобы
нода выпала из списка.

Подписывается «метка времени + тело», расхождение часов допускается до
5 минут: так подпись нельзя переиграть позже.

## Локальная проверка без Cloudflare

```bash
NODE_SECRET=dev node dev-server.mjs                 # каталог на 127.0.0.1:8787

DIRECTORY=http://127.0.0.1:8787 NODE_SECRET=dev \
ROLE=rendezvous HOST=127.0.0.1 ./register-node.sh   # нода

curl -s http://127.0.0.1:8787/nodes
```

Приложение с этим каталогом собирается так:

```bash
SKYDESKTOP_DIRECTORY=http://127.0.0.1:8787/nodes scripts/build-skydesktop.sh debug
```
