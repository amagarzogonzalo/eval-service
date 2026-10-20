# Minimal target image for the eval Docker sandbox.
#
# The target checkout and its venv are mounted read-only at their host
# absolute paths; this image only needs the interpreter and basic tools.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        python3 \
        python3-venv \
    && rm -rf /var/lib/apt/lists/*

# The sandbox user is supplied with `--user`; UID 65534 (nobody) needs no
# passwd entry because the adapter resolves paths, not users.
WORKDIR /tmp
CMD ["python3", "--version"]
