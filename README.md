# Janus

A two-way dotfile manager with template rendering, secrets support, and a staged pipeline that makes it safe to manage config files across machines.

## What Makes Janus Different

Most dotfile managers work in one direction: you put files in a repo and symlink them out. Janus works both ways. When an application modifies its own config (and it will), those changes write through the symlink back to your staged copy. You can then `sync` those changes back into your source templates, hunk by hunk.

Janus also separates concerns into a three-stage pipeline, so you can inspect and control exactly what gets rendered, what gets staged, and what gets deployed:

```
                    generate              stage                deploy
  source files ──────────────► .generated/ ────────► .staged/ ────────► target paths
  (templates)                  (rendered)            (ready)            (symlinks)

                                                         ◄────────────
                                                     sync (two-way)
                                    import
  managed ◄──────────────────────────────────────────────────── existing configs
```

Each stage is independently runnable. You can re-generate without re-deploying, diff between stages, or deploy only specific files.

## Quick Start

```sh
# Initialize a dotfiles directory
janus init

# Import an existing config file
janus import ~/.config/hypr/hypr.conf

# Or import an entire directory
janus import ~/.config/alacritty

# Run the full pipeline for all managed files
janus apply --all

# Check what's in sync
janus status --all
```

## Installation

```sh
cargo install --path .
```

Requires Rust 2024 edition. No external dependencies at runtime unless you use secrets (which requires the [1Password CLI](https://developer.1password.com/docs/cli/)).

## How It Works

### Directory Layout

After running `janus init`, your dotfiles directory looks like this (using hypr and alacritty as examples):

```
~/dotfiles/                        # dotfiles_dir (configurable)
├── vars.toml                      # global template variables (default, but you can reference whatever files you want)
├── hypr/
│   └── hypr.conf                  # source file (Tera template)
├── alacritty/
│   └── config.toml                # source file (plain copy)
├── .generated/                    # rendered templates / copied files
│   ├── hypr/hypr.conf
│   └── alacritty/config.toml
├── .staged/                       # files ready for deployment
│   ├── hypr/hypr.conf
│   └── alacritty/config.toml
└── .janus_state.toml              # internal state tracking
```

| Directory | Commit | Purpose |
|-----------|--------|---------|
| Root (`~/dotfiles/`) | ✅ |Your source files. Templates use [Tera](https://keats.github.io/tera/) syntax. This is what you commit to git. |
| `.generated/` | ❌ |Output of template rendering. Plain files are copied as-is. You generally don't commit this, _especially if you use secrets_. |
| `.staged/` | ❌ | Copies of generated files, ready to be symlinked. When an app modifies its config, the change lands here (via the symlink). You generally don't commit this, _especially if you use secrets_. |
| `.janus_state.toml` | ✅ |Tracks which files are deployed and which import paths were ignored. |

### The Pipeline

**Generate** reads each source file. If `template = true` (the default), it renders the file through [Tera](https://keats.github.io/tera/) with your variables and secrets. Otherwise, it copies the file as-is. Output goes to `.generated/`.

**Stage** copies files from `.generated/` to `.staged/`. This separation lets you diff between what was generated and what's currently deployed (since apps may have modified the staged copy through the symlink).

**Deploy** creates symlinks from `.staged/` files to their target paths (e.g., `~/.config/hypr/hypr.conf` -> `~/dotfiles/.staged/hypr/hypr.conf`). By default, deploys are atomic (temp symlink + rename) to avoid windows where the file doesn't exist.

**Apply** runs all three in sequence: generate, stage, deploy.

### Two-Way Sync

When a deployed application modifies its config, the change writes through the symlink directly into `.staged/`. Run `janus status --all` to see which files have drifted, then `janus sync` to interactively merge those changes back into your source templates hunk by hunk.

## Configuration

The config file lives at `~/.config/janus/config.toml` (or wherever `$XDG_CONFIG_HOME` points). Override with `--config`.

### Minimal Config

```toml
dotfiles_dir = "~/dotfiles"
vars = ["vars.toml"]

[[files]]
src = "hypr/hypr.conf"
```

### Full Config Reference

```toml
dotfiles_dir = "~/dotfiles"

# Global template variable files (relative to dotfiles_dir).
# Later files override earlier ones.
vars = ["vars.toml", "machine-vars.toml"]

# Global secret config files (relative to dotfiles_dir).
# Secrets are resolved at generate-time from external engines.
secrets = ["secrets.toml"]

# --- File entries ---

[[files]]
src = "hypr/hypr.conf"                        # relative path in dotfiles_dir (required)
target = "~/.config/hypr/hypr.conf"            # deploy target (default: ~/.config/{src})
template = true                                # render as Tera template (default: true)
vars = ["hypr-vars.toml"]                      # per-file var overrides
secrets = ["hypr-secrets.toml"]                # per-file secret overrides

[[files]]
src = "bashrc"
target = "~/.bashrc"
template = false                               # deploy as plain copy, no rendering

# --- Filesets ---

[filesets.desktop]
patterns = ["hypr/*", "waybar/*", "mako/*"]    # glob patterns matching src paths
vars = ["desktop-vars.toml"]                   # vars applied to matching files
secrets = ["desktop-secrets.toml"]             # secrets applied to matching files

[filesets.shell]
patterns = ["bashrc", "zshrc", "starship.toml"]
```

### `[[files]]` Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `src` | string | *required* | Relative path within `dotfiles_dir` |
| `target` | string | `~/.config/{src}` | Deployment target path (supports `~`) |
| `template` | bool | `true` | Whether to render as a Tera template |
| `vars` | list of strings | `[]` | Per-file variable files (override globals) |
| `secrets` | list of strings | `[]` | Per-file secret files (override globals) |

### `[filesets.<name>]` Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `patterns` | list of strings | *required* | Glob patterns that match `src` paths |
| `vars` | list of strings | `[]` | Variable files applied to matching files |
| `secrets` | list of strings | `[]` | Secret files applied to matching files |

Filesets let you operate on groups of files: `janus apply --filesets desktop,shell`. They also support fileset-level variable and secret overrides that are automatically inherited by matching files during generation.

## Template Variables

Variable files are plain TOML. Values are available in templates via `{{ name }}`:

```toml
# vars.toml
terminal_font = "JetBrains Mono"
terminal_font_size = 14
colorscheme = "catppuccin"
```

```
# alacritty/config.toml (source template)
[font]
normal.family = "{{ terminal_font }}"
size = {{ terminal_font_size }}
```

### Merge Order

Variables merge in this order, with later values winning:

1. **Global** `vars` (from top-level config)
2. **Fileset** `vars` (from each matching fileset)
3. **Per-file** `vars` (from the `[[files]]` entry)

## Secrets

Secrets work like template variables but are resolved at generate-time from external secret managers. They are never stored in your dotfiles -- only the reference is kept in config. However, the _are_ stored in `.generated/`, `.staged/`, and deployed files. 

### Secret Config File Format

```toml
# secrets.toml
[[secret]]
name = "db_password"
engine = "1password"
reference = "op://Private/database/password"

[[secret]]
name = "api_key"
engine = "1password"
reference = "op://Work/api-service/credential"
```

Each entry has:

| Field | Description |
|-------|-------------|
| `name` | Template variable name (used as `{{ name }}` in templates) |
| `engine` | Secret backend -- currently `1password` |
| `reference` | Engine-specific locator (e.g., `op://Vault/Item/Field`) |

Use them in templates exactly like variables:

```
# config template
api_key = "{{ api_key }}"
db_password = "{{ db_password }}"
```

### How Resolution Works

- Secret config files are parsed immediately (cheap TOML reads)
- Actual secret lookups (`op read`) are **deferred** until a file that references that secret config is generated. We can't actually tell if the file will use a particular secret, so we have to read all secrets in a secret file when needed.
- Results are **cached** per generate run -- each unique reference is resolved at most once, even if multiple files use the same secret
- If a secret name collides with a variable name, generation **bails with an error** listing all conflicts

### Merge Order

Secrets follow the same merge order as variables:

1. **Global** `secrets`
2. **Fileset** `secrets`
3. **Per-file** `secrets`

### Supported Engines

| Engine | Requires | Reference format |
|--------|----------|-----------------|
| `1password` | [1Password CLI](https://developer.1password.com/docs/cli/) (`op`) | `op://Vault/Item/Field` |

## Commands

### Pipeline Commands

| Command | Description |
|---------|-------------|
| `janus generate <files\|--all\|--filesets>` | Render templates into `.generated/` |
| `janus stage <files\|--all\|--filesets>` | Copy `.generated/` to `.staged/` |
| `janus deploy <files\|--all\|--filesets> [--force]` | Symlink `.staged/` files to target paths |
| `janus apply <files\|--all\|--filesets> [--force]` | Run generate + stage + deploy in one shot |

### Reverse Commands

| Command | Description |
|---------|-------------|
| `janus import <path> [--all] [--max-depth N]` | Import existing config files into management |
| `janus undeploy <files\|--all\|--filesets> [--remove-file]` | Remove deployed symlinks (leaves a copy by default) |
| `janus unimport <files\|--filesets> [--remove-file]` | Fully reverse an import (no `--all` -- too destructive) |

### Inspection Commands

| Command | Description |
|---------|-------------|
| `janus status <files\|--all\|--filesets> [--only-diffs] [--deployed] [--undeployed]` | Show pipeline status for each file |
| `janus diff <files\|--all\|--filesets>` | Show diff between `.generated/` and `.staged/` |
| `janus sync <files\|--all\|--filesets>` | Interactively merge staged changes back into source templates |

### Housekeeping

| Command | Description |
|---------|-------------|
| `janus init [--dotfiles-dir PATH]` | Create dotfiles directory, config, and state file |
| `janus clean [--generated] [--orphans]` | Delete generated files or remove orphaned files from generated/staging |

### Global Flags

| Flag | Description |
|------|-------------|
| `--config <path>` | Override config file location |
| `--dry-run` | Preview actions without making changes |
| `-v` / `-vv` | Increase verbosity (DEBUG / TRACE) |
| `-q` / `-qq` / `-qqq` | Decrease verbosity (WARN / ERROR / OFF) |

## Importing Existing Configs

`janus import` brings existing config files under management. It copies each file into your dotfiles directory, adds a `[[files]]` entry to your config, and runs the full forward pipeline.

```sh
# Import a single file
janus import ~/.config/hypr/hypr.conf

# Import a directory (walks recursively)
janus import ~/.config/alacritty

# Skip interactive prompts
janus import ~/.config/waybar --all
```

The destination path inside your dotfiles directory is determined automatically:

| Source location | Destination | Example |
|----------------|-------------|---------|
| Under `~/.config/` | Strip prefix | `~/.config/hypr/hypr.conf` -> `hypr/hypr.conf` |
| Under `~/` | Strip home + leading dot | `~/.bashrc` -> `bashrc` |
| Elsewhere | Flatten with underscores | `/etc/systemd/system/foo.service` -> `etc_systemd_system/foo.service` |

## Safety

Janus is designed to be safe by default:

- **`undeploy` leaves files behind.** When you undeploy, the symlink is replaced with a regular copy of the file so your config doesn't disappear. Use `--remove-file` to actually delete it.
- **`unimport` has no `--all`.** Unimporting removes source files and config entries. Requiring explicit file selection prevents accidents.
- **Atomic deploys.** By default, symlinks are created atomically (temp symlink + rename) so there's never a moment where the target file doesn't exist.
- **Explicit file selection.** Every command that operates on files requires either explicit file arguments, `--all`, or `--filesets`. Nothing defaults to "all".
- **Dry run everything.** Every mutating command supports `--dry-run`.
- **State saved per file.** Deploy and import save state after each file, not in a batch. If something fails halfway, the state file accurately reflects what actually happened.

## Building

```sh
cargo build                        # default (includes atomic-deploy feature)
cargo build --no-default-features  # without atomic deploy
cargo test                         # run tests
cargo check                        # type-check without building
```
