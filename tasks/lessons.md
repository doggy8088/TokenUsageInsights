# Lessons

## 2026-07-10 Rust exact test filter executed zero tests

- Classification: missing verification.
- Failure mode: `cargo test <short-name> -- --exact` matched no tests because Rust's exact name includes the module path.
- Detection signal: Cargo reported `running 0 tests` with the target test filtered out.
- Prevention rule: use the fully qualified name for `--exact`, or omit `--exact`; never accept a targeted test run unless at least one test executed.
- Tripwire: check the test summary for `1 passed` or an intentional failure before treating the command as evidence.

## 2026-07-10 Cross-realm JavaScript assertion

- Classification: incorrect assumption about test behavior.
- Failure mode: `assert.deepStrictEqual` rejected structurally identical arrays created inside `vm` because their prototypes came from a different JavaScript realm.
- Detection signal: the assertion reported identical displayed structures but non-reference-equivalent realm prototypes.
- Prevention rule: normalize `vm` results into host values or compare serialized primitive projections such as session ID arrays.
- Tripwire: when a `vm` assertion displays identical actual/expected values, check realm/prototype identity before treating it as a product failure.

## 2026-07-10 Session identifiers are untrusted input

- Classification: security/privacy oversight.
- Failure mode: the first cycle-safety patch used `Map` in the tree builder but left a plain-object lookup and raw session-ID interpolation in the table renderer.
- Detection signal: independent frontend review found prototype-key collisions and unescaped values inside `innerHTML` templates.
- Prevention rule: treat every field parsed from assistant logs as untrusted; use `Map` or null-prototype dictionaries for identifier keys and escape all HTML interpolation.
- Tripwire: search changed render paths for plain `{}` identifier maps and raw `${...id}` interpolation before UI verification.

## 2026-07-10 Migration semantics require a new marker

- Classification: unsafe change scope.
- Failure mode: migration behavior changed during review while retaining the same `v3` marker, which could let an environment that ran the earlier implementation skip the corrected logic.
- Detection signal: independent data review compared the marker value with the revised migration transaction.
- Prevention rule: whenever deployed or potentially executed migration semantics change, allocate a new monotonic marker even within the same development task unless non-execution is proven.
- Tripwire: before rollout, search the database and code for the proposed marker and confirm it uniquely identifies the final transaction semantics.

## 2026-07-10 Deployment must tolerate an already-stopped service

- Classification: environment-dependent assumption.
- Failure mode: the release switch assumed port 3003 would still have the previously observed listener and aborted when the service exited before deployment began.
- Detection signal: the switch stopped at its initial listener check before making any state change.
- Prevention rule: deployment scripts must branch safely for both running and already-stopped services while preserving the same backup and rollback guarantees.
- Tripwire: re-read listener/process state at action time and treat absence as a valid start path, not an exceptional failure.

## 2026-07-10 Readiness deadlines must include pre-bind migrations

- Classification: incorrect environment/runtime assumption.
- Failure mode: the deployment wrapper treated absence of a listener after 10 seconds as startup failure even though the process could still be performing a first-run reparse before binding.
- Detection signal: release build succeeded, the new process did not report an immediate exit, rollback succeeded, and the service architecture performs sync work before opening port 3003.
- Prevention rule: capture startup logs, distinguish process exit from slow readiness, and size readiness deadlines for migration work before terminating a new binary.
- Tripwire: after any interrupted migration attempt, allocate a fresh marker before retrying so partially written sync state cannot suppress the corrected rebuild.

## 2026-07-10 Session identity depends on metadata sequence

- Classification: incorrect assumption about repository data behavior.
- Failure mode: preferring `payload.id` over `payload.session_id` was insufficient because subagent rollouts contain a later embedded parent `session_meta` that overwrote the correct child identity.
- Detection signal: post-migration transcript/session count stayed at 9 despite 45 rollout files; metadata audit showed first-event IDs matched file UUIDs while later events did not.
- Prevention rule: model the complete event sequence in parser fixtures and lock immutable file/session identity at the first valid canonical metadata event.
- Tripwire: acceptance checks must compare retained transcript count with unique session identity count after a real reparse, not only check that self-parent count is zero.

## 2026-07-10 Multi-file patches require explicit file boundaries

- Classification: tooling error.
- Failure mode: a lesson-file hunk was placed before its `Update File` header, so `apply_patch` searched for lesson text inside `src/db.rs` and rejected the entire patch.
- Detection signal: patch verification named the wrong target file for the missing lesson heading.
- Prevention rule: every multi-file patch must introduce the next `Update File` header before any hunk context from that file.
- Tripwire: if a patch reports context from the wrong file, confirm atomic rejection and correct file boundaries before retrying.

## 2026-07-10 PowerShell variables are case-insensitive

- Classification: environment-dependent tooling error.
- Failure mode: `$home` was treated as the read-only built-in `$HOME`, so the HTTP smoke command failed before making its request.
- Detection signal: PowerShell reported `Cannot overwrite variable HOME because it is read-only or constant`.
- Prevention rule: use descriptive response names such as `$homeResponse` instead of identifiers that differ from built-ins only by case.
- Tripwire: treat PowerShell variable names as case-insensitive when reviewing inline verification scripts.

## 2026-07-10 Optional remote files must not fail release discovery

- Classification: environment-dependent tooling error.
- Failure mode: a diagnostic batch treated an optional remote `tasks/todo.md` lookup as mandatory, causing the combined command to return failure despite successfully reading the release tag and version.
- Detection signal: useful v0.1.1 output was present, but the batch exited non-zero only because the remote audit file did not exist.
- Prevention rule: probe optional paths separately or explicitly normalize their absence to success; only required release artifacts may gate discovery.
- Tripwire: label every remote read as required or optional before composing parallel release checks.

## 2026-07-22 Assistant selection must be captured across async UI work

- Classification: incorrect assumption about frontend state timing.
- Failure mode: asynchronous date, usage, and setup requests read mutable `currentAssistant` after a user switched agents, allowing stale Antigravity work to repaint the Grok empty state and modal.
- Detection signal: selecting Grok and opening its empty-state setup guide showed Antigravity copy or branding.
- Prevention rule: capture the normalized assistant at request/render start, use it for endpoint and UI payloads, and ignore the response if the current selection changed.
- Tripwire: exercise the sequence “select Grok → open setup guide immediately” and assert Grok title, Grok body, Grok logo, and `~/.grok/sessions` path; event handlers must wrap assistant-argument callbacks so the click event is not passed as the assistant.

## 2026-07-22 Shell quoting can alter JavaScript assertions

- Classification: missing verification.
- Failure mode: a shell single-quoted `node -e` assertion contained JavaScript single quotes, so the shell stripped part of the expected string and produced a false failing check.
- Detection signal: Node displayed an assertion string missing its embedded quotes.
- Prevention rule: keep JavaScript assertions free of shell quote delimiters or use a dedicated script/input mode when nested quoting is required.
- Tripwire: inspect the effective command text in the error before changing production code when a source-string assertion fails unexpectedly.

## 2026-07-22 Frontend asset versions are part of the implementation

- Classification: missing verification.
- Failure mode: changing JavaScript and adding localized UI content without bumping the imported i18n/CSS asset versions allowed a browser to combine new markup with stale dictionaries or styles.
- Detection signal: Grok setup content rendered translation keys and the visual layout did not match the current stylesheet even though the repository source was correct.
- Prevention rule: whenever static frontend content, translations, or CSS changes, update the corresponding cache-busting version and verify the served asset URLs.
- Tripwire: assert the homepage references the new `app.js`, `i18n.js`, and stylesheet versions, then fetch each asset before manual UI validation.

## 2026-07-23 Preserve the existing PR base when committing follow-up fixes

- Classification: misunderstanding user intent.
- Failure mode: treating a request to commit follow-up changes as permission to squash the existing feature commit that already serves as a pull request base.
- Detection signal: the resulting branch no longer retained `c064548` as its first commit.
- Prevention rule: when a pull request already exists, preserve its base commit and create a new commit for the working-tree delta unless history rewriting is explicitly requested.
- Tripwire: compare `HEAD`, the PR branch tip, and the intended base before any reset; after committing, assert the original commit remains an ancestor of the new `HEAD`.
