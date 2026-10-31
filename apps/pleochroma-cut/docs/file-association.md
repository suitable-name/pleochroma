# Opening `.psdd` Files from the System

This note is for whoever packages Pleochroma Cut. The program already opens a
design given on its command line; registering the `.psdd` file type with
the operating system is what makes a double-click do that. Nothing here is
installed by the program itself.

## What the Program Does

```
pleochroma-cut <path>
```

opens `<path>` in the editor once the window is up. The first argument that is not
a flag (anything starting with `-`, such as `--log`) is the file. A `.psdd`
design file, a `.asc`, a `.gem` and a `.gcs` all work;
a file is recognised by its content, not only its name. A
path with spaces must reach the program as one argument, which is what both
registrations below do.

A file given this way opens instead of the "Recover Unsaved Work?" /
"Reopen Last Design?" offer. There is no single-instance forwarding: every launch
starts its own window, so a second double-click opens a second window rather than
handing the file to one that is already running.

The file type's identifiers:

| | |
|---|---|
| Extension | `.psdd` |
| Media type | `application/vnd.pleochroma.design+toml` (TOML text) |
| Description | Pleochroma design file |

## Windows

Register per user under `HKEY_CURRENT_USER\Software\Classes` (no administrator
rights needed; an installer running per machine can write the same keys under
`HKEY_LOCAL_MACHINE\Software\Classes`). Adjust the path to `pleochroma-cut.exe`.

```reg
Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Classes\.psdd]
@="Pleochroma.Design"
"Content Type"="application/vnd.pleochroma.design+toml"

[HKEY_CURRENT_USER\Software\Classes\Pleochroma.Design]
@="Pleochroma design file"

[HKEY_CURRENT_USER\Software\Classes\Pleochroma.Design\DefaultIcon]
@="C:\\Program Files\\Pleochroma\\pleochroma-cut.exe,0"

[HKEY_CURRENT_USER\Software\Classes\Pleochroma.Design\shell\open\command]
@="\"C:\\Program Files\\Pleochroma\\pleochroma-cut.exe\" \"%1\""
```

The `"%1"` in the open command is what passes the double-clicked file as the
argument, quoted so a path with spaces stays one argument. After writing the keys,
Explorer picks the association up on its next refresh (or run
`assoc .psdd` to check it; signing out and in always works).

To remove the association, delete the `.psdd` and `Pleochroma.Design` keys.

## Linux

Two files: a shared-mime-info type, so the system knows what a `.psdd` file
is, and a desktop entry that offers the program as its opener.

`pleochroma-design.xml`, installed to `~/.local/share/mime/packages/` (or
`/usr/share/mime/packages/` for a package):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/vnd.pleochroma.design+toml">
    <comment>Pleochroma design file</comment>
    <sub-class-of type="application/toml"/>
    <glob pattern="*.psdd"/>
    <magic priority="60">
      <match type="string" offset="0" value="format = &quot;pleochroma-design&quot;"/>
    </magic>
  </mime-type>
</mime-info>
```

`pleochroma-cut.desktop`, installed to `~/.local/share/applications/` (or
`/usr/share/applications/`); set `Exec` to the installed binary:

```ini
[Desktop Entry]
Type=Application
Name=Pleochroma Cut
Comment=Design and cut faceted gemstones
Exec=pleochroma-cut %f
Icon=pleochroma-cut
Terminal=false
Categories=Graphics;Science;
MimeType=application/vnd.pleochroma.design+toml;
```

`%f` passes the selected file as one argument. Then refresh the caches and make the
program the default opener:

```sh
update-mime-database ~/.local/share/mime
update-desktop-database ~/.local/share/applications
xdg-mime default pleochroma-cut.desktop application/vnd.pleochroma.design+toml
```

The `<magic>` rule matches the first line a design file starts with
(`format = "pleochroma-design"`), so a design file is recognised even if it was
renamed. Drop that element if you only want the `*.psdd` name match.

## Check

1. Save a design (a `.psdd` file appears).
2. Double-click it (or run `pleochroma-cut path/to/it.psdd`).
3. The window opens with that design loaded and its file name in the title bar.
