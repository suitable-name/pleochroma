# Service files for pleochroma-worker

Templates that run the coordinator (`pleochroma-worker serve`) or a render worker
(`pleochroma-worker join`) as a background service on Linux, under its own user. Each
template has placeholders in comments; change them before the first start. The
step-by-step setup (user, folders, certificates, GPU groups, logs, updates) is in the
manual, chapter 10, "Running the coordinator or a render computer as a Linux service".

All files use LF line endings. Both services pass `--headless`, so the program never
opens a window.

| File | What it is | Copy it to |
|---|---|---|
| `systemd/pleochroma-coordinator.service` | systemd unit for `serve` | `/etc/systemd/system/pleochroma-coordinator.service` |
| `systemd/pleochroma-worker.service` | systemd unit for `join` | `/etc/systemd/system/pleochroma-worker.service` |
| `openrc/pleochroma-coordinator` | OpenRC init script for `serve` (`supervise-daemon`), mode 0755 | `/etc/init.d/pleochroma-coordinator` |
| `openrc/pleochroma-coordinator.confd` | its arguments and environment | `/etc/conf.d/pleochroma-coordinator` |
| `openrc/pleochroma-worker` | OpenRC init script for `join`, mode 0755 | `/etc/init.d/pleochroma-worker` |
| `openrc/pleochroma-worker.confd` | its arguments and environment | `/etc/conf.d/pleochroma-worker` |

What the templates assume:

- the binary is `/usr/local/bin/pleochroma-worker`;
- a system user and group `pleochroma` with the home folder `/var/lib/pleochroma`;
- the certificate authority and server certificate in `/var/lib/pleochroma/pki`, the worker
  certificate in `/var/lib/pleochroma/worker-cert`, the library in
  `/var/lib/pleochroma/facet_diagrams.sqlite`;
- OpenRC logs go to `/var/log/pleochroma/` (the init script creates the folder); systemd
  logs go to the journal.

A computer that runs both services uses one user and one folder; the two templates keep
their certificates in different subfolders.

systemd:

```
sudo cp systemd/pleochroma-coordinator.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now pleochroma-coordinator
```

OpenRC:

```
sudo install -m 0755 openrc/pleochroma-coordinator /etc/init.d/pleochroma-coordinator
sudo install -m 0644 openrc/pleochroma-coordinator.confd /etc/conf.d/pleochroma-coordinator
sudo rc-update add pleochroma-coordinator default
sudo rc-service pleochroma-coordinator start
```

Use the `pleochroma-worker` files in the same way for a render worker.
