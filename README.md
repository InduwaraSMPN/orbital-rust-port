# Orbital (Rust port)

A selection-triggered, AI-powered text-action launcher for Windows — a Rust + GPUI
port of the original C#/WPF application.

**This is a port of [CrowKing63/Orbital](https://github.com/CrowKing63/Orbital) —
original C#/WPF project by CrowKing63, MIT licensed.**

## What it does

Orbital watches your mouse and keyboard globally and pops up a floating action bar
above your cursor whenever text is selected. Triggers: mouse drag, double-click,
Shift+arrow / Ctrl+A selection, a custom hotkey, or long-press. Actions run local
commands (copy, cut, paste, search, key simulation) or any OpenAI-compatible LLM
endpoint (OpenAI, OpenRouter, Ollama, LM Studio, or a custom URL).

## Build

Requirements: Rust stable (edition 2024), Visual Studio Build Tools with the C++
workload, and the Windows SDK. Release users need none of these.

```powershell
cargo build --release
.\target\release\Orbital.exe
```

API keys are encrypted with Windows DPAPI and stored locally.

## Parity

Behavior is ported 1:1 from the C# original, including settings file compatibility.
See `PORT_PLAN.md`, `PARITY-CONTRACT.md`, and `PORT_STATUS.md` for the port contract,
deviations, and parity status.

## License

MIT — see [LICENSE](LICENSE), which retains the copyright notice of the original
project this port derives from.
