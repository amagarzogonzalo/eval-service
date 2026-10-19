# Oracle Cloud runner (Phase 3): research + setup plan

Status: researched 2026-10-19. Nothing here is provisioned yet.
This is the working document for Phase 3 of the agreed v2 plan
(2 OCPU / 12 GB, ARM64 proof, reclamation accepted, backups are the
guarantee).

## 1. What Phase 3 must deliver

- A **free** ARM64 host (Oracle Ampere A1, Ubuntu 24.04) that can run
  `eval` live suites unattended: proxy, budgets, store, cassettes.
- **ARM64 proof**: platform builds and tests green on `aarch64`, plus
  one real live cycle through `eval proxy` recorded to the store.
- **Cattle, not pet**: the host is re-creatable from this repo (IaC +
  cloud-init) and the eval store is backed up off-host; reclamation or
  termination must not lose evidence or keys.
- **No keepalive theatre**. We do not run synthetic load to dodge
  Oracle's idle reclamation. See §3.

Out of scope here: Docker executor (Phase 4), watch/dashboard
(Phase 5), PR gating and split tokens (Phase 6), judge calibration
(Phase 7).

## 2. Verified Always Free facts (as of Oct 2026)

Sources: Oracle Always Free Resources doc, OCI signup/FAQ, Budgets
and Quotas docs (links in §11).

| Resource | Always Free allowance | Notes |
|---|---|---|
| Ampere A1 (ARM) | 1,500 OCPU-hours + 9,000 GB-hours/month = **2 OCPU / 12 GB continuous**, home region only | Doc now states the equivalent of 2 OCPU/12 GB; the larger 4/24 allowance was reduced in 2026 — verify limits in the console before sizing |
| AMD micro | 2 × VM.Standard.E2.1.Micro (1/8 OCPU, 1 GB) | fallback control host, not the ARM runner |
| Block volume | **200 GB** total (boot + block), **5 volume backups** | 50 GB default boot volume; 47 GB minimum; boot volume backups count against the 5, keep boot ≤ 50 GB |
| Object Storage | **20 GB** combined standard/IA/archive + 50k API req/month | home region; versioning + lifecycle for the store backup |
| Egress | **10 TB/month** free public egress | evals are tiny; the real risk is a runaway target, cap at the target layer |
| Network gateways | VCN, subnets, route/security lists, internet gateway, **NAT gateway**, service gateway carry no billed line item | this is what makes a private subnet with outbound internet free |
| Bastion | free for free + paid accounts | targets must be resources without public endpoints (private subnet) |
| Vault | 150 secrets, software-protected master keys free | where LLM keys and GitHub tokens live |
| Resource Manager | 100 stacks, 2 concurrent jobs, 1 private endpoint | runs the same Terraform in OCI; no local state file to lose |
| Monitoring / Notifications | 500M ingestion + 1B retrieval points; 1M HTTPS / 1000 email notifications per month | alarms + events for reclamation/termination |
| Logging | 10 GB/month shared (free tenancies) | journald shipper is optional |
| Load balancer | 1 flexible 10 Mbps | **not needed**: the runner has no inbound service; skip it |

Signup reality: credit/debit card required (no prepaid/virtual
cards), one account per person, **home region cannot be changed after
signup**, and accounts idle 30+ days may be deemed abandoned
(suspension/termination risk). Region choice is therefore a one-way
door: pick a region with A1 capacity and low latency to the LLM
providers (EU candidates: Frankfurt, Amsterdam, Madrid, Stockholm).

## 3. Reclamation reality (the design constraint)

Oracle's published policy: an Always Free instance is deemed idle if,
over a 7-day window, **all** of CPU (p95), network, and memory (A1
only) stay below **20%**. Notices arrive by email; the documented
action is **stopping** the instance, one week later. Boot and block
volumes remain and the instance can be restarted "as long as the
shape is available" — capacity is not guaranteed after a stop.

Community reports describe stops with email warnings, and Oracle's
terms allow termination for abandoned/abusive accounts. Treat the
instance as reclaimable at any time.

Consequences baked into this plan:

- The **store** and **run evidence** live in Object Storage (and are
  re-syncable to a fresh host in minutes).
- The **host** is defined by `infra/oracle/` + cloud-init; rebuild is
  the recovery path, not a ticket to Oracle.
- **Secrets** are in Vault; a rebuilt host fetches them with its own
  instance identity (no key material on disk or in the image).
- Upgrading to **Pay As You Go** is the only legitimate way to opt out
  of idle reclamation (stays $0 within Always Free limits, and is also
  the reliable fix for out-of-capacity). The plan works without it;
  the decision is explicitly deferred (see §10).
- We do **not** run `stress-ng`/busy loops to stay above the idle
  thresholds. That is the "keepalive claim" this plan rejects: it
  risks the account and proves nothing.

## 4. Target architecture

```
admin workstation ──(Bastion session, SSH port-forward)──┐
                                                          │
 VCN 10.0.0.0/16                                          v
 ├── public subnet 10.0.0.0/24 ── IGW ── Bastion (managed, free)
 └── private subnet 10.0.1.0/24 ── NAT gateway ── internet egress
        │                         └─ service gateway ── Object Storage
        └── runner VM: Ubuntu 24.04 aarch64, A1.Flex 2/12, 50 GB boot
            ├── /opt/eval/bin/eval        (release build from this repo)
            ├── /srv/eval-service         (pinned checkout, platform code)
            ├── /srv/targets/<project>    (read-only target checkouts)
            ├── /srv/eval-venvs/<project> (target deps, never inside target)
            ├── /var/lib/eval-runs        (store root; synced to bucket)
            ├── systemd: eval-backup.timer, eval-watch.service (Phase 5)
            └── egress: github.com, LLM providers, OCI APIs
```

No public IP on the instance. Outbound internet via NAT (free);
management via Bastion port-forward sessions (free) from allowlisted
admin CIDRs. If Bastion is too awkward, the fallback is Tailscale on
the VM (free tier) — but that is a third-party dependency and still
needs a NAT egress path; prefer Bastion first.

Docker (Phase 4) will later run targets on an internal-only Docker
network whose only route out is the loopback proxy; the host network
stays private.

## 5. Provisioning runbook

### 5.1 Order of operations

1. **Account**: sign up, choose home region, enable MFA, create a
   dedicated compartment `eval` and an admin group/user.
2. **Guardrails first** (before any compute): budget + quota + alarm
   so a mistake cannot silently bill (see §8).
3. **Network + Bastion + buckets + Vault** via Terraform
   (`infra/oracle/`), applied through Resource Manager or local CLI.
4. **Instance** via the capacity playbook (§5.3); cloud-init does the
   rest.
5. **Verify** (§9), then **back up** and **drill the rebuild**.

### 5.2 IaC layout (to add in this repo)

```
infra/oracle/
  versions.tf        # oci provider, region var
  variables.tf
  network.tf         # VCN, public/private subnets, IGW, NAT, SGW,
                     # route tables, security lists/NSGs
  bastion.tf         # oci_bastion_bastion (STANDARD, private target
                     # subnet, client CIDR allowlist)
  identity.tf        # dynamic group + read policies for instance
  storage.tf         # store bucket (versioning on), backup bucket
  vault.tf           # vault + AES key (secret values created manually)
  monitoring.tf      # notification topic, subscription, event rule
                     # (instance stopped/terminated), budget + alerts
  quotas.tf          # oci_limits_quota: cap the free allowance
  instance.tf        # A1.Flex 2/12, 50 GB boot, Ubuntu 24.04 aarch64,
                     # no public IP, user_data = cloud-init.yaml
  cloud-init.yaml    # bootstrap (below)
  terraform.tfvars.example
```

Key resource sketches (verify attributes against the provider docs
at apply time):

- `oci_core_instance`: `shape = "VM.Standard.A1.Flex"`,
  `shape_config { ocpus = 2, memory_in_gbs = 12 }`,
  `source_details { source_type = "image", source_id = <Ubuntu 24.04
  aarch64 image>, boot_volume_size_in_gbs = 50 }`,
  `create_vnic_details { subnet_id = <private>, assign_public_ip =
  false }`, `metadata { ssh_authorized_keys, user_data }`.
- `oci_bastion_bastion`: `bastion_type = "STANDARD"`,
  `target_subnet_id = <private subnet>`, `client_cidr_block_allow_list
  = [admin/32]`.
- `oci_limits_quota` statement example (confirmed name):
  `set compute-core quota standard-a1-core-count to 2 in compartment
  eval`. Generate the remaining statements (memory, block storage,
  volume backups) from **Console → Limits, Quotas and Usage → Create
  Quota Policy Stub**; do not guess quota names.
- Object Storage buckets: `versioning = "Enabled"`, lifecycle to
  expire only after the backup keeps ≥ N versions; the 20 GB cap makes
  lifecycle mandatory.
- Vault: create vault + key in Terraform; create **secret values**
  with `oci vault secret create-base64` (or the console). Never put a
  secret value in `terraform.tfvars` or state.

### 5.3 Capacity playbook (the "Out of host capacity" lottery)

Confirmed tactics, in order:

1. **Rotate availability domains**: most home regions have 3 ADs;
   capacity often exists in only one. Never pin a fault domain.
2. **Launch small, resize up**: create 1 OCPU / 6 GB (rarely fails),
   then `oci compute instance update --shape-config '{"ocpus":2,
   "memoryInGBs":12}'`. Resizing an existing instance succeeds far
   more often than a fresh 2/12 launch.
3. **Patient retry**: poll on the order of minutes with jitter, not
   seconds. Retry only capacity-class errors (`500 InternalError`,
   `429`), stop on auth/permission/quota errors. Use an
   `opc-retry-token` so a timed-out request cannot create a second
   instance. Prior art: `oci-arm-catcher` and
   `oci-capacity-aware-provisioner`.
4. **Optional**: PAYG upgrade gives capacity priority (and removes
   idle-stop). Same $0 within Always Free limits.
5. Never fall back to a paid shape; if capacity never appears,
   reconsider region (one-way door) or run Phase 3 on the 2 × AMD
   micro fallback for the harness smoke — ARM64 proof still requires
   A1.

### 5.4 cloud-init bootstrap (outline, not final code)

- users: `eval` (systemd services), admin SSH key only; no root
  password login.
- packages: `git`, `build-essential`, `pkg-config`, `curl`, `jq`,
  `unzip`, `python3`, `python3-venv`, `python3-dev`, `docker.io`
  (or Docker CE arm64), `fail2ban`, `unattended-upgrades`, `chrony`,
  `ufw`, `ca-certificates`.
- rust: `rustup` for `eval` user; install stable + 1.85 (MSRV);
  `cargo build --release -p eval-cli` at first boot.
- target venvs: `python3 -m venv /srv/eval-venvs/<project>` and
  `pip install -r <target>/requirements.txt` — **outside** the target
  checkout; `PYTHONPATH=<target>/src` comes from the adapter env.
  (digest_agent needs langgraph/pydantic/reportlab; all have arm64
  wheels today, keep `build-essential` as the source-build fallback.)
- systemd units: `eval-backup` (timer), `eval-watch` (Phase 5),
  journald `SystemMaxUse=512M`, timezone UTC.
- swap: 2 GB swapfile or zram; 12 GB RAM is plenty but a runaway
  target should not OOM the host unnoticed.

### 5.5 Access

- Preferred: Bastion **port-forwarding session** (no agent dependency)
  to port 22. Managed SSH also works once the Oracle Cloud Agent's
  Bastion plugin is enabled (on by default on Oracle platform images,
  not always on Canonical images).
- Security list: ingress to the private subnet only from the bastion
  subnet on 22; no ingress from 0.0.0.0/0 anywhere.
- Alternative if Bastion proves painful: Tailscale on the VM (free
  plan), no inbound ports. Keep it behind a decision (§10).
- Host firewall (ufw): deny inbound, allow outbound; fail2ban on sshd;
  key-only SSH; `AllowUsers admin eval`.

### 5.6 Secrets (Vault + instance principals)

- Dynamic group matching the runner instance, policy:
  ```
  Allow dynamic-group eval-runner-dg to read secret-family in compartment eval
  Allow dynamic-group eval-runner-dg to read buckets in compartment eval
  Allow dynamic-group eval-runner-dg to manage objects in compartment eval
  ```
  (Tighten `manage objects` to `use objects` plus explicit read if the
  backup path only needs create/overwrite; verify verbs against the
  policy reference at apply time.)
- Services fetch at job start with `--auth instance_principal`:
  `oci secrets secret-bundle get ...` → inject into the `eval proxy`
  process environment only. Nothing is written to the store, cassettes
  (cassette records never include Authorization headers), or logs.
- Rotation: rotate provider keys in Vault; the runner picks them up on
  the next job. GitHub split tokens (Phase 6) live here too: one
  read-only token for polling, one narrow-write token for comments and
  statuses.

### 5.7 Backup & DR

| What | Where | How often | Restore |
|---|---|---|---|
| Eval store (`/var/lib/eval-runs`) | Object Storage bucket, versioning | timer, ~hourly + after each run | `oci os object sync` reverse into a fresh host |
| Boot volume | OCI backup policy (5 free backups) | daily, keep ≤ 3 | boot-volume backup → new instance if capacity allows |
| Platform code | this git repo | pin a commit per job | clone at pinned SHA |
| Host config | `infra/oracle/` + cloud-init in this repo | every change | Resource Manager apply |
| Secrets | Vault | rotate quarterly or on suspicion | re-fetch by instance principal |

Keep boot volume ≤ 50 GB so boot + 5 backups stay inside the 200 GB /
5-backup always-free envelope (the backups-per-volume storage
accounting is not clearly documented; leaving headroom is cheap).

**Rebuild drill (Phase 3 exit)**: terminate the instance, provision a
new one via the runbook, restore the store, run one live suite and one
cassette replay, verify the usage report matches the pre-termination
numbers. Record the drill as evidence.

### 5.8 Monitoring and cost safety

- **Budget** (root compartment, targets `eval`): amount $0.01 with
  ACTUAL ≥ 100% alert to email; budgets evaluate every 24 h.
- **Quota**: cap A1 cores/memory and block storage to the free
  allowance (so a typo cannot create a paid shape).
- **Events**: rule on Compute instance state change
  (stop/terminate/reboot) → Notifications topic → email; this is the
  reclamation tripwire.
- **Heartbeat** (optional, Phase 5): the runner writes a small
  `heartbeat.json` to the backup bucket each job; an external check
  (GitHub Actions scheduled workflow with read-only OCI access, or a
  cron on the admin workstation) alerts when it goes stale.
- **Logs**: journald capped; ship to OCI Logging only if needed
  (10 GB/month shared).
- Never: reserved public IP unattached, load balancer, NAT billed
  line items (currently none), cross-region traffic, paid shapes.

## 6. GitHub integration model

- Phase 3 smoke uses manual runs over SSH/Bastion.
- From Phase 5, the VM runs a poller (`eval watch`): read-only token
  polls the platform repo for eval requests; jobs check out the
  platform at a pinned SHA and targets read-only at pinned SHAs.
- **No self-hosted GitHub Actions runner on this host.** PR content is
  untrusted input; running repository workflows on the host that holds
  provider keys is exactly the boundary the plan forbids. If a runner
  is ever wanted, restrict it to `main`-only workflows behind a
  protected environment and ephemeral containers — decided in Phase 6,
  not now.
- Posting results (statuses/comments) uses the narrow write token in
  Phase 6; Phase 3 evidence is the store + report output only.

## 7. ARM64 proof checklist (Phase 3 exit)

1. `uname -m` = `aarch64`; `free -h` ≈ 12 GiB; 2 OCPUs visible.
2. `cargo fmt --all -- --check`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo test --workspace --locked`
   green on the VM (EVAL_PYTHON=python3).
3. `eval check integrations/digest_agent/live_bench.yaml --probe` OK.
4. One live cycle through `eval proxy` (OpenRouter or Groq),
   `--workers 1`, baseline → work → compare PASS, usage report with
   provider-reported tokens.
5. One cassette record + offline replay cycle on the VM.
6. Store synced to the bucket; restore verified on a fresh path.
7. Terminate-and-rebuild drill completed within the target RTO
   (≤ 1 h with capacity available).
8. Reclamation alarm test-fired; budget and quota visible in console.

## 8. Open questions (decide before provisioning)

1. **Free vs PAYG account**: PAYG removes idle stops and eases
   capacity, still $0 within limits — but requires a card on file and
   discipline. The plan is written to work free; PAYG is the fallback.
2. **Home region**: one-way choice; prefer A1 capacity + provider
   latency (EU if the operator is in Europe). Verify A1 availability
   in the console's capacity/limit view before committing.
3. **Bastion vs Tailscale** for admin access.
4. **Secrets source of truth**: OCI Vault (recommended here) vs
   GitHub environment secrets (simpler, couples the runner to
   Actions).
5. **Store retention**: how many versions/days the 20 GB bucket
   keeps (evidence vs cost).

## 9. Risks

| Risk | Mitigation |
|---|---|
| Idle reclamation stop; restart fails on capacity | Store backed up; rebuild path; alarm on state change; PAYG option |
| Out-of-host capacity blocks first launch | AD rotation, 1/6→2/12 resize, patient retry, PAYG priority |
| Signup rejected / card issues | One account per person; valid card; support path |
| Home region wrong or capacity-starved | Decide before signup; fallback plan is a different tenancy choice, not a migration |
| Account deemed abandoned (30+ days idle) | Runner poller provides regular activity; monitoring email |
| arm64 wheel gaps in target deps (reportlab/orjson) | `build-essential` fallback; test venv build early |
| Secret exposure via target | Phase 4 Docker isolation + proxy-only egress; Vault tokens scoped; no host env for containers |
| Runaway target egress | Docker network egress control (Phase 4); host egress rules as a second layer |
| Backup window loses last run | Sync after each run, not only hourly |
| Cassette replay not satisfying live gates | `origin=replay` labeling; live baselines only from live runs (already enforced by policy, Phase 6 formalizes it) |

## 10. Alternatives if Oracle signup is blocked

Researched 2026-10-19. None of these replaces the Oracle always-free
2 OCPU/12 GB runner; they are fallbacks.

**Azure free account (not a permanent free VM).** The $200 credit
expires after **30 days**; the 12-month free VMs are B1s, B2ats v2,
and B2pts v2 — all **1 GiB RAM** (B2pts v2 is ARM64, 2 vCPU/1 GiB),
750 h/month each, plus 2 × 64 GiB P6 disks. There is **no always-free
VM**; after 12 months B2pts is billed (~$6/mo), and a 4 GiB ARM
B2pls v2 is ~$16/mo, a 2/8 D2ps_v5 ~$56/mo. So Azure can host the
Phase 3 proof for 30 days on credit (D2ps_v5 or B2pls v2) or run a
1 GiB health probe for a year — it cannot replace the 2/12 runner.

**GitHub-hosted ARM64 runners (best zero-server free option).** Since
Jan 2026, `ubuntu-24.04-arm` standard runners work in private repos
at **2 vCPU/8 GiB**, and usage counts against the plan's included
minutes (2,000/month on GitHub Free). They are ephemeral: persist
`eval-runs`/baselines as artifacts or a dedicated store branch, and
run live suites only via manual/scheduled workflows (never on PRs).
The target shares the VM with the proxy, so `/proc` could read the
proxy environment until the Docker isolation of Phase 4 exists — do
not treat PR checkouts as untrusted on this path yet.

**Cheap paid fallback.** Hetzner CAX11 (2 vCPU/4 GiB ARM) or similar
is ~€4/month, with persistent disk and no capacity lottery. Not free,
but the pragmatic choice if both free paths fail.

Decision: keep Oracle as the plan; if signup/capacity is the blocker,
fix that first (PAYG within Always Free limits is still $0 and gets
capacity priority). If Oracle is impossible, GitHub ARM runners are
the only free ARM64 path with enough RAM; Azure is a 30-day bridge.

## 11. References

- Always Free resources: https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm
- Signup + FAQ (home region fixed, card rules): https://docs.oracle.com/en-us/iaas/Content/GSG/Tasks/signingup_topic-Sign_Up_for_Free_Oracle_Cloud_Promotion.htm , https://www.oracle.com/cloud/free/faq/
- Out of host capacity: https://docs.oracle.com/en-us/iaas/Content/Compute/Tasks/troubleshooting-out-of-host-capacity.htm
- Bastion: https://docs.oracle.com/en-us/iaas/Content/Bastion/Concepts/bastionoverview.htm
- Instance principals: https://docs.oracle.com/en-us/iaas/Content/Identity/Tasks/callingservicesfrominstances.htm
- Compartment quotas: https://docs.oracle.com/en-us/iaas/Content/Quotas/Concepts/resourcequotas.htm
- Budgets: https://docs.oracle.com/en-us/iaas/Content/Billing/Concepts/budgetsoverview.htm
- Terraform `oci_limits_quota`: https://registry.terraform.io/providers/oracle/oci/latest/docs/resources/limits_quota
- Capacity retry prior art: https://github.com/alexpua/oci-arm-catcher , https://github.com/its-spark-dev/oci-capacity-aware-provisioner
