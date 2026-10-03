# The pak AES key, extraction and repacking

`pakchunk0-Windows.pak` has an encrypted index (`bEncryptedIndex=1`). Reading the cooked assets
(maps, Blueprints, data assets) needs its AES-256 key. The Half Sword developers share this key
with modders, and these notes publish it with how it was found.

## The key

```
bcbf7b45a4a8150d06f7b955bc25ef5ce603470f508302cad0eb48fea2d91517
```

It is bound to the zero-GUID (default) encryption key slot. `tools/mapdump` has it built in and
accepts another key in `HSMP_PAK_AES` (see [README.md](README.md)); repak takes it on the command
line.

## How it was found

The key is not stored as plain bytes in the shipping executable:

1. **Static scan of the executable.** Every high-entropy 32-byte window in `.data`, `.rdata`,
   `_RDATA` and `.rodata` (about 23 million candidates) was tried. There were four false
   positives and no match, so the key is obfuscated or derived at runtime.
2. **Heap scan of the running game.** With the game at the main menu (about 45 s after launch,
   after pak mounting), every `PAGE_READWRITE` region was read with `ReadProcessMemory` (about
   15 000 regions, 1.7 GB). Each 32-byte window, at an 8-byte stride, was tried as the key to
   AES-ECB-decrypt the head of the pak's encrypted index. The right key makes the first `int32`
   decode as a valid `FString` length followed by a readable mount path. Exactly one window passed,
   and the decrypted mount point was `../../../`, the UE default.

The same method works on other UE5 shipping games whose pak uses default-GUID encryption. A pak's
footer tells whether its index is encrypted at all.

## Extracting

[repak](https://github.com/trumank/repak) reads and writes UE paks. To extract a few folders:

```powershell
repak --aes-key bcbf7b45a4a8150d06f7b955bc25ef5ce603470f508302cad0eb48fea2d91517 `
    unpack "<game dir>\HalfswordUE5\Content\Paks\pakchunk0-Windows.pak" `
    --output extracted_pak `
    --include HalfswordUE5/Content/Maps/Menus/ `
    --include HalfswordUE5/Content/UI/
```

Useful entries:

| Path | What |
|---|---|
| `HalfswordUE5/Content/Maps/Menus/Map_Menu_SplashScreens.umap` | the main menu level |
| `HalfswordUE5/Content/Maps/Menus/Map_Menu_Startup.umap` | the startup splash level |
| `HalfswordUE5/Content/UI/UI_Startup_Menu.uasset` / `.uexp` | the main menu widget ([README.md](README.md), "The main menu") |
| `HalfswordUE5/Content/Maps/Arenas/Map_Arena_*.umap` | the arenas HSMP plays on |

Extracted assets are the game's content: keep them local and never commit them. Reading Blueprint
bytecode from them is described in [README.md](README.md) ("Bytecode"); `tools/mapdump` reads the
pak directly without extracting it.

## Repacking

```powershell
repak --aes-key <KEY> pack <extracted-dir> <output>.pak --version V11 --compression Oodle
```

Put the result in `<game dir>\HalfswordUE5\Content\Paks\`. UE mounts paks in name order and a later
pak overrides an earlier one, so a name such as `pakchunk99-Multiplayer_P.pak` wins over
`pakchunk0-Windows.pak`. Oodle compression needs `oo2core_9_win64.dll` from your own game install.

HSMP itself ships no pak: everything runs from UE4SS Lua mods and the native module. Pak patching
is an option for changes Lua cannot make; one worked example is
[pak-bp-patch-guide.md](pak-bp-patch-guide.md).
