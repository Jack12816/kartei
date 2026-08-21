# acme-api

A small, fictional JSON API serving users and their sessions. It
exists to give kartei's integration tests a realistic Ruby project
shape: Grape-style API classes, controllers, a library namespace,
YAML configuration with anchors and an RSpec tree.

## Usage

    make start
    curl localhost:3000/api/v1/users
