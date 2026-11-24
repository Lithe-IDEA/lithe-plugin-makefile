# Makefile Build Plugin for Lithe (`io.github.lithe-idea.makefile-build`)

Provides native Makefile build system integration for Lithe (independent of any specific language plugin):

- **Automatic detection**: detects `GNUmakefile`, `Makefile`, and `makefile` in the workspace and nested folders.
- **GNU Make database + static fallback target discovery**: runs `make -p -R -r -q` (bounded timeout & output cap, isolated process group) to resolve `$(foreach)/$(eval)`, `$(wildcard ...)`, `ifeq`, `include`, `.PHONY`, `.DEFAULT_GOAL`, and line numbers while merging `##` / `#` descriptions; falls back cleanly to a purely static parser when `make` is unavailable.
- **Unified Run Configurations & Before Launch**: discovered Makefile targets automatically materialize as **Makefile Target** (`build.run-type`) configurations sharing the same group, SVG icon, editor form, and launch plan as user-created configurations, plus a **Run Makefile Target** (`beforeLaunchTasks`) step.
- **Registered Make toolchain & Settings UI**: registers `make` under **Build, Execution, Deployment → Toolchains → Build Tools** (`toolDefinitions: 1`) with cross-platform discovery, and renders an inline Make toolchain picker and lookup-order diagram on the Makefile settings page (`settings.view`).
- **Build options & diagnostics**: supports `-f`, `-j`, `-B`, `-n`, `-k` and Make/Go/GCC diagnostic line matching.

## Build & Package

Supported release targets: `darwin` (universal arm64 + x86_64), `windows-x64`, `windows-arm64`, `linux-x64`, `linux-arm64`.

```bash
cd backend && cargo test
node scripts/build-backend.mjs darwin
node scripts/package.mjs --platforms darwin
node scripts/verify-protocol.mjs
```

Install `dist/makefile-build-0.0.1-darwin.zip` in Lithe via **Settings → Plugins → Install Plugin from Disk…**.
