-- The production replay_fists retirement fixture. The native machine's staged copy lives under
-- test-results/ (not in git); a checkout runs the committed copy beside the combat handoff.
local staged=T.path("test-results/dev-feature-checks/inventory-retirement-stage/fixture.lua")
local committed=T.path("docs/internal/handoff/combat-evidence-20261005/astra-fists-retirement-test.lua")
local fixture=T.exists(staged) and staged or committed
local ok,err=pcall(dofile,fixture)
T.check(ok,"replay_fists retirement fixture: every assertion holds ("..fixture:match("[^/]+$")..")",err)
