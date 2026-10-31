# syntax=docker/dockerfile:1
# check=skip=SecretsUsedInArgOrEnv
#
# 上方 check 指示：TOKEN_USAGE_INSIGHTS_SERVICE 只是服務模式旗標，名稱含 TOKEN 會被 SecretsUsedInArgOrEnv 誤判為機密。
#
# Token 戰情室最小化容器映像檔
#
# 建置內容必須是專案根目錄（同目錄的 Dockerfile.dockerignore 會以白名單只送出必要檔案）：
#   docker build -f docker/Dockerfile -t token-usage-insights:local .
# 一般使用請改用 docker/tui-docker.sh，它會處理建置、建立具名容器、匯入資料與開啟瀏覽器。

# ---- 建置階段：以 musl 靜態連結，執行階段不需要 glibc ----
FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /src

# .git 不在建置內容中，build.rs 無法自行取得提交日期，改由建置參數帶入（可留空）
ARG APP_BUILD_DATE=""
ENV APP_BUILD_DATE=${APP_BUILD_DATE}

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

# 快取 crates 與 target 目錄以加速重複建置；執行檔需在同一個 RUN 內複製出快取掛載點
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin token-usage-insights \
    && install -D -m 0755 target/release/token-usage-insights /out/token-usage-insights

# ---- 執行階段 ----
FROM alpine:3.22

# reqwest 使用 rustls + webpki-roots 內建根憑證，import --url 不需要另外安裝 ca-certificates
RUN addgroup -S app \
    && adduser -S -G app -h /home/app app \
    && mkdir -p /data \
    && chown app:app /data

WORKDIR /app

# 刻意不放 VERSION 檔：避免程式把 /app 判定為標準安裝目錄而啟用自我更新與服務 runner 機制
COPY --from=builder /out/token-usage-insights /app/token-usage-insights
COPY static /app/static
COPY pricing.csv /app/pricing.csv

# INSIGHTS_DIR 指向獨立的資料磁碟區；容器內沒有任何 AI 工具的本機日誌，資料只來自 import
ENV INSIGHTS_DIR=/data \
    HOST=0.0.0.0 \
    PORT=3003 \
    HOME=/home/app \
    TOKEN_USAGE_INSIGHTS_SERVICE=1

USER app
VOLUME ["/data"]
EXPOSE 3003

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${PORT}/api/version" || exit 1

# --no-auto-update 必須放在子命令之前；docker exec 執行 import / export-all 不經過此 ENTRYPOINT
ENTRYPOINT ["/app/token-usage-insights", "--no-auto-update"]
