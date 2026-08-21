NAME = kartei
PREFIX := /usr/local
CC ?= gcc
CFLAGS += -Wall -O2

.PHONY: all clean

all: build

build test: main.o
	@echo done

%.o: %.c
	$(CC) -c $< -o $@

clean:
	$(RM) -f *.o
