# Patching a Blueprint in the pak (unused option)

HSMP does not patch any game asset. Its main-menu entry is built at runtime by HSMPMenu, which
adds its own ribbons next to the native ones and leaves every native button alone
([README.md](README.md), "The main menu"). This page records the alternative that was worked out
when the first menu integration tried to repurpose the Quit ribbon (`Button_3` of
`UI_Startup_Menu_C`): change the widget's Blueprint in a sideload pak instead of hooking it from
Lua. The extraction and repack steps were verified; the asset edit itself was never shipped.

Reasons to consider it for other changes:

- a UE4SS hook runs after a Blueprint function and cannot cancel it, while a patched asset can
  change what the function does;
- a label or layout change in the asset needs no runtime code;
- a pak patch also works without UE4SS.

Reasons HSMP does not use it: a patched asset must be redone for every game build that changes
it, it replaces the game's own file for everyone who installs it, and the runtime approach was
enough.

## Tools

- [repak](https://github.com/trumank/repak) to unpack and repack ([pak-unlocked.md](pak-unlocked.md)).
- [UAssetGUI](https://github.com/atenfyr/UAssetGUI) to edit a cooked UE 5.4 asset by hand
  (`winget install Atenfyr.UAssetGUI`). Scripted editing through
  [UAssetAPI](https://github.com/atenfyr/UAssetAPI) is possible but more fragile across engine
  versions.

## 1. Unpack the widget

```powershell
$KEY = "bcbf7b45a4a8150d06f7b955bc25ef5ce603470f508302cad0eb48fea2d91517"
repak --aes-key $KEY unpack "<game dir>\HalfswordUE5\Content\Paks\pakchunk0-Windows.pak" `
    --output extracted_mp_pak `
    --include "HalfswordUE5/Content/UI/UI_Startup_Menu.uasset" `
    --include "HalfswordUE5/Content/UI/UI_Startup_Menu.uexp"
```

## 2. Edit it in UAssetGUI

Open `extracted_mp_pak\HalfswordUE5\Content\UI\UI_Startup_Menu.uasset` and set the engine version
to **UE5.4**.

- Find the export
  `BndEvt__UI_Startup_Menu_Button_3_K2Node_ComponentBoundEvent_9_OnButtonClickedEvent__DelegateSignature`
  (the `_9_` index is the one in the current game build).
- In its script bytecode, find the call to `KismetSystemLibrary::QuitGame`.
- To make the click do nothing, point the call at a function with no visible effect, for example
  the button's own hover handler. Calling a function of your own instead needs a target UFunction
  that exists at load time (a C++ UE4SS mod or another asset), which is more work.
- To change the label, edit the `Text` property of the button's `TextBlock` export in the widget
  tree.
- Save.

## 3. Repack as a sideload pak

```powershell
repak --aes-key $KEY pack extracted_mp_pak "<game dir>\HalfswordUE5\Content\Paks\pakchunk99-Multiplayer_P.pak" `
    --version V11 --compression Oodle
```

UE mounts paks in name order and a later pak overrides an earlier one, so
`pakchunk99-Multiplayer_P.pak` wins over `pakchunk0-Windows.pak`. Delete the file to undo the
patch.

## 4. Check it

Launch the game, open the main menu and click the patched button. Disable any Lua mod that hooks
the same widget while you test, so you see the patch alone.
