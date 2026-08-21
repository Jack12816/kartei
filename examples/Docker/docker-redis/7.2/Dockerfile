FROM debian:bookworm-slim
LABEL org.opencontainers.image.authors="someone@example.org"

ENV DEBIAN_FRONTEND=noninteractive
ENV REDIS_VERSION=7.2.4

# Install the server from the distribution packages
RUN apt-get update -yqqq && \
  apt-get install -y redis-server

EXPOSE 6379
CMD ["redis-server"]
