# Set up Oyo with an agent

Ask your coding assistant to install Oyo and configure it for your machine. Use **Copy setup prompt** in the [website's install section](https://oyo.victorare.mu/#install), then paste it into your assistant.

This guide is for an assistant setting up an end user's machine. It is not repository contributor guidance. Read the whole guide before making changes.

## Inspect the machine first

1. Identify the operating system, architecture, shell, and available package managers. Check whether `oy` is already on `PATH` with your shell's command lookup. If it exists, run `oy --version` and `oy --help` before deciding whether installation is needed.
2. Preserve an existing installation. Do not reinstall, upgrade, or switch package managers unless the user requests it or the installed version cannot support the requested setup. Explain any compatibility problem first.
3. Read the project's [installation instructions](https://github.com/ahkohd/oyo#install). Choose a documented method that supports this machine and uses a package manager already installed. The npm package is `@ahkohd/oyo`, the Cargo package is `oyo`, and the executable is `oy`. Do not guess a package name or run an unrelated package suggested by a tool. Do not install another package manager or use elevated privileges without the user's agreement.
4. Check `oy --version` and `oy --help` after installation. If no documented method supports the platform, explain the blocker instead of attempting an unverified installer.

## Find the active configuration

Read the current [config reference](https://oyo.victorare.mu/config/) and [theme reference](https://oyo.victorare.mu/theming/). Match the options to the installed version's `oy --help`.

Oyo loads the first existing `config.toml` in this order:

1. `$XDG_CONFIG_HOME/oyo/config.toml`, when `XDG_CONFIG_HOME` is set.
2. `~/.config/oyo/config.toml`.
3. The platform config directory, such as `~/Library/Application Support/oyo/config.toml` on macOS.

These files are alternatives, not layers. Inspect the existing candidates before creating a file that could shadow another one. If no config exists, use the first applicable location above. Resolve the user's real home directory and environment. Do not assume a Linux path on another platform.

Extra files passed with `--config <path>` overlay the user config in argument order. They do not isolate Oyo from the user config. Tables merge and scalar values in later files win. `review.hooks`, `review.actions`, and `selection.actions` append.

## Offer the optional starter preset

The [starter TOML](https://oyo.victorare.mu/starter.toml) uses Tokyo Night for the UI and syntax, a unified single-pane view, word highlights, and a 30-column file panel on the right. It keeps Tokyo Night's native backgrounds. It is an optional starting point, not a change to Oyo's defaults.

The supported view name is `unified`. An older preference written as `view_mode = "single"` is not a supported view name and falls back to the default. Use `unified` to make that intent explicit. Both theme names in this preset are `tokyonight`.

1. Download and inspect the starter TOML as data. Do not execute downloaded content.
2. For a new setup, offer the preset and write it to the selected config path if the user wants it. Create only the required Oyo config directory.
3. For an existing config, keep the user's intentional values. Show which preset keys are missing and which conflict. Add missing settings only after checking they suit the user's setup. Change an existing preference only when the user chooses the replacement.
4. Before editing, make a uniquely named backup beside the active file. Preserve unrelated keys, hooks, actions, keybindings, comments, and table order. Use a TOML-aware editor or targeted edits, not a whole-file overwrite or a blind append that duplicates tables. Preserve a managed symlink and edit its target only with the user's agreement. Stop if the config is invalid or cannot be edited safely.
5. Show the resulting config diff. Validate TOML and supported option values before launching Oyo. If a setting is unavailable in the installed version, report it and leave it out rather than silently discarding the user's configuration.

For a temporary preview, save the preset to a separate file and launch `oy --config <path-to-starter.toml>` with a suitable local diff. This overlays existing settings for that session. Do not save Settings while previewing unless the user wants those values written to the user config.

## Keep workflow integration optional

Ask whether the user wants Git or Jujutsu integration before editing either tool's configuration. Use only the tools they already use. Start from the project's [Git integration](https://github.com/ahkohd/oyo#use-oyo-with-git) and [Jujutsu integration](https://github.com/ahkohd/oyo#use-oyo-with-jujutsu) instructions. Prefer repository-local settings when suitable. Preserve aliases and existing diff tools. Do not change a global pager, default diff tool, shell profile, or agent instructions without the user's agreement.

Oyo can compare local files and review public remote changes without mandatory authentication. Do not ask the user to log in for basic setup or public read-only reviews. Authentication is needed for private data and some review sync operations. Do not publish comments or run `oy review push` as part of setup.

## Verify the result

1. Run `oy --version` and `oy --help`. Run `oy skill path` and read the printed file for the installed version's agent workflow.
2. Open an existing local diff with `oy`, or create two small temporary text files and run `oy <old-file> <new-file>`. Use temporary files rather than modifying the user's repository to manufacture a diff.
3. Check that Oyo starts without config warnings. If the starter was selected, check the unified view, Tokyo Night themes, word highlights, and right file panel. The file panel is most useful with a multi-file diff. Open Settings with `g s` to inspect the effective settings. Exit without saving if this is only a preview.
4. Report the installed version, installation method, active config path, settings changed or preserved, backup path, optional integrations, and checks actually run. Explain any remaining blocker. Do not claim an interactive check passed if no terminal was available.
