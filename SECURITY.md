# Security policy

## Supported deployment

Ununknown is a single-user local application. Its API intentionally accepts file-system paths and
can write corrected media or remove source files when that option is enabled. The server therefore
refuses to bind to non-loopback addresses. Do not expose it through a reverse proxy, tunnel, shared
host, or public network.

The bundled UI and API share one origin. Mutating browser requests with a non-loopback `Origin` are
rejected. API responses use restrictive browser security headers and internal errors are logged
without returning system details to the browser.

Provider credentials entered in the UI are stored in the local SQLite database. On Unix, the
database is forced to owner-only permissions. For managed installations, prefer the documented
environment variables so credentials can be supplied by the process manager instead.

## Reporting a vulnerability

Please use GitHub's private security-advisory reporting flow for this repository. Do not include
real provider credentials, private music files, or personally identifying file paths in a report.
