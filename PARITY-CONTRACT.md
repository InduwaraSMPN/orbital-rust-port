# Parity contract — `Orbital` C# → Rust port

- **End state:** A — replacement. Standalone Rust binary (`Orbital.exe`), the C# source
  (`orbital-main/`) stays untouched as the reference implementation only. No bindings, ever.
- **Unit of parity:** one action execution — `ActionProfile` + selected text + anchor rects in;
  clipboard / process launch / tooltip / sound / exit behavior out. Plus the pure seams:
  `calculate_position`, `CheckEditability` `(canSelect, canWrite, hasText)`, settings JSON
  files, `Loc::get`, WAV bytes, `SimulateKey` name table.
- **Acceptable differences:** log wording; wall-clock timing; icon pixels; MDL2→Lucide glyph
  mapping (flagged visual diff, `action_presets::lucide_name`); message text is 1:1 but the
  surrounding message-box implementation differs (WPF MessageBox → GPUI dialog).
- **Unacceptable differences:** the gating table above; numeric placement results; JSON
  wire compatibility (`ResultAction` string values, key names, defaults); error classes of
  `LlmError` (timeout / network / api-status / parse); exit behavior on hook failure; narrowed
  input acceptance (`SimulateKey` names, import validation errors, `ActionType::try_from_str`
  Trim + Popup fallback).
- **Measurement:** xunit vectors ported 1:1 into `tests/*.rs`
  (`action_gating.rs`, `placement.rs`, `fallback_editor.rs`, `settings.rs`, `executor.rs`), plus
  `proptest` generators over placement / gating. The five xunit files are inputs, not the
  contract — expected values come from the source's actual behavior (bugs included).

## Documented target-side decisions (not behavior changes)

1. **i18n engine.** Custom `i18n.rs` over serde_json catalogs instead of `fluent`/`rust-i18n`:
   the source uses raw `{0}` placeholders with `string.Format`; a custom loader reproduces that
   1:1 and avoids migrating ~1,000 strings to a different placeholder syntax. Locale files
   converted from `Strings.*.xaml` by `scripts/Convert-Strings.ps1` (UTF-8 verified by
   code-point escapes, entities decoded as WPF does at load).
2. **Clipboard.** `windows`-crate Win32 direct (OpenClipboard/EnumClipboardFormats/…) instead of
   `arboard`: the source backs up and restores the *entire* `IDataObject` (any format), which
   arboard cannot round-trip; full-format fidelity is observable behavior.
3. **TLS.** reqwest 0.13 `rustls-no-provider` + `rustls` with the `ring` provider installed at
   startup (avoids `aws-lc-rs`, which needs NASM on Windows). Same HTTP semantics: Bearer +
   HTTP-Referer + X-Title headers, 60 s timeout, 500-char error truncation.
4. **GPUI stack.** Upstream Zed snapshot `gpui-pre =0.3.8` (Windows: DirectX 11 + DirectWrite)
   + `gpui-kit 0.7.1` (component library, pins the same rev) + `gpui-tray 0.1` (`gpui-kit`
   feature; `Shell_NotifyIconW` tray). Fallback if the snapshot proves unbuildable here:
   `fc-gpui 0.11.2` (tray built in; drop `gpui-tray`). Decision recorded at scaffold time.
5. **UIA CacheRequest.** `uiautomation` (Rust) has no cache-batch API; the single-IPC
   optimization of the source is replaced by the same sequence of property/pattern queries.
   The parity unit stays the resulting `(canSelect, canWrite, hasText)` table, not the IPC count.
6. **TextPatternRange.Replace.** The source calls it via `dynamic` (not in the .NET interop
   surface). If the Rust UIA crate lacks it, the clipboard delete+paste path is used — same
   observable outcome (selection replaced with the result text).
7. **Velopack.** `release_flow.rs` re-implements the observable behavior: GitHub latest-release
   check on startup, balloon notification, restart-from-tray applies the update and restarts.
8. **Registry autostart.** windows crate `Win32_System_Registry` directly (no `windows-registry`
   dual-line landmine). Same key/value semantics, value = quoted exe path.

## Triage

Per the port-to-rust skill: source-wins diffs are fixed in the port; source-bug and
out-of-scope normalizations are never chosen silently — they are listed in
`PORT_STATUS.md` with a date for the contract owner to decide.
