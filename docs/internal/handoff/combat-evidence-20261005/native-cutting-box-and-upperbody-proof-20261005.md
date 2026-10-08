# Native cutting-box ownership and severed upper-body lifecycle

Read-only cooked Blueprint evidence, 2026-10-05. No native execution or production changes in this investigation.

## Cutting Box is not an ordinary collision-array member

Primary source: `weapon-native-functions.txt`, `ExecuteUbergraph_ModularWeaponBP`.

- 3623 assigns the input hitting component. 4683–5501 may replace it with the nearest valid member of `Collision Components Array`, using `GetClosestPointOnCollision` and the original impact point.
- 5703–6192 enumerates `Collision Component.GetChildrenComponents(true)`. Children tagged `Tip` become the native Tips array.
- For children without `Tip`, 6066 branches to 12739: the same recursive child is cast to `BoxComponent`. A successful cast writes `self.Hit Box Collision` at 12926. Enumeration continues, so a later eligible Box replaces an earlier one.
- 14896 passes that exact `Hit Box Collision` to the victim's `Deal Complex Damage` alongside the selected `Collision Component`.
- Therefore requiring the cutting Box itself to occur in `Collision Components Array` rejects a legitimate native shape. The impacting collider and cutting child have separate identities.
- The extracted functions contain one assignment to `Hit Box Collision` and no explicit reset to None. A previous module's Box can therefore remain stored when the current module has no eligible Box. Do not silently replace the supplied original Box with the first child of the current collider. An exact same-weapon identity and the original captured Box frame remain necessary.
- 35952–36795 later temporarily changes Box extents for a separate vertex-paint call and restores them. That later extent change is after ordinary DCD and after stuck-constraint construction; it must not be substituted for the original DCD frame.

## Upper-body severing does not replace the Willie pawn

Primary source: `willie-native-all-functions.txt` (all cooked Willie functions).

`Dismember Function Delayed` creates skeletal **components on self**, not another Willie:

- 3379 `AddComponent("NODE_AddSkeletalMeshComponent-39", ..., self, false)`; 3566 assigns it to `Dismembered Limb`; 3939 adds it to `Dismember Child Parts`.
- The other component branch uses `AddComponent("NODE_AddSkeletalMeshComponent-31", ..., self, false)` at 5901, adds it to the same child-parts array at 5980, and copies original component tags plus `Dismembered` at 6294–6396.

In `ExecuteUbergraph_Willie_BP`:

- 132837 sets the same pawn's `Health = 0`.
- 133059 assigns `Dismember Child Parts[0]` to `Upper Body Mesh`; 133078 sets `Upper Body Spawned = true`.
- 133089–134464 builds supplementary physics constraints on that mesh and records them in `Dismembered Body Constraints`.
- 134538–134577 detaches the same pawn's `Camera Scene`, then attaches it to `Upper Body Mesh`, bone `spine_05`.
- 135262–135303 sets Timeline_5 speed and starts it. The speed depends on zombie/head/arm conditions and a native random factor.
- Timeline_5 updates severed upper-body tonus. Its finished wrapper explicitly dispatches to 638650, which jumps to 638547 (tonus zero) and calls native `Death()` at 638570. `Force Death` can take this terminal path earlier.
- 276464 onward separately creates upper-body held **weapon actors**, constrained to `Upper Body Mesh` hands, and stores them in `Sapwned R Hand Weapon Upper Body` / corresponding left property.

There is no `Possess` call in the extracted full Willie functions. The inspected severing path does not justify loosening original-pawn life context or rebinding a new pawn. Remaining implementation questions concern sampling and replaying the detached skeletal component, supplemental constraints, and alternate held-weapon actors. This is not proof that those states currently replicate correctly, nor evidence that severing caused the observed earlier grounded-contact bug.

## Ordinary cutting paint completion is not another GetDamage call

Primary sources: `willie-native-replay-functions.txt` and the full Willie function export.

- DCD calls GetDamage synchronously at19010. GetDamage directly writes main Health35513, ForceDeath45256 and part health (for example Neck71082, Head73512), and calls Break functions in the same function. Neither extracted DCD nor GetDamage contains a Delay.
- RVP paint settings do enable multithreaded work. The damage-path `AdditionalDataToPassThrough` values are defaults; they do not forward original HitByComponent as a subsequent gameplay damage source.
- The bound closest-vertex callback dispatches670897→655863 and handles foot blood/color state.
- The bound all-vertex-colors callback dispatches670937→666542. Its armor-swap branch saves vertex data and retires the old armor component; its other branch updates `Currently VP Painted Dismembered Mesh`. It does not call GetDamage or write Health.
- An exhaustive GetDamage call-site census in the extracted Willie functions finds DCD19010; self-hand cutting176016/177673; bite214951; and synchronous recursive parent-bone damage89391 inside GetDamage. No RVP callback is a GetDamage caller.

Therefore the specific hypothesis that ordinary asynchronous paint completion re-enters GetDamage with a remote source and is undone by C3.echo is not supported by this native path. This does not prove all asynchronous visual/dismembered-component replication correct. The separate Constraint_Weapon_Stuck delayed/direct GetDamage calls are real and require their own authenticated continuation.
