ARG BASE_TAG=3.4-slim

FROM ruby:3.4-slim AS build
ARG BUNDLE_JOBS
ENV RAILS_ENV=production BUNDLE_PATH=/bundle
ENV LEGACY_HOME /home/app
RUN gem install bundler

FROM ruby:3.4-slim
ENV PORT=8080
CMD ["puma", "start"]
