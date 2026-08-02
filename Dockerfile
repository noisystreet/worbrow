# syntax=docker/dockerfile:1
#
# worbrow MCP 容器镜像：stdio MCP server（`worbrow mcp`）。
# 仅用于 MCP 握手/工具调用；搜索与抓取需要浏览器后端，容器内不预装。
# 构建（含 MCP 功能，与发布形态一致）：
#   docker build -t worbrow-mcp .
# 本地验证（stdio 交互）：
#   docker run -i --rm worbrow-mcp

# ---- 构建阶段：编译 worbrow（--features mcp，与 .deb 发布形态一致）----
FROM rust:1.97-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release --features mcp

# ---- 运行阶段：仅携带编译产物，非 root 运行 ----
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 worbrow
COPY --from=builder /build/target/release/worbrow /usr/local/bin/worbrow
USER worbrow
ENTRYPOINT ["worbrow", "mcp"]
