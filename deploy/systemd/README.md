# systemd integration

`highland.service` is a starting point (`R-31`). Copy it to
`/etc/systemd/system/`, then:

```console
# systemctl daemon-reload
# systemctl enable --now highland.service
```

`Type=notify` requires the daemon to send `READY=1` once it is running. Until
Milestone 3 that does not happen; until then use `Type=simple` and a
`Restart=on-failure` policy.

Notes:

- `ExecReload` sends `SIGHUP`, which takes the same path as `highland reload`
  (`R-30`).
- `Before=keepalived.service` is intentionally absent. Add it only when both
  daemons are meant to run on the same host, which is a migration setup, not a
  supported steady state.
- If the control socket is in `RuntimeDirectory=highland`, configure
  `control.socket = "/run/highland/control.sock"`.
- The daemon does not require systemd. It runs under OpenRC, in a container, or
  from a shell (`R-32`).
