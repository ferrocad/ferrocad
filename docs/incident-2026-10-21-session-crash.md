# Host session crash — 2026-10-21 16:45 EEST

Status: incident (2026-10-21). Read-only post-mortem of the loss of the GNOME/Wayland
session on `john-XPS-15-9570` and the power-off that followed. Written from the
persistent journal of boot `905b047c5c4f40b6a6e9d6578d849e24` (boot `-1`), the
`sysstat` daily file for 21 October, `coredumpctl`, and on-disk state observed after
the next boot. No setting was changed, no service restarted, no journal or file
deleted, no build run, and no GUI application started while collecting evidence.

All `journalctl` invocations below were run with `--no-pager`; timestamps are local
(EEST, UTC+3) as printed by `--output=short-iso`. Sizes reported by systemd are
binary (`G` = GiB); `sar` figures are KiB and are converted below where convenient.

Host baseline (`free -h`, `swapon --show`, `uname` via `journalctl -k -b 0`): 30 GiB
RAM, one 33.8 GiB swap partition (`/dev/nvme0n1p8`), 12 CPUs, kernel
`7.0.0-15-generic`, NVIDIA `580.159.03`, mutter/gnome-shell 50.1.

---

## 1. Summary

The session was killed by **`systemd-oomd`**, not by a crash of GNOME, not by the GPU,
and not by the kernel. At `16:45:32` oomd SIGKILLed
`org.gnome.Shell@ubuntu.service` because the `user@1000.service` cgroup had been above
its 50 % memory-pressure threshold for more than 20 s
(`being 82.22% > 50.00% for > 20s with reclaim activity`); the shell's own cgroup was
at 21.3 GiB at that moment and reported a 26.7 GiB peak with a 22.6 GiB swap peak.
Every Wayland client died with it (`Lost connection to Wayland compositor`,
`Error reading events from display: Broken pipe`), GDM took over 4 s later, and 21 s
after the kill the human pressed the power key, which produced a clean, orderly
`poweroff.target`. The kernel OOM killer never ran, no Xid/NVRM errors were logged, no
`gnome-shell` coredump exists, no panic or machine-check occurred, and the filesystem
was unmounted cleanly.

Confidence: **high** that oomd ended the session (directly logged by the killer and by
systemd). **Medium** on *what allocated the memory*: the accounting localises the
failing cgroup to gnome-shell, but the journal is completely silent between
`16:37:14` and `16:45:32`, so the allocation source inside that cgroup cannot be
attributed. The machine was also heavily overcommitted earlier in the afternoon
(`%commit` reached 98.00 % at `15:30`), and a 23-day-old session had accumulated
unusually large peaks in several user-space units.

---

## 2. Timeline

Last 15 minutes of boot `-1`, then the shutdown/reboot boundary. `sar` rows are from
`/var/log/sysstat/sa21` (sampled every 10 min, so the last snapshot is 5 min 27 s
before the kill); everything else is journal output.

| Time | Source | Event |
|---|---|---|
| 16:30:06 | journald | `Starting sysstat-collect.service` — routine |
| 16:34:05 | journald | `anacron.service` — `Normal exit (0 jobs run)` |
| 16:34:23 | journald | Nautilus activated by `xdg-desktop-portal-gnome` (portal request); repeats `16:37:14` |
| 16:34:24 | Chrome | `CompositorAnimationObserver is active for too long (1631.54s)` |
| 16:35:01 | Chrome | `Request cancelled by user.` |
| 16:40:05 | `sar -r` | **Last sample before the event**: `kbmemfree` 2.9 GiB, `kbavail` 12.0 GiB, `kbmemused` 14.4 GiB (`%memused` 47.23), `kbcommit` 46.8 GiB (`%commit` 72.83) |
| 16:40:05 | `sar -S` | Swap used 1.9 GiB of 33.8 GiB (`%swpused` 5.62) — swap was **not** exhausted |
| 16:40:05 | `sar -u`, `sar -q` | CPU `95.27` %idle, `ldavg-1` 0.90 — the machine was **not** compiling at this instant |
| 16:40:05 | `sar -B` | `majflt/s` 6.56, `pgscank/s` 484.29, `pgsteal/s` 947.01 — reclaim already running |
| ~16:42:45 | derived | Start of the oomd pressure window, back-computed from `Total: 2min 47s` at 16:45:32 — **inference, not observed** |
| 16:45:32 | systemd-oomd | `Killed /user.slice/.../org.gnome.Shell@ubuntu.service due to memory pressure ... 82.22% > 50.00% for > 20s`; `Pressure: Avg10: 81.26, Avg60: 31.87, Avg300: 7.90, Total: 2min 47s`; `Current Memory Usage: 21.3G`; `Pgscan: 155781882` |
| 16:45:32 | gnome-shell clients | `Lost connection to Wayland compositor` (gnome-calendar, xdg-desktop-portal-gnome); `Error reading events from display: Broken pipe` (terminal, Chrome, dropbox, gedit, solaar, blueman, update-notifier, evolution-alarm-notify) |
| 16:45:32 | systemd\(4282\) | `org.gnome.Shell@ubuntu.service: Main process exited, code=killed, status=9/KILL` |
| 16:45:33 | systemd\(4282\) | `Failed with result 'oom-kill'`; `Triggering OnFailure= dependencies`; `Consumed 3d 4h 4min 52.315s CPU time over 3w 1d 18h 28min 2.531s ..., 26.7G memory peak, 22.6G memory swap peak` |
| 16:45:36 | systemd-logind | `Session 2 logged out`; `New session 'c2' of user 'gdm-greeter' ... type 'wayland'` — greeter takes over |
| 16:45:37 | gnome-shell\(2123022\) | `Running GNOME Shell (using mutter 50.1) as a Wayland display server` (GDM greeter) |
| 16:45:46 | systemd\(4282\) | `Stopped user@1000.service`; `session.slice: ... 26.9G memory peak, 22.8G memory swap peak`; `user@1000.service: ... 27.7G memory peak, 31.2G memory swap peak` |
| 16:45:53 | systemd-logind | **`Power key pressed short.`** — human-initiated |
| 16:45:57 | journald | `systemd-poweroff.service has 'start' job queued`; `rfkill: input handler enabled` |
| 16:46:18 | kernel | `wlp59s0: deauthenticating ... (Reason: 3=DEAUTH_LEAVING)` |
| 16:46:19 | systemd | Swap deactivated, snap mounts unmounted, `EXT4-fs (nvme0n1p9): unmounting filesystem` |
| 16:46:21 | systemd | `Reached target shutdown.target`, `Reached target poweroff.target` |
| 16:46:21 | systemd-shutdown | `Syncing filesystems and block devices.` / `Sending SIGTERM to remaining processes...` |
| 16:46:21 | systemd-journald | `Received SIGTERM from PID 1 (systemd-shutdow)` → `Journal stopped` — **end of boot `-1`** |
| — | — | gap **2 min 12 s** |
| 16:48:33 | kernel | `Linux version 7.0.0-15-generic ...` — start of boot `0` (`594c7cae...`) |
| 16:48:34 | systemd-fsck | `/dev/nvme0n1p9: clean, 5576907/46022656 files, 172417005/184081920 blocks` — clean unmount, no journal recovery |

Reproduce the boundary:

```
$ journalctl --list-boots --no-pager
 -1 905b047c5c4f40b6a6e9d6578d849e24 Mon 2026-09-28 01:55:02 EEST Wed 2026-10-21 16:46:21 EEST
  0 594c7cae24b743349b2ab6b3502082cc Wed 2026-10-21 16:48:33 EEST Wed 2026-10-21 16:50:16 EEST

$ journalctl -b -1 --no-pager --output=short-iso | tail -1
2026-10-21T16:46:21+03:00 ... systemd-journald[365]: Journal stopped
```

---

## 3. Evidence

| # | Hypothesis | Command | Result | Verdict |
|---|---|---|---|---|
| 1a | Kernel OOM killer killed the session | `journalctl -k -b -1 --no-pager \| grep -iE "out of memory\|oom\|killed process\|invoked oom-killer"` | No `invoked oom-killer`, no `Killed process`. Only `OOM killer disabled.` / `enabled.` at `2026-10-17 00:35:25` and `2026-10-20 16:47:22` — suspend/resume pairs, unrelated to 21 Oct | **ruled out** |
| 1b | `systemd-oomd` killed the session | `journalctl -b -1 --no-pager \| grep -iE "oomd\|memory pressure"` | `16:45:32 ... Killed /user.slice/user-1000.slice/user@1000.service/session.slice/org.gnome.Shell@ubuntu.service due to memory pressure for /user.slice/user-1000.slice/user@1000.service being 82.22% > 50.00% for > 20s with reclaim activity`; preceding line `systemd[4282]: org.gnome.Shell@ubuntu.service: systemd-oomd killed some process(es) in this unit.`; subsequent `Failed with result 'oom-kill'` | **supported** (primary cause) |
| 1c | `earlyoom` was involved | `which earlyoom` | no path printed — not installed | **ruled out** |
| 2 | NVIDIA / DRM failure (Xid, NVRM, GPU hang, driver reset) | `journalctl -k -b -1 --no-pager \| grep -iE "nvidia\|nvrm\|xid\|drm\|gpu\|gsp"` | Zero Xid errors. The only `NVRM` line in the whole boot is `2026-09-28T01:55:04 ... NVRM: loading NVIDIA UNIX x86_64 Kernel Module 580.159.03`. No GPU reset, no atomic-commit failure, no nvrm event near 16:45. Boot `0` loads the same driver cleanly (`NVRM: loading ... 580.159.03`) | **ruled out** |
| 3 | GNOME / Wayland component crashed (`gnome-shell`, `mutter`, `Xwayland` segfault/abort) | `coredumpctl --no-pager list \| grep "2026-10-21"` ; `... \| grep -iE "gnome-shell\|mutter\|Xwayland"` | 0 coredumps on 2026-10-21. The only gnome-shell entries ever are `2026-06-03 14:04:08` and `14:08:59` (SIGSEGV, months earlier). Journal shows `status=9/KILL` and `result 'oom-kill'`, i.e. an external SIGKILL, not a fault | **ruled out** (death was a kill, not a crash) |
| 4 | Disk exhaustion (`ENOSPC`) at the time of the event | `journalctl -b -1 --no-pager \| grep -iE "no space left\|enospc\|read-only file system"` ; `df -h` ; `df -i /home` | Only one ENOSPC in the boot, on **2026-10-20 02:23:22** (`failed to commit changes to dconf: ... /home/john/.config/dconf/user.EPGDX3: write() failed: No space left on device`). No ENOSPC on 21 Oct. Post-boot `/home` = 691 G, 646 G used, 9.4 G avail (99 %); inodes 13 % used. `/` = 91 % used. `systemd-journald` reported `1.9G, max 4G` and never hit its cap. The only `Read-only file system` hits are `fwupd` failing to open `/dev/sda1` (a removable/ISO device, repeated on 29 Sep, 3/7/13/17 Oct), unrelated to `/home` or `/` | **ruled out as cause**; **supported as a background condition** (see §5) |
| 5 | Kernel panic / MCE / thermal / watchdog / hardware | `journalctl -k -b -1 --no-pager \| grep -iE "kernel panic\|mce\|machine check\|thermal\|critical\|watchdog\|hardware error"` (counted) ; `ls -la /var/lib/systemd/pstore/` | Count `0` for machine-check/panic/hardware-error; count `0` for thermal-critical/throttling. `/var/lib/systemd/pstore/` is empty. `/var/crash/` holds only an Oct-20 Chrome core, an Oct-20 `gsd-housekeeping` core, and a zero-byte `kdump_lock` created at `16:48` | **ruled out** (with pstore caveat, §7) |
| 6 | Clean reboot after the session died (shutdown upstream of the crash) | `journalctl -b -1 \| grep -iE "poweroff.target\|systemd-shutdown\|Journal stopped\|Power key"` ; `journalctl -b 0 \| grep -iE "recovering journal\|fsck"` | `16:45:53 systemd-logind: Power key pressed short.` → `16:46:21 Reached target poweroff.target` → `systemd-shutdown[1]: Syncing filesystems...` → `Journal stopped`. Boot `0` shows no ext4 journal recovery and `fsck` reports `clean` | **supported** — the shutdown was clean and user-initiated, and therefore *not* the crash |
| 7 | Memory pressure / overcommit was the proximate mechanism | `sar -r -f /var/log/sysstat/sa21` ; `sar -S` ; systemd `Consumed ... memory peak` accounting | `%commit` 24.93 at 11:30 → 71.95 at 11:50 → 81.15 at 13:30 → 88.01 at 14:50 → **98.00 at 15:30** → 74.37 at 15:50 → 72.83 at 16:40. Peaks: `org.gnome.Shell@ubuntu.service` 26.7 GiB mem / 22.6 GiB swap, `session.slice` 26.9 GiB / 22.8 GiB, `app.slice` 13.2 GiB / 8.3 GiB, `user@1000.service` **27.7 GiB / 31.2 GiB** | **supported** |
| 8 | Parallel `cargo test` runs caused the spike | `sar -u`, `sar -q`, `du -sh ferrocad/target` | At 16:40:05 CPU was `95.27` %idle with `ldavg-1` 0.90 — no compilation was saturating the box at the last sample. `target/` is 25 G and its newest dependency artifacts are from `2026-10-20 17:54`, i.e. nothing was rebuilt on 21 Oct that persisted | **inconclusive** |

---

## 4. Verdict

**OOM — a userspace OOM kill by `systemd-oomd` against the shell — is the cause of the
session loss.** NVIDIA/DRM, GNOME/Wayland faults, disk exhaustion, kernel panic and
hardware are all ruled out by direct absence of their signatures. The log did not
"just stop": boot `-1` contains a complete, orderly shutdown, so the reboot hypothesis
is correct only in the trivial sense that the shutdown came *after* the session died,
`poweroff.target` reached at `16:46:21` against a kill at `16:45:32`.

The mechanism is fully documented by the killer itself:

```
$ journalctl -b -1 --no-pager | grep -E 'systemd-oomd\[606\]|oom-kill' | tail -12
2026-10-21 16:45:32 systemd[4282]: org.gnome.Shell@ubuntu.service: systemd-oomd killed some process(es) in this unit.
2026-10-21 16:45:32 systemd-oomd[606]: Considered 68 cgroups for killing, top candidates were:
2026-10-21 16:45:32 systemd-oomd[606]:         Path: /user.slice/user-1000.slice/user@1000.service/session.slice/org.gnome.Shell@ubuntu.service
2026-10-21 16:45:32 systemd-oomd[606]:                 Memory Pressure Limit: 0.00%
2026-10-21 16:45:32 systemd-oomd[606]:                 Memory Pressure Duration: 0
2026-10-21 16:45:32 systemd-oomd[606]:                 Pressure: Avg10: 81.26, Avg60: 31.87, Avg300: 7.90, Total: 2min 47s
2026-10-21 16:45:32 systemd-oomd[606]:                 Current Memory Usage: 21.3G
2026-10-21 16:45:32 systemd-oomd[606]:                 Memory Min: 0B
2026-10-21 16:45:32 systemd-oomd[606]:                 Memory Low: 0B
2026-10-21 16:45:32 systemd-oomd[606]:                 Pgscan: 155781882
2026-10-21 16:45:32 systemd-oomd[606]:                 Last Pgscan: 155745018
2026-10-21 16:45:32 systemd-oomd[606]: Killed .../org.gnome.Shell@ubuntu.service due to memory pressure for
    /user.slice/user-1000.slice/user@1000.service being 82.22% > 50.00% for > 20s with reclaim activity
```

The last line is wrapped and its leading path abbreviated (`...`) for width; the full
path is the `Path:` line above.

The policy that fired is the stock Ubuntu one, not local tuning:

```
$ grep -r "" /usr/lib/systemd/system/user@.service.d/10-oomd-user-service-defaults.conf
[Service]
ManagedOOMMemoryPressure=kill
ManagedOOMMemoryPressureLimit=50%
```

`/etc/systemd/oomd.conf` has no active settings (`grep -vE '^\s*#|^$'
/etc/systemd/oomd.conf` prints only `[OOM]`), and there are no `user@.service.d` /
`user.slice.d` drop-ins in `/etc`.

### What the evidence cannot distinguish

- **Which allocation filled the shell's cgroup.** oomd attributes 21.3 GiB to
  `org.gnome.Shell@ubuntu.service`, but that is a cgroup total; it does not identify a
  window, client buffer, extension or library. The journal logs *nothing* between
  `16:37:14` (Nautilus activation) and `16:45:32` apart from once-a-minute
  `bluetoothd` "Host is down" lines.
- **Whether the 16:42/16:45 spike was caused by the coding agents at all.** At the last
  sample (16:40:05) the box was 95 % idle with 12.0 GiB available and the two
  `cargo test` runs are not visible in the journal. The pressure window
  (`Total: 2min 47s`) is the only timing anchor, and it is back-computed, not observed.
- **Whether gnome-shell held 21.3 GiB of anonymous memory or a large buffer mapping.**
  Both would look identical to `memory.current`; only a memory profile of the live
  process, which no longer exists, could tell them apart.
- **Whether the kernel would have OOM-killed something on its own.** It never got the
  chance: oomd acted first at 82.22 % pressure.
- **The state of GPU memory.** No `nvidia-smi` history exists and no GPU error was
  logged; a silent VRAM-to-system-memory fallback cannot be excluded or confirmed.
- **Whether `gnome-shell` was already carrying the 26.7 GiB peak before this event.**
  systemd reports only a lifetime peak (since 2026-09-28), not a curve.

---

## 5. Contributing factors

1. **A 23-day-old, leaking session.** `journalctl --list-boots` shows boot `-1` up from
   `2026-09-28 01:55:02` to `2026-10-21 16:46:21`. Lifetime peaks at the moment of
   teardown (`journalctl -b -1 | grep "Consumed"`):

   | Unit | CPU time | Memory peak | Swap peak |
   |---|---|---|---|
   | `org.gnome.Shell@ubuntu.service` | 3 d 4 h 4 min | 26.7 GiB | 22.6 GiB |
   | `session.slice` | 3 d 4 h 52 min | 26.9 GiB | 22.8 GiB |
   | `app.slice` | 3 d 4 h 19 min | 13.2 GiB | 8.3 GiB |
   | `user@1000.service` | 6 d 9 h 26 min | 27.7 GiB | 31.2 GiB |

   A 30 GiB machine whose shell has, at some point, occupied 26.7 GiB plus 22.6 GiB of
   swap had no headroom left for a spike. Secondary long-lived peaks are also abnormal
   for their function: `dbus.service` 6.2 GiB, `app-gnome-google\x2dchrome-496494.scope`
   10.4 GiB, `plocate-updatedb.service` 3.6 GiB.

2. **Heavy overcommit earlier in the afternoon.** From `sar -r -f
   /var/log/sysstat/sa21`: `kbcommit` 16,797,548 KiB (16.0 GiB) at `11:30`,
   48,487,200 KiB (46.2 GiB) at `11:50`, 54,685,948 KiB (52.1 GiB) at `13:30`,
   59,306,660 KiB (56.6 GiB) at `14:50`, **66,038,836 KiB — 63.0 GiB, `%commit` 98.00 —
   at `15:30`**, then 50,117,460 KiB (47.8 GiB) at `15:50` and 49,076,456 KiB (46.8 GiB)
   at `16:40`. The commit limit was within 2 % of exhaustion 75 minutes before the kill.
   *Inference:* a rise from 16.0 to 46.2 GiB in one 20-minute window, and later steps of
   ~5 GiB, are consistent with large parallel Rust/link jobs; the journal itself carries
   no build output to prove it.

3. **Swap was free but pressure was high.** `sar -S` at `16:40:05` shows
   `kbswpused 1,990,240` of `33,432,988` KiB free — 1.9 GiB used of 33.8 GiB. The kill
   was not triggered by swap exhaustion. `vm.swappiness` is `60` and
   `vm.overcommit_memory` is `0` (`cat /proc/sys/vm/...`). *Inference:* the allocation
   outpaced reclaim, so pages were not moved to swap before PSI crossed the threshold.

4. **`/home` at 99 %.** 691 G total, 646 G used, **9.4 G available**, 13 % inodes. It
   did not cause this event, but it has bitten this host before: a dconf write failed
   with `No space left on device` on 2026-10-20 at `02:23:22`, and `coredumpctl` shows
   repeated `SIGBUS` dumps from `rust-lld` (2026-09-26/27 and 2026-10-20 17:51, two of
   them 233 MiB) and from `git` (2026-09-18/19/20). On a full filesystem, `rust-lld`
   mapping a large output file is a classic `SIGBUS` source. `ferrocad/target` alone
   measures 25 G (`du -sh`).

5. **Crash-dump handling competes for RAM.** `system-systemd\x2dcoredump.slice` has a
   lifetime peak of 4.3 GiB on this boot, and `/var/crash` still holds a 287 MB Chrome
   core from 2026-10-20. Dumping a multi-hundred-megabyte core on a memory-pressured,
   99 %-full disk is a plausible aggravator at exactly the wrong moment.

6. **Journald storage was not a factor.** `journalctl --disk-usage` → `2G`; journald
   started with `System Journal ... is 1.9G, max 4G, 2G free`, and its only pressure
   response was `Under memory pressure, flushing caches.` on 2026-10-20 19:04:43.

7. **Precedent on the same boot.** oomd had already killed
   `app-gnome-google\x2dchrome-496494.scope` on 2026-10-20 at `19:05:02` for the same
   reason (`82.87% > 50.00% for > 20s`). This was the second oomd kill in 24 hours and
   the second one aimed at the user slice broadly.

---

## 6. Prevention / next-time instrumentation

All items below are **recommendations only** — nothing in this list has been applied.

1. **Give the session headroom on `/home`.** `target/` is 25 G of a 691 G volume with
   9.4 G free. Prune with `cargo clean` in `ferrocad/` (and
   `spikes/occt-tessellation/`) or delete stale `target/debug/deps` generations; that
   alone removes the ENOSPC/SIGBUS class of failure. *Recommendation.*
2. **Serialize or bound the agent builds.** Two concurrent `cargo test` runs plus a
   shell share one target directory. Set `CARGO_BUILD_JOBS` (or
   `BUILD_JOBS`/`-j`) for background agents, and avoid two agents touching the same
   workspace. The repo already routes `rustc` through the `kache` wrapper
   (`ferrocad/.cargo/config.toml`), which is a good place to enforce a job ceiling.
   *Recommendation.*
3. **Do not let oomd pick the shell first.** Mark the session's compositor as a unit
   oomd should avoid so a leaf app dies instead:
   `ManagedOOMPreference=avoid` on `session.slice` (or on
   `org.gnome.Shell@ubuntu.service`). Keep `ManagedOOMMemoryPressure=kill` on
   `user@.service` so something still gets killed. *Recommendation; needs verification
   on this systemd version before adopting.*
4. **Cap the long-lived shell.** `MemoryHigh=`/`MemoryMax=` on `session.slice`, or a
   policy of logging out at least weekly, keeps a 26.7 GiB peak from becoming the
   baseline. A weekly `gnome-shell` restart also bounds the `dbus.service` (6.2 GiB)
   and browser peaks. *Recommendation.*
5. **Make the next spike attributable.** `sysstat` samples every 10 min, so the
   16:40:05 → 16:45:32 window is a blind spot. Drop the interval to 1 min for
   `-r -S -B -q`, or log a per-cgroup `memory.current`/`memory.stat` snapshot every
   60 s (`systemd-cgtop -m -d 60 -n 1` in a journald-backed loop). With either, the
   allocation that preceded this kill would be attributable to a unit. *Recommendation.*
6. **Bound coredump retention.** `/var/crash` holds a 287 MB Chrome core and
   `coredumpctl` shows 233 MiB `rust-lld` cores; systemd-coredump peaked at 4.3 GiB.
   Set `[Coredump] MaxUse=`/`KeepFree=` in `/etc/systemd/coredump.conf` so dumps cannot
   crowd a 9.4 GiB-free disk. *Recommendation.*
7. **`systemd-oomd` already logs everything needed** (candidate, pressure, pgscan,
   decision). No extra logging is required; the gap is memory *history*, not the kill
   record. *Observation.*
8. **NVIDIA logging is not a gap here.** Zero Xid/NVRM errors in boot `-1`; no
   `/var/log/nvidia*`. Enabling `nvidia-persistenced` logging would not have helped
   this incident and is not recommended on its evidence. *Recommendation: no change.*

---

## 7. What could not be verified

- **`/sys/fs/pstore/`** — `ls -la /sys/fs/pstore/` returns `Permission denied` (root
  only). The systemd-collected copy, `ls -la /var/lib/systemd/pstore/`, is **empty**,
  so no firmware/pstore record was captured either way. This does not weaken the
  "no panic" conclusion, which rests on the clean `poweroff.target` sequence.
- **`nvidia-smi`** — fails:
  `NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.`
  Cause: `/dev/nvidia*` does not exist in this investigation environment, while
  `lsmod` shows `nvidia`, `nvidia_drm`, `nvidia_modeset`, `nvidia_uvm` loaded and in
  use (`nvidia 104112128 52 nvidia_uvm,nvidia_drm,nvidia_modeset`). This is an
  environment/visibility limitation, not a driver fault — and in any case a
  post-reboot query would not describe the 16:45 state.
- **`systemctl status systemd-oomd`** — `Failed to connect to system scope bus via
  local transport: Operation not permitted` (no D-Bus access). The policy was
  therefore read from `/usr/lib/systemd/system/user@.service.d/10-oomd-user-service-defaults.conf`
  instead of from the live unit.
- **`last -x`** — `last: not found`. Reboot/shutdown records were reconstructed from
  the journal only.
- **`sar -F` (filesystem utilisation over time)** — `Requested activities not
  available in file /var/log/sysstat/sa21`. Free space at the moment of the crash is
  unknown; the nearest anchors are the `16:40:05` sar memory sample and the boot-`0`
  `fsck` counters (9.4 GiB available after reboot).
- **No `gnome-shell` coredump for 2026-10-21** — expected, since SIGKILL does not dump;
  `coredumpctl --no-pager list | grep "2026-10-21"` returns 0 rows. There is therefore
  no heap profile, no backtrace, and no `/proc/<pid>/smaps` from the victim.
- **No per-process accounting between 16:40:05 and 16:45:32.** The `16:42:45` pressure
  start is arithmetic (`16:45:32` minus `Total: 2min 47s`), labelled as inference
  throughout; the growth curve is unknown.
- **No evidence of what the agents were executing.** Their output lives in terminal
  scopes, which journald does not capture. `ferrocad/target`'s newest dependency
  artifacts are from `2026-10-20 17:54:57` (e.g.
  `zvariant_utils-e9256c2d37e7776e.d`), so no build from 21 Oct left a lasting artefact
  there — which is why hypothesis 8 is recorded as inconclusive rather than supported.
- **Cosmetic journal anomaly, not evidence:** `journalctl --list-boots` shows boot `-7`
  with `FIRST ENTRY Sun 2026-11-08 12:13:28 EET` but `LAST ENTRY Sat 2026-09-05
  20:57:09 EEST`, i.e. first > last. This is a clock-change artefact in that boot's
  records, months before the incident, and has no bearing on this analysis. It is noted
  only so the next reader does not mistake it for corruption.
