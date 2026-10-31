#!/usr/bin/env bash
# Token 戰情室容器管理腳本：每個名稱對應一個獨立容器與資料磁碟區，用來分開檢視不同人的用量資料。
# 用法請執行 docker/tui-docker.sh help，完整說明見 docker/README.md。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE="${TUI_IMAGE:-token-usage-insights:local}"
PORT_START="${TUI_PORT_START:-3100}"
CONTAINER_PORT=3003
LABEL_PREFIX="io.github.doggy8088.token-usage-insights"
APP_BIN="/app/token-usage-insights"

usage() {
    cat <<EOF
Token 戰情室容器管理腳本

用法:
  $(basename "$0") <命令> [參數]

命令:
  up <名稱> [來源...] [--port <埠號>] [--no-open] [--rebuild]
      建立（或啟動既有的）容器，依序匯入來源後開啟瀏覽器。
      來源可為本機 JSON 檔案路徑或 https:// 開頭的 SAS URL，可指定多個；
      亦可用 --file <路徑> / --url <網址> 明確指定。
  import <名稱> <來源...>      匯入資料到執行中的容器（重複匯入會自動去重）
  open <名稱>                  以預設瀏覽器開啟該容器的看板
  url <名稱>                   印出該容器的看板網址
  export <名稱> [--out <路徑|SAS URL>]
      匯出該容器內的全部資料（預設輸出 ./<名稱>-export-all.json）
  logs <名稱> [-f]             檢視容器日誌
  stop <名稱>                  停止容器（資料保留）
  rm <名稱> [--purge]          移除容器；加上 --purge 才會一併刪除資料磁碟區
  ls                           列出所有由本腳本建立的容器
  build [--no-cache]           重新建置映像檔 ${IMAGE}
  help                         顯示此說明

環境變數:
  TUI_IMAGE       映像檔名稱（預設 token-usage-insights:local）
  TUI_PORT_START  自動挑選本機埠號的起點（預設 3100）

範例:
  $(basename "$0") up alice ~/Downloads/alice-usage.json
  $(basename "$0") up bob 'https://<account>.blob.core.windows.net/<container>/<檔名>.json?<SAS>'
  $(basename "$0") import alice ./alice-2026-11.json
  $(basename "$0") rm bob --purge
EOF
}

die() {
    echo "❌ $*" >&2
    exit 1
}

info() {
    echo "▶ $*"
}

require_docker() {
    command -v docker >/dev/null 2>&1 || die "找不到 docker 指令，請先安裝 Docker Desktop 或 Docker Engine"
    docker info >/dev/null 2>&1 || die "無法連線到 Docker daemon，請確認 Docker 已啟動"
}

validate_name() {
    local name="${1:-}"
    [[ -n "$name" ]] || die "請指定容器名稱"
    [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] \
        || die "名稱只能包含英數字、底線、點與連字號，且必須以英數字開頭：$name"
}

container_name() {
    echo "tui-$1"
}

volume_name() {
    echo "tui-$1-data"
}

container_exists() {
    docker container inspect "$1" >/dev/null 2>&1
}

container_running() {
    [[ "$(docker container inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" == "true" ]]
}

container_port() {
    docker container inspect -f "{{ index .Config.Labels \"${LABEL_PREFIX}.port\" }}" "$1" 2>/dev/null
}

require_container() {
    container_exists "$1" || die "找不到容器 $1，請先執行：$(basename "$0") up ${1#tui-}"
}

dashboard_url() {
    echo "http://localhost:$1"
}

build_image() {
    local build_date
    build_date="$(git -C "$PROJECT_DIR" log -1 --format=%cs 2>/dev/null || true)"
    info "建置映像檔 ${IMAGE}（首次建置需編譯 Rust，約需數分鐘）"
    docker build \
        -f "$SCRIPT_DIR/Dockerfile" \
        --build-arg "APP_BUILD_DATE=${build_date}" \
        -t "$IMAGE" \
        "$@" \
        "$PROJECT_DIR"
}

ensure_image() {
    if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
        build_image
    fi
}

host_port_listening() {
    # 以 bash 內建的 /dev/tcp 探測，連得上代表本機已有程式監聽該埠
    (exec 3<>"/dev/tcp/127.0.0.1/$1") >/dev/null 2>&1
}

port_reserved_by_container() {
    # 已停止的容器不會監聽，但重新啟動時仍會使用原本的埠號
    docker ps -a --filter "label=${LABEL_PREFIX}.port=$1" --format '{{.Names}}' | grep -q .
}

pick_free_port() {
    local port="$PORT_START"
    while ((port < 65535)); do
        if ! host_port_listening "$port" && ! port_reserved_by_container "$port"; then
            echo "$port"
            return 0
        fi
        port=$((port + 1))
    done
    die "找不到可用的本機埠號"
}

wait_until_ready() {
    local container="$1"
    local _
    for _ in $(seq 1 60); do
        if docker exec "$container" wget -q -O /dev/null "http://127.0.0.1:${CONTAINER_PORT}/api/version" >/dev/null 2>&1; then
            return 0
        fi
        if ! container_running "$container"; then
            docker logs --tail 30 "$container" >&2 || true
            die "容器 $container 啟動失敗，以上為最後的日誌"
        fi
        sleep 1
    done
    die "等待 $container 啟動逾時，請執行 $(basename "$0") logs ${container#tui-} 查看原因"
}

import_file() {
    local container="$1" file="$2"
    [[ -f "$file" ]] || die "找不到檔案：$file"
    # 保留原始檔名，讓看板的匯入批次記錄可辨識來源
    local staging="/tmp/import-$$-$RANDOM"
    local base
    base="$(basename "$file")"
    info "匯入檔案 $file"
    docker exec "$container" mkdir -p "$staging"
    docker cp "$file" "$container:$staging/$base" >/dev/null
    # docker cp 會保留來源權限但擁有者變成 root；來源為 0600 時 app 使用者會讀不到，因此改回 app 擁有
    docker exec -u root "$container" chown -R app:app "$staging"
    local status=0
    docker exec "$container" "$APP_BIN" import --file "$staging/$base" || status=$?
    docker exec "$container" rm -rf "$staging" || true
    ((status == 0)) || die "匯入 $file 失敗（結束碼 $status）"
}

import_url() {
    local container="$1" url="$2"
    # 只顯示不含 query string 的網址，避免把 SAS 簽章印在終端機上
    info "匯入網址 ${url%%\?*}"
    local status=0
    docker exec "$container" "$APP_BIN" import --url "$url" || status=$?
    ((status == 0)) || die "匯入網址失敗（結束碼 $status）"
}

import_source() {
    local container="$1" source="$2"
    case "$source" in
        https://*) import_url "$container" "$source" ;;
        http://*) die "網址必須使用 HTTPS：${source%%\?*}" ;;
        *) import_file "$container" "$source" ;;
    esac
}

open_browser() {
    local url="$1"
    if command -v open >/dev/null 2>&1 && [[ "$(uname -s)" == "Darwin" ]]; then
        open "$url"
    elif command -v wslview >/dev/null 2>&1; then
        wslview "$url"
    elif command -v xdg-open >/dev/null 2>&1; then
        xdg-open "$url" >/dev/null 2>&1 &
    elif command -v cmd.exe >/dev/null 2>&1; then
        cmd.exe /c start "" "$url" >/dev/null 2>&1
    else
        echo "請手動以瀏覽器開啟：$url"
    fi
}

cmd_up() {
    local name="${1:-}"
    validate_name "$name"
    shift

    local port="" no_open=0 rebuild=0
    local sources=()
    while (($# > 0)); do
        case "$1" in
            --port)
                [[ $# -ge 2 ]] || die "--port 需要指定埠號"
                port="$2"
                shift 2
                ;;
            --file | --url)
                [[ $# -ge 2 ]] || die "$1 需要指定值"
                sources+=("$2")
                shift 2
                ;;
            --no-open)
                no_open=1
                shift
                ;;
            --rebuild)
                rebuild=1
                shift
                ;;
            -*)
                die "未知參數：$1"
                ;;
            *)
                sources+=("$1")
                shift
                ;;
        esac
    done

    if [[ -n "$port" ]]; then
        if ! [[ "$port" =~ ^[0-9]+$ ]] || ((port < 1 || port > 65535)); then
            die "埠號無效：$port"
        fi
    fi

    require_docker
    if ((rebuild)); then
        build_image
    else
        ensure_image
    fi

    local container volume
    container="$(container_name "$name")"
    volume="$(volume_name "$name")"

    if container_exists "$container"; then
        local existing_port
        existing_port="$(container_port "$container")"
        if [[ -n "$port" && "$port" != "$existing_port" ]]; then
            die "容器 $container 已使用埠號 $existing_port；若要更換，請先執行 $(basename "$0") rm $name（資料會保留）再重新 up"
        fi
        if ((rebuild)); then
            echo "⚠️ 已重新建置映像檔，但既有容器仍使用舊映像檔；如需套用請執行 rm $name 後重新 up（資料會保留）"
        fi
        port="$existing_port"
        if container_running "$container"; then
            info "容器 $container 已在執行中"
        else
            info "啟動既有容器 $container"
            docker start "$container" >/dev/null
        fi
    else
        if [[ -z "$port" ]]; then
            port="$(pick_free_port)"
        elif host_port_listening "$port" || port_reserved_by_container "$port"; then
            die "本機埠號 $port 已被使用，請改用其他埠號或省略 --port 自動挑選"
        fi
        info "建立容器 $container（資料磁碟區 $volume，本機埠號 $port）"
        # 只綁定 127.0.0.1，避免個人用量資料暴露在區域網路上
        docker run -d \
            --name "$container" \
            --label "${LABEL_PREFIX}.name=$name" \
            --label "${LABEL_PREFIX}.port=$port" \
            -v "$volume:/data" \
            -p "127.0.0.1:${port}:${CONTAINER_PORT}" \
            "$IMAGE" >/dev/null
    fi

    wait_until_ready "$container"

    local source
    for source in ${sources[@]+"${sources[@]}"}; do
        import_source "$container" "$source"
    done

    local url
    url="$(dashboard_url "$port")"
    echo
    echo "✅ $name 的看板已就緒：$url"
    if ((no_open == 0)); then
        open_browser "$url"
    fi
}

cmd_import() {
    local name="${1:-}"
    validate_name "$name"
    shift
    (($# > 0)) || die "請至少指定一個匯入來源（JSON 檔案路徑或 SAS URL）"

    require_docker
    local container
    container="$(container_name "$name")"
    require_container "$container"
    container_running "$container" || die "容器 $container 未執行，請先執行：$(basename "$0") up $name"

    local source
    for source in "$@"; do
        import_source "$container" "$source"
    done
    echo "✅ 匯入完成，重新整理看板即可看到資料：$(dashboard_url "$(container_port "$container")")"
}

cmd_open() {
    local name="${1:-}"
    validate_name "$name"
    require_docker
    local container
    container="$(container_name "$name")"
    require_container "$container"
    container_running "$container" || die "容器 $container 未執行，請先執行：$(basename "$0") up $name"
    open_browser "$(dashboard_url "$(container_port "$container")")"
}

cmd_url() {
    local name="${1:-}"
    validate_name "$name"
    require_docker
    local container
    container="$(container_name "$name")"
    require_container "$container"
    dashboard_url "$(container_port "$container")"
}

cmd_export() {
    local name="${1:-}"
    validate_name "$name"
    shift
    local out="./${name}-export-all.json"
    while (($# > 0)); do
        case "$1" in
            --out)
                [[ $# -ge 2 ]] || die "--out 需要指定路徑或 SAS URL"
                out="$2"
                shift 2
                ;;
            *) die "未知參數：$1" ;;
        esac
    done

    require_docker
    local container
    container="$(container_name "$name")"
    require_container "$container"
    container_running "$container" || die "容器 $container 未執行，請先執行：$(basename "$0") up $name"

    case "$out" in
        https://*)
            docker exec "$container" "$APP_BIN" export-all --out "$out"
            ;;
        *)
            local staging="/tmp/export-$$-$RANDOM.json"
            docker exec "$container" "$APP_BIN" export-all --out "$staging"
            docker cp "$container:$staging" "$out" >/dev/null
            docker exec "$container" rm -f "$staging" || true
            echo "✅ 已匯出到 $out"
            ;;
    esac
}

cmd_logs() {
    local name="${1:-}"
    validate_name "$name"
    shift
    require_docker
    docker logs "$@" "$(container_name "$name")"
}

cmd_stop() {
    local name="${1:-}"
    validate_name "$name"
    require_docker
    local container
    container="$(container_name "$name")"
    require_container "$container"
    docker stop "$container" >/dev/null
    echo "⏹ 已停止 $container（資料保留在磁碟區 $(volume_name "$name")）"
}

cmd_rm() {
    local name="${1:-}"
    validate_name "$name"
    shift
    local purge=0
    while (($# > 0)); do
        case "$1" in
            --purge) purge=1 ;;
            *) die "未知參數：$1" ;;
        esac
        shift
    done

    require_docker
    local container volume
    container="$(container_name "$name")"
    volume="$(volume_name "$name")"
    if container_exists "$container"; then
        docker rm -f "$container" >/dev/null
        echo "🗑 已移除容器 $container"
    else
        echo "容器 $container 不存在，略過"
    fi
    if ((purge)); then
        if docker volume inspect "$volume" >/dev/null 2>&1; then
            docker volume rm "$volume" >/dev/null
            echo "🗑 已刪除資料磁碟區 $volume"
        fi
    else
        echo "資料仍保留在磁碟區 $volume；下次 up $name 會沿用，加上 --purge 可一併刪除"
    fi
}

cmd_ls() {
    require_docker
    docker ps -a \
        --filter "label=${LABEL_PREFIX}.name" \
        --format "table {{.Label \"${LABEL_PREFIX}.name\"}}\t{{.Names}}\t{{.Status}}\thttp://localhost:{{.Label \"${LABEL_PREFIX}.port\"}}"
}

main() {
    local command="${1:-help}"
    (($# > 0)) && shift
    case "$command" in
        up) cmd_up "$@" ;;
        import) cmd_import "$@" ;;
        open) cmd_open "$@" ;;
        url) cmd_url "$@" ;;
        export) cmd_export "$@" ;;
        logs) cmd_logs "$@" ;;
        stop) cmd_stop "$@" ;;
        rm) cmd_rm "$@" ;;
        ls) cmd_ls ;;
        build)
            require_docker
            build_image "$@"
            ;;
        help | -h | --help) usage ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
