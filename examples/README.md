# examples

The integration testing ground of kartei: a small, entirely fictional
multi-repository file tree the end-to-end tests index. Nothing in
here is a real project — the files only carry the shapes the tests
assert on (Ruby classes with scopes, YAML anchors and merge keys, a
Dockerfile ENV, an Erlang module, make targets, an RSpec tree).

Git cannot track nested repositories, so the repository roots are
marked by an empty `.kartei-repo` file instead. The test harness
copies this tree into a temporary directory, turns every marked
directory into a git repository (deepest first, so the parent records
nothing about its nested checkouts) and indexes the copy — the
checked-in tree itself is never touched.

    Ruby/acme-api            API classes, lib namespace, config/jwt.yml
    Ruby/blog-engine         second Ruby repo (prefix disambiguation)
    Ruby/gems/metrics_helper singleton methods in a gem
    Docker/docker-postgres   Dockerfile per version directory
    Docker/docker-redis      Dockerfile ENV variables
    Erlang/chat-unread       Erlang functions; tmp/ holds the
                             nested-repository trap (git-ignored)
    Config/pipeline          compose anchors and a long Makefile
