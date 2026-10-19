#!/usr/bin/env bash
# Агент регистрации ноды в каталоге SkyDesktop.
#
# Запускается на самой ноде (рядом с hbbs или hbbr) и раз в HEARTBEAT секунд
# сообщает каталогу, что нода жива. Перестал слать — запись выпала по TTL.
#
#   DIRECTORY=https://nodes.example.com \
#   NODE_SECRET=... \
#   ROLE=relay HOST=relay1.example.com PORT=21117 REGION=eu \
#   ./register-node.sh
set -eu

DIRECTORY="${DIRECTORY:?укажите адрес каталога}"
NODE_SECRET="${NODE_SECRET:?укажите общий секрет}"
ROLE="${ROLE:?укажите роль: rendezvous или relay}"
HOST="${HOST:?укажите публичный адрес ноды}"
REGION="${REGION:-}"
VERSION="${VERSION:-}"
HEARTBEAT="${HEARTBEAT:-30}"

# Порты по умолчанию — те же, что у ядра.
if [ -z "${PORT:-}" ]; then
  case "$ROLE" in
    rendezvous) PORT=21116 ;;
    relay) PORT=21117 ;;
    *) echo "неизвестная роль: $ROLE" >&2; exit 1 ;;
  esac
fi

body=$(printf '{"role":"%s","host":"%s","port":%s,"region":"%s","version":"%s"}' \
  "$ROLE" "$HOST" "$PORT" "$REGION" "$VERSION")

while :; do
  ts=$(date +%s)
  # Подписывается «метка времени + тело»: так подпись нельзя переиграть позже.
  sig=$(printf '%s.%s' "$ts" "$body" \
    | openssl dgst -sha256 -hmac "$NODE_SECRET" -hex \
    | sed 's/^.*= //')

  code=$(curl -sS -o /dev/null -w '%{http_code}' \
    -X POST "${DIRECTORY%/}/register" \
    -H 'content-type: application/json' \
    -H "x-skydesktop-timestamp: $ts" \
    -H "x-skydesktop-signature: $sig" \
    --data "$body" || echo 000)

  if [ "$code" != "200" ]; then
    echo "$(date -Is) каталог ответил $code" >&2
  fi

  sleep "$HEARTBEAT"
done
