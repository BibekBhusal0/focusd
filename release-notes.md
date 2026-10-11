# Release notes

## Fixes

- Daemon socket is now per-user (`$XDG_RUNTIME_DIR/focusd/`, or a uid-keyed file under `/tmp`) instead of a shared `/tmp/focusd.sock`, so another local user can no longer pre-create or spoof it.
- The daemon refuses to replace a socket file owned by someone else instead of deleting it.
- Both ends verify the peer runs as the same user and drop anything else.
- Socket reads are capped at 64 KiB with timeouts (5 s client, 30 s daemon), so an unterminated stream cannot exhaust memory.
