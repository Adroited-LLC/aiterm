# Branching and release flow

**If you are an agent working in this repository, read this before creating a
branch, pushing, or opening a pull request. It is not advisory.**

Work moves in one direction, through gates that each answer a different
question. Nothing skips a step, and nothing is committed directly to a
promotion branch.

```
  your work  ──PR──▶  dev-pr  ──▶  alpha  ──▶  beta  ──▶  staging  ──▶  main
                        │           │           │           │           │
                     accepted    it builds   it works   it holds up   released
```

## The branches

| Branch | Question it answers | Who moves things into it |
|---|---|---|
| `main` | What is released? | A promotion from `staging`, deliberately. |
| `staging` | Does it hold up under real use? | Promotion from `beta` once verified. |
| `beta` | Does it actually work? | Promotion from `alpha` for testing. |
| `alpha` | Do the accepted changes integrate? | Promotion from `dev-pr`. |
| `dev-pr` | **Where every pull request lands.** | Merged PRs. |
| `dev-<username>` | One person's integration branch. | Its owner, freely. |
| `feat/…` `fix/…` `perf/…` `refactor/…` | One change. | Its author. |

## Rules

**1. Pull requests target `dev-pr`. Never `main`.**
A PR opened against `main` is targeting the wrong branch — retarget it rather
than merging it. `main` only ever moves by promotion from `staging`.

**2. One change per branch, cut from `dev-pr`.**

```bash
git fetch origin
git checkout -B fix/the-thing origin/dev-pr
```

Cut from a personal or promotion branch instead and the PR carries every other
change on it, which makes it unreviewable.

**3. Keep a contributed change to one commit.**
GitHub squash-merges. A one-commit PR comes back patch-identical, which is what
lets a downstream fork's rebase recognise it as landed and drop its local copy.
A three-commit PR squashed upstream matches none of the three, and whoever is
tracking this repo re-resolves the same conflict forever. Amend rather than
stack.

**4. Say when a change depends on another.**
Stacked work is fine — put `Stacks on #N` at the top of the description and
merge in order. Do not pretend a dependent branch is independent; its diff will
include the other change and confuse the review.

**5. Promotion is a merge, never a force-push.**
Promotion branches are shared. Rewriting one breaks every checkout of it.

```bash
git checkout alpha && git merge --ff-only origin/dev-pr && git push origin alpha
```

Use `--ff-only`. If it refuses, the target has commits the source does not —
which means someone committed directly to a promotion branch. Find out what
happened rather than forcing it through.

**6. Nothing is committed directly to `dev-pr`, `alpha`, `beta`, `staging` or
`main`.** They are outputs. The only exception is a hotfix, which still goes
through a PR into `dev-pr` and is then promoted straight up the chain — the
point of the chain is that the same commits reach `main` that were tested, and
a hotfix applied at the top has been tested nowhere.

## What each gate is actually for

The chain is only worth its overhead if the steps differ. They do:

- **`dev-pr`** — accepted, not trusted. A merged PR means a human agreed with
  the change, not that the result works with everything else merged that day.
- **`alpha`** — the first place the accepted changes exist *together*. Build and
  test suite must pass here. Breakage found here is integration breakage.
- **`beta`** — running it. Exercising the thing by hand, on a real machine, with
  real sessions. Most UI faults are only visible here.
- **`staging`** — living with it. Whatever `beta` proved works, `staging` proves
  keeps working. This is where a slow leak, a wrong default or an annoyance
  surfaces.
- **`main`** — what people install.

Skipping a gate does not save the time it costs; it moves the discovery later,
which is where it gets expensive.

## Downstream forks

A fork tracking this repo should mirror `main` (or `staging`, to run ahead) and
rebase its own patches on top. Rule 3 is what makes that work: keep contributed
patches to one commit and an accepted one disappears from the fork's stack on
its next sync, with no duplicate and no conflict.
