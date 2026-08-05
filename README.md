# HyprIcons
Desktop icons for Hyprland (Wayland)

## Running under Hyprland

HyprIcons is a normal program, not a Hyprland plugin, so Hyprland just needs
to launch it once per session. Add it to `~/.config/hypr/hyprland.conf` as
an autostart entry:

```
exec-once = hypricons
```

To start it with specific flags instead of (or in addition to) a saved
config file, pass them on the same line:

```
exec-once = hypricons --icon-size 64 --show-hidden
```

Reload Hyprland to pick up the change without logging out:

```
hyprctl reload
```

Only add one `exec-once = hypricons` line — running more than one instance
at a time will draw duplicate, overlapping icons on the same monitor(s).

## Flags

```
hypricons [OPTIONS]
```

With no options, HyprIcons starts using `~/.config/hypricons/config.json` if
it exists, falling back to built-in defaults for anything not set there.
Settings apply in this order, each overriding the last: built-in defaults,
then the config file, then any flags passed on the command line. Flags only
affect the current run — use `--save-config` to persist them.

Boolean flags take an optional value: bare `--flag` means `true`; use
`--flag false` (or `--flag=false`) to explicitly turn it off.

- `--config <PATH>` — Config file to load instead of the default. Only
  used if `<PATH>` exists; otherwise the default config file (or built-in
  defaults) is used. Does not change where `--save-config` writes.
- `--desktop-path <PATH>` — Folder whose contents are shown as icons.
  Default: `~/Desktop`.
- `--icon-size <PIXELS>` — Icon size in pixels. Default: `48`.
- `--show-hidden [true|false]` — Show files whose name starts with `.`.
  Default: `false`.
- `--single-click [true|false]` — Launch an item with a single click
  instead of a double click. Default: `false`.
- `--sort-by <name|date|type>` — Sort icons by file name, modification
  date, or file type. Default: `name`.
- `--columns <N>` — Stored in the config file but not currently read by
  the icon layout, which sizes its grid from the screen instead. Setting
  it has no visible effect yet.
- `--show-home [true|false]` — Show a Home icon that opens your home
  directory. Default: `true`.
- `--show-trash [true|false]` — Show a Trash icon that opens the trash.
  Default: `true`.
- `--debug [true|false]` — Log at debug level to stderr instead of info
  level. Default: `false`.
- `--save-config` — Write the settings resulting from this command line
  (defaults + config file + other flags) to
  `~/.config/hypricons/config.json`, then exit without starting the app.
  Always writes to the default config path, even if `--config` was also
  passed.
- `-h`, `--help` — Print the flag summary and exit.
- `-V`, `--version` — Print the version and exit.

## Examples

Preview a different folder without touching your saved config:

```
hypricons --desktop-path ~/Downloads
```

Persist larger icons and hidden files as the new defaults:

```
hypricons --icon-size 64 --show-hidden --save-config
```
