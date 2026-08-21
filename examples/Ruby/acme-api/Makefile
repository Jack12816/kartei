MAKEFLAGS += --warn-undefined-variables -j1
SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := all
.DELETE_ON_ERROR:
.SUFFIXES:
.PHONY:

# Host binaries
BUNDLE ?= bundle
DOCKER ?= docker

all:
	# acme-api
	#
	# start             Start the application server
	# test              Run the whole test suite
	# shell             Open an interactive shell in the app container

start:
	# Start the application server
	@$(BUNDLE) exec puma

test:
	# Run the whole test suite
	@$(BUNDLE) exec rspec

shell:
	# Open an interactive shell in the app container
	@$(DOCKER) compose exec app bash
