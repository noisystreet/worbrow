# 质量入口：统一命令（just 未安装时使用 make）
# 常用：make check / make test / make build

.PHONY: fmt lint test check deny machete doctor build deb

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test

check: fmt lint test

deny:
	cargo deny check

machete:
	cargo machete

doctor:
	cargo run -- doctor

build:
	cargo build --release

# MCP 服务器（stdio）支持：`worbrow mcp` 子命令；默认启用（见 Cargo.toml default）
# Debian 打包（`make deb` / CI，cargo-deb 3.x）
deb:
	cargo deb
