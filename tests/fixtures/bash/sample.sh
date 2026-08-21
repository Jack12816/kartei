#!/usr/bin/env bash

PROGNAME="$(basename "${0}")"
COLOR='red'

function first-helper()
{
  local inner='nope'
  INNER_TOO='nope'
}

second_helper() {
  other='nope'
}

declare -r LIMIT=5
