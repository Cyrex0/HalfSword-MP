#!/usr/bin/env bash
# Entrypoint of the HSMP Docker image. Starts hsmp-master (optional) and hsmp-server with
# environment-driven config; tini (PID 1) forwards signals and reaps them.
#
# HSMP_STATE_DIR (default /hsmp/data, a volume) holds the server identity key
# (server_identity.key): clients pin it, so it must survive container re-creation.
set -euo pipefail

umask 077
mkdir -p "${HSMP_STATE_DIR:-/hsmp/data}"

if [[ "${HSMP_WITH_MASTER:-0}" == "1" ]]; then
    /usr/local/bin/hsmp-master --bind "${HSMP_MASTER_BIND}" &
    # Point the server at the local master so it auto-registers.
    PORT_PART="${HSMP_MASTER_BIND##*:}"
    export HSMP_MASTER_URL="http://127.0.0.1:${PORT_PART}"
    echo "[docker-entry] master up on ${HSMP_MASTER_BIND}, auto-register via ${HSMP_MASTER_URL}"
fi

CMD=(/usr/local/bin/hsmp-server
     --bind "${HSMP_BIND}"
     --max-peers "${HSMP_MAX_PEERS}"
     --name "${HSMP_NAME}"
     --mode "${HSMP_MODE}"
     --bans-file "${HSMP_BANS_FILE}"
     --admins-file "${HSMP_ADMINS_FILE}")

# RCON: off unless HSMP_RCON_BIND is set (password from HSMP_RCON_PASSWORD; a non-loopback
# bind also needs HSMP_RCON_ALLOW_REMOTE=1, which the server reads itself).
if [[ -n "${HSMP_RCON_BIND:-}" ]]; then
    CMD+=(--rcon-bind "${HSMP_RCON_BIND}")
fi

exec "${CMD[@]}"
