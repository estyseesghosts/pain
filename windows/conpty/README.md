# Bundled ConPTY host

The in-box Windows ConPTY (the `kernel32.dll` implementation) silently drops
unknown APC sequences. Kitty graphics are carried in APCs, so a Windows Pain
build that uses the in-box host cannot claim Kitty support even when
`libghostty-vt` parses the protocol correctly.

Pain therefore prefers an adjacent, redistributable OpenConsole pair at
runtime:

- `conpty.dll`
- `OpenConsole.exe`

Both files must stay side by side. `conpty.dll` launches the matching
`OpenConsole.exe` as the console host; mixing versions is unsupported.

## Provenance

| Field | Value |
| --- | --- |
| Package | `Microsoft.Windows.Console.ConPTY` |
| Version | `1.25.261109001` |
| Package URL | <https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY/1.25.261109001> |
| Flat-container | `https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.25.261109001/microsoft.windows.console.conpty.1.25.261109001.nupkg` |
| Source | <https://github.com/microsoft/terminal> |
| License | MIT (`MIT` license expression in the nuspec) |
| Architecture | x64 (`runtimes/win-x64/native/conpty.dll`, `build/native/runtimes/x64/OpenConsole.exe`) |

### Checksums (SHA-256)

```text
conpty.dll       e971856f89748dc3b458125e9afdd79b5e9574a7c937676116d3cbbd3b71f492
OpenConsole.exe  b17e48b07ef153b420f0a6e178bacd31fb4c3390e21ae746586213fc0719a775
```

File version for both binaries: `1.25.2611.09001`.

## Packaging

`build.rs` copies both files beside the built executable (and into `deps/` so
`cargo test` exercises the same host). Release packaging must keep them next
to `pain.exe`. If `conpty.dll` is absent or lacks the ConPTY exports, Pain
logs a warning and falls back to `kernel32.dll`; that fallback is explicitly
Kitty-graphics-degraded and must not be described as full graphics support.

Do not harvest a DLL from an installed Windows Terminal MSIX package: the
MSIX does not contain `conpty.dll`. Use the NuGet package above, which is the
supported redistributable.

## Updating

1. Download the new `.nupkg` from NuGet.
2. Replace both files with the x64 pair from the same package version.
3. Update the version, URL and checksums in this file.
4. Re-run the native Windows APC smoke gate (`cargo test --test pty
   conpty_apc`).

Keep `OpenConsole.exe` beside `conpty.dll`; Microsoft's host selection rejects
an `OpenConsole.exe` placed directly next to the DLL only in some providers,
so the pair layout above is the supported one for this application.
