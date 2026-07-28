#### Security
- `ADD` used instead of `COPY` (ADD fetches from URLs and auto-extracts archives — prefer COPY for local files)
- `latest` tag used in `FROM` — pin a specific version or digest
- `RUN pip install` or `RUN apt-get install` without `--no-cache-dir` or version pinning
- Running as root — use `USER` directive to drop privileges
- Exposing unnecessary ports — only expose what the application needs

#### Layer Optimisation
- Multiple `RUN` commands that could be combined (increases image size)
- Cache-invalidating instructions (`COPY .`) placed before dependency installation
- `apt-get upgrade` or `apt-get dist-upgrade` without pinning (non-deterministic builds)
- Large files added to the image and then removed in a later layer (still occupy space)

#### Build Correctness
- Missing `WORKDIR` — commands run from root by default
- `CMD` vs `ENTRYPOINT` confusion — use exec form (`["executable", "arg"]`) not shell form
- Missing `HEALTHCHECK` for services
- Build arguments not documented with `ARG`
