# attest_follower_module: logosctl runbook

`attest_follower_module` is a Logos core module (Rust, `#rust` template of `logos-module-builder`). It supervises
`attest-followerd`, the follower daemon that keeps a beacon-veil light-client instance on LEZ current. The daemon gets
its beacon data from Logos `beacon_api_module` v0.2.0 (`lc_request`) through `logosctl call`. The module declares
`beacon_api_module` as a dependency, so loading it loads the beacon module first.

Why a separate process: the daemon links the LEZ wallet and RISC Zero host crates. Those do not belong inside a Qt
plugin. The daemon must also survive `kill -9` and restart on its own (021 R4).

| Method | Answer |
|---|---|
| `configure(settings_json)` | `{ bin, config, log?, state_dir?, env? }`: paths of `attest-followerd`, its TOML config, its JSON log, its state dir, and extra env |
| `start()` | `{ ok, pid }` |
| `stop()` | SIGTERM, then up to 60 s for the step in progress, then SIGKILL: `{ ok, pid, exit, forced }` |
| `status()` | `{ ok, configured, running, pid, uptime_s, last_exit, last_line, cache }` |

| Event (payload: the follower's JSON log line) | Log line |
|---|---|
| `update_submitted` (design name: `updateSubmitted`) | `submit` |
| `handoff_applied` (`handoffApplied`) | `applied` with `extra.handoff = true` |
| `follower_error` (`followerError`) | `submit_failed`, `*_error`, `no_instance` |

Paths below: `B=<the build folder>`, `R=$B/repo`. Always `source $R/scripts/env.sh` first.

Two traps, both found on 2026-11-12:
- The logosctl daemon's local endpoint lives under the daemon's `TMPDIR`. env.sh sets `TMPDIR=$B/.tmp`, so every
  client (and the follower: `logosctl_tmpdir` in its TOML) must use that same `TMPDIR`. Otherwise it reports
  `NO_DAEMON`.
- Each follower uses its own wallet home, never the shared `wallet-dev`. Concurrent writers corrupt the wallet
  storage, and a shared payer collides on nonces. `crates/follower/scripts/wallet_home.sh <name>` creates the home.
  It funds `<name>-payer` from `bv-payer` under the deploy lock, creates a funded private `bv-submitter`, and prints
  the env lines (`LEE_WALLET_HOME_DIR`, `BV_PAYER`, `BV_SUBMITTER`). Add `BV_GUESTS` (deployed guest bins) and
  `BV_IDS` (a frozen copy of deploy/ids.toml) to that env file. Since 2026-11-12, env.sh keeps an existing
  `LEE_WALLET_HOME_DIR`; the follower scripts still load the env file after env.sh.

## 1. Build

```bash
# logosctl 0.3.2: release AppImage, extracted once so each call does not mount or extract it again.
mkdir -p $B/run/w4/logosctl && cd $B/run/w4/logosctl && $B/tools/logosctl/logosctl-x86_64.AppImage --appimage-extract
LOGOSCTL=$B/run/w4/logosctl/squashfs-root/AppRun

# beacon_api_module v0.2.0 (pinned commit aca138e5, its own flake.lock), portable .lgx for the release logosctl.
cd $B/run/w4/nix && nix build "git+file://$B/vendor/logos-evm-beacon-api-module#lgx-portable" -o beacon-api-lgx-portable

# attest_follower_module (path: flake, so untracked files are seen).
nix build "path:$R/modules/attest_follower_module#lgx-portable" -o $B/run/w4/nix/attest-follower-lgx-portable

# The daemon and the CLI.
cd $R && nice -n 15 cargo build --profile host -p follower -p attest_cli
```

## 2. Session, install, load

`crates/follower/scripts/logos_session.sh` wraps these steps. It uses session dir `$B/run/w4/logos-session`, and
`HOME=$B/run/w4/logos-home`, because the package downloader starts a storage node in `$HOME`.

```bash
S=$B/run/w4/logos-session
$LOGOSCTL --config-dir $S daemon start --detach
$LOGOSCTL --config-dir $S install $B/run/w4/nix/beacon-api-lgx-portable/*.lgx -y
$LOGOSCTL --config-dir $S install $B/run/w4/nix/attest-follower-lgx-portable/*.lgx -y
$LOGOSCTL --config-dir $S module load attest_follower_module      # loads beacon_api_module first
$LOGOSCTL --config-dir $S call beacon_api_module init_defaults
$LOGOSCTL --config-dir $S call beacon_api_module list_providers 11155111
# The same request the follower makes, by hand:
$LOGOSCTL --config-dir $S --json call beacon_api_module lc_request 11155111 str:getLightClientFinalityUpdate 'str:{}' 'str:' 30000
# or: crates/follower/scripts/logos_session.sh probe   ->   probe ok provider=lodestar version=gloas finalized_slot=...
```

## 3. Run the follower under the module

```bash
# Checked 2026-11-12 11:15 UTC with the dev instance: $B/run/w4/module-real/{settings.json,events.ndjson,stop.json}.
cat > $B/run/w4/module-settings.json <<JSON
{ "bin": "$B/target/host/attest-followerd",
  "config": "$B/run/w4/dev-logos/followerd.toml",
  "log": "$B/run/w4/dev-logos/follower.jsonl",
  "state_dir": "$B/run/w4/dev-logos",
  "env": { "LEE_WALLET_HOME_DIR": "$B/run/wallet-w4", "BV_PAYER": "...", "BV_SUBMITTER": "...",
           "BV_GUESTS": "$B/run/w4/guests", "BV_IDS": "$B/run/w4/dev/ids.toml",
           "RISC0_DEV_MODE": "1", "BV_REPO": "$R", "TMPDIR": "$B/.tmp" } }
JSON
$LOGOSCTL --config-dir $S call attest_follower_module configure @$B/run/w4/module-settings.json
$LOGOSCTL --config-dir $S call attest_follower_module start
$LOGOSCTL --config-dir $S watch attest_follower_module --json        # update_submitted / handoff_applied / follower_error
# Note (logosctl 0.3.2): a `watch` client killed by a signal seems to leave its subscription behind. The next `watch`
# then prints each event once per earlier session (seen 1x, 2x, 3x over three sessions). De-duplicate on the
# payload's (pid, n), or restart the session.
$LOGOSCTL --config-dir $S call attest_follower_module status
$LOGOSCTL --config-dir $S call attest_follower_module stop
```

For a 24 h G4 run, `crates/follower/scripts/run_a.sh start|status|stop|collect` starts the daemon directly with
`setsid` (`RUN_DIR` selects the run: `$B/run/w4/run-a`, `$B/run/w4/run-b`). The run dir holds `wallet.env`,
a frozen `ids.toml` and `instance.json` (from `attest init --out`); start refuses `wallet-dev`, an unverified store
or a failed module probe. The daemon
does not depend on the logosctl session staying alive, apart from its beacon source. The module path above is the
packaging the design calls for, and it is shown working separately.

## 4. The daemon without Logos (stated fallback, K8)

Set `[source] kind = "direct_http"` in the TOML. The daemon then calls the same Beacon API light-client routes over
HTTP, on the same public providers that `beacon_api_module` seeds. Every log line carries
`beacon_source: "direct_http"`, and the G4 summary reports it.

## 5. Restart-resume (021 R4)

```bash
crates/follower/scripts/r4_drill.sh                         # mock chain
WALLET_ENV=$B/run/w4/wallet-w4.env CHAIN=host INSTANCE=<id> APPLIED_BEFORE_KILL=1 \
  FOLLOWERD=$B/target/host/attest-followerd crates/follower/scripts/r4_drill.sh   # real instance on seq-dev
```

The drill runs `kill -9`, then restarts. PASS means: the restarted process logs one `resume` line at the pre-kill
head, submits nothing older than that head, does not re-initialise, applies a new update, and stops cleanly on
SIGTERM with exit 0.
