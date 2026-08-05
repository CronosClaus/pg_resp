FROM golang:1.22 AS builder

WORKDIR /build
COPY . .

RUN go mod tidy && \
    go build -o demo-app main.go

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/demo-app /usr/local/bin/

EXPOSE 8080
CMD ["demo-app"]
