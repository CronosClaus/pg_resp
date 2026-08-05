FROM golang:1.22 AS builder
WORKDIR /build
COPY . .
RUN go mod tidy && go build -o limiter main.go

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/limiter /usr/local/bin/
ENTRYPOINT ["limiter"]
