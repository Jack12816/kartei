MAKEFLAGS += --warn-undefined-variables -j1
SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := all
.DELETE_ON_ERROR:
.SUFFIXES:
.PHONY:

# Environment switches
COMPOSE_PROJECT ?= pipeline

# Directories
LOG_DIR ?= log

# Host binaries
DOCKER ?= docker
ECHO ?= echo
MKDIR ?= mkdir
RM ?= rm

# Container binaries
COMPOSE ?= $(DOCKER) compose -p $(COMPOSE_PROJECT)

all:
	# pipeline
	#
	# install           Pull all images
	# start             Start the stack in the background
	# start-foreground  Start the stack in the foreground
	# stop              Stop the stack
	# restart           Restart the stack
	# logs              Follow the stack logs
	# shell             Open a shell in the scheduler container
	# clean             Remove the log directory
	# distclean         Remove the stack and all its volumes

install:
	# Pull all images
	@$(COMPOSE) pull

build:
	# Build all images
	@$(COMPOSE) build

start:
	# Start the stack in the background
	@$(MKDIR) -p $(LOG_DIR)
	@$(COMPOSE) up -d

stop:
	# Stop the stack
	@$(COMPOSE) stop

restart: \
	stop \
	start

logs:
	# Follow the stack logs
	@$(COMPOSE) logs -f

status:
	# Show the container status
	@$(COMPOSE) ps

shell:
	# Open a shell in the scheduler container
	@$(COMPOSE) exec scheduler bash

db-shell:
	# Open a database shell
	@$(COMPOSE) exec postgres psql -U postgres

reset-db:
	# Drop and recreate the database
	@$(COMPOSE) exec postgres dropdb -U postgres pipeline
	@$(COMPOSE) exec postgres createdb -U postgres pipeline

clean:
	# Remove the log directory
	@$(RM) -rf $(LOG_DIR)

clean-containers:
	# Remove the stack containers
	@$(COMPOSE) down

clean-images:
	# Remove the stack images
	@$(COMPOSE) down --rmi all

start-foreground:
	# Start the stack in the foreground
	@$(MKDIR) -p $(LOG_DIR)
	@$(COMPOSE) up

distclean: \
	clean \
	clean-containers \
	clean-images
