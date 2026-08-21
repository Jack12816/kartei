FROM postgres:16
LABEL org.opencontainers.image.authors="someone@example.org"

ENV POSTGRES_DB=app
ENV LANG=en_US.UTF-8

# Install the extensions we ship by default
RUN apt-get update -yqqq && \
  apt-get install -y postgresql-16-postgis-3
