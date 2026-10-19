# Своя серверная часть SkyDesktop

Сервер связи (hbbs), ретранслятор (hbbr) и каталог нод на одной машине.
Альтернатива каталогу на Cloudflare (`../directory/`) — API у них одинаковый.

## Что разворачивается

| Служба | Порт | Назначение |
| --- | --- | --- |
| hbbs | 21115–21116, 21118 (TCP), 21116 (UDP) | регистрация адресов, пробивка NAT |
| hbbr | 21117, 21119 | ретрансляция, когда прямое соединение невозможно |
| каталог | 8787 | список нод для клиентов |

## Развёртывание

```bash
scp -r server/self-hosted server/directory root@СЕРВЕР:/opt/skydesktop-server
ssh root@СЕРВЕР
cd /opt/skydesktop-server

# Секрет для подписи регистраций и ключ пула (появится после первого старта hbbs).
cat > .env <<'EOF'
NODE_SECRET=<длинная случайная строка>
POOL_KEY=
EOF
chmod 600 .env

docker compose up -d hbbs hbbr
cat data/id_ed25519.pub          # публичный ключ пула → в POOL_KEY
docker compose up -d directory
```

Порты должны быть открыты наружу. На HestiaCP:

```bash
V=/usr/local/hestia/bin/v-add-firewall-rule
$V ACCEPT 0.0.0.0/0 21115-21119 TCP SKYDESKTOP_TCP
$V ACCEPT 0.0.0.0/0 21116 UDP SKYDESKTOP_UDP
$V ACCEPT 0.0.0.0/0 8787 TCP SKYDESKTOP_DIRECTORY
```

## Регистрация нод в каталоге

Ноды не прописываются руками — они регистрируются сами и подтверждают, что
живы. Шаблон службы запускает агента для каждой роли:

```bash
cp skydesktop-register@.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now skydesktop-register@rendezvous skydesktop-register@relay
curl -s http://127.0.0.1:8787/nodes
```

`HOST` в юните — публичное имя ноды, именно оно попадёт клиентам.
Чтобы добавить в пул вторую машину, разверните на ней hbbs (и/или hbbr)
и запустите там того же агента с тем же `NODE_SECRET` и адресом каталога.

## Сборка клиента под этот пул

```bash
SKYDESKTOP_DIRECTORY=http://СЕРВЕР:8787/nodes scripts/build-skydesktop.sh
```

Клиент раз в минуту забирает список, сам подставляет ключ пула и убирает из
настроек ядра одиночный сервер и ретранслятор — при вшитом каталоге источник
истины он.

## Чего здесь нет

Каталог отдаётся по HTTP. Для боевой эксплуатации его стоит закрыть TLS
(отдельный домен на nginx или вариант с Cloudflare Worker), иначе список нод
и ключ пула идут открытым текстом.

Состояние каталога живёт в памяти: после перезапуска список собирается заново
из heartbeat'ов нод примерно за полминуты.
