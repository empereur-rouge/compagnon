#!/usr/bin/env bash
# PostgreSQL jetable pour la suite de tests.
#
# Le port 5433 et non 5432 : un PostgreSQL déjà installé sur la machine ne doit jamais être
# atteint par une suite qui crée et détruit des bases.
set -euo pipefail

CONTENEUR=compagnon-pg-test
PORT=5433

case "${1:-demarrer}" in
  demarrer)
    if docker ps --format '{{.Names}}' | grep -qx "$CONTENEUR"; then
      echo "déjà en marche sur 127.0.0.1:$PORT"
      exit 0
    fi
    docker rm -f "$CONTENEUR" >/dev/null 2>&1 || true
    docker run -d --name "$CONTENEUR" \
      -e POSTGRES_PASSWORD=test -e POSTGRES_USER=compagnon -e POSTGRES_DB=compagnon_test \
      -p "$PORT:5432" postgres:17-alpine >/dev/null
    printf 'démarrage'
    until docker exec "$CONTENEUR" pg_isready -U compagnon -d compagnon_test >/dev/null 2>&1; do
      printf '.'; sleep 1
    done
    echo " prêt sur 127.0.0.1:$PORT"
    ;;
  arreter)
    docker rm -f "$CONTENEUR" >/dev/null 2>&1 && echo "arrêté" || echo "n'était pas en marche"
    ;;
  *)
    echo "usage: $0 [demarrer|arreter]" >&2
    exit 2
    ;;
esac
