local M=dofile(T.path("mods/HSMPCombat/Scripts/stuck_membership.lua"))
local function obj(address,name,world,fields)
    local o=fields or {};o.IsValid=function()return true end
    o.GetAddress=function()return address end;o.GetFullName=function()return name end
    if world then o.GetWorld=function()return world end end
    return o
end
local function fixture()
    local world=obj(1,"World arena")
    local weapon=obj(10,"Weapon own",world)
    local victim=obj(20,"Willie target",world)
    -- Component GetWorld is deliberately absent: pinned GetWorld is AActor-only.
    local collider=obj(11,"Weapon own.Box",nil,{GetOwner=function()return weapon end})
    local body=obj(21,"Willie target.Mesh",nil,{GetOwner=function()return victim end})
    local context={world="arena#1",drops=0,native_world=M.reference(world),match_id=7,round=2,
        attacker_peer=1,victim_peer=2,attacker_life=3,victim_life=4,victim_name="Target",
        source_address=11,target_address=21,source=3145728,ordinal=1,source_class="ArmingSword_C",bone="head"}
    local q={context=context,world=context.native_world,weapon=weapon,collider=collider,victim=victim,body=body}
    local x={q=q,context=context,world=world,weapon=weapon,victim=victim,collider=collider,body=body,
        list={},parents={},scan=0,entries=0,scope=true,evidence_reads=0}
    function x.parent(cid)
        return {world="arena#1",drops=0,attacker_peer=1,peer=2,nm="Target",match_id=7,round=2,
            attacker_life=3,victim_life=4,source_address=11,target_address=21,source=3145728,
            source_class="ArmingSword_C",bone="head",at=10,ats=10000,vts=9900,vats=9890,flags=160,gate=false,cid=cid}
    end
    function x.constraint(id,p)
        local c=obj(id,"Constraint "..id,world,{["My Weapon"]=weapon,["Weapon Hit Module"]=collider,
            ["Hit Actor"]=victim,["Component 2 (Body)"]=body,["Bone Name 2"]={ToString=function()return "head"end},
            GetOwner=function()error("constraint actor Owner must not be read")end})
        if p then x.parents[M.key(M.reference(c),context)]=M.bind({p},M.reference(c),context,10.01)end
        return c
    end
    local array={GetArrayNum=function()return x.count or #x.list end,GetArrayAddress=function()return 100 end,
        GetArrayDataAddress=function()return 200 end,
        ForEach=function(_,fn)
            x.scan=x.scan+1
            for i,c in ipairs(x.list)do
                x.entries=x.entries+1
                local result=fn(i,{get=function()return c end})
                if result==true then x.stopped=true;break end -- pinned bool-true stop
            end
        end}
    weapon["Stuck Constraints Array"]=array;x.array=array
    x.env={guard=function()return x.scope end,unwrap=function(v)return v:get()end,
        parent=function(id,c)return x.parents[M.key(id,c)]end,
        evidence=function()x.evidence_reads=x.evidence_reads+1;return "density=observed"end}
    function x.run()return M.resolve(q,x.env)end
    return x
end

do
    local x=fixture();local p=x.parent(31);x.list={x.constraint(30,p)}
    local r=x.run()
    T.check(r.complete and r.state=="unique" and r.parent==p and r.constraint.address==30 and x.scan==1,
        "single current membership preserves exact complete parent in one scan")
    T.check(x.evidence_reads==1 and r.evidence=="density=observed" and M.current(x.q,r,x.env.guard),
        "unique constraint details and fresh scalar input identities remain available")
    T.check(r.weapon==nil and r.collider==nil and r.body==nil and r.constraint.GetAddress==nil,
        "resolution contains scalar identities and the existing plain parent only")
    x.list={};r=x.run()
    T.check(r.complete and r.state=="none" and not r.parent,"complete empty array has no membership")
end
do
    local x=fixture();local p=x.parent(31)
    local a,b=x.constraint(30,p),x.constraint(32,p);x.list={a,b}
    local r=x.run()
    T.check(r.state=="unique" and r.complete and r.matches==2 and r.parent==p and not r.constraint and x.evidence_reads==0,
        "multiple memberships with identical full parent keep one parent and no emitting constraint authority")
    x.list={a,a};r=x.run()
    T.check(r.state=="unique" and r.matches==1,"repeated exact native identity is deduplicated")
    local other=x.parent(33);b=x.constraint(32,other)
    for _,list in ipairs({{a,b},{b,a}})do
        x.list=list;r=x.run()
        T.check(r.complete and r.state=="ambiguous" and not r.parent and not r.constraint,
            "distinct complete parents are ambiguous regardless of native array order")
    end
    x.list={a,x.constraint(34)};r=x.run()
    T.check(r.complete and r.state=="unavailable" and not r.parent,"one bound membership and one orphan cannot choose a parent")
    local altered=x.parent(31);altered.ats=altered.ats+1
    x.list={a,x.constraint(32,altered)};r=x.run()
    T.check(r.state=="ambiguous","same cid with a different complete origin timestamp cannot collapse into one parent")
    local incomplete=x.parent(31);incomplete.vats=nil
    x.list={x.constraint(30,incomplete)};r=x.run()
    T.check(r.complete and r.state=="unavailable","missing required parent timestamp cannot be defaulted into a usable claim")
end
do
    local x=fixture();local p=x.parent();local other=x.parent()
    local a,b=x.constraint(30,p),x.constraint(32,other);x.list={a,b}
    local r=x.run()
    T.check(r.state=="ambiguous","distinct pending origins with nil cid do not collapse into one parent")
    b=x.constraint(32,p);x.list={a,b};r=x.run()
    T.check(r.state=="unique" and r.parent==p and not r.header.cid,"same pending origin can await its original cid")
    p.cid=41
    T.check(M.parent_current(r.header,p,x.context),"successful origin flush may assign cid to the same pending record")
    p.ats=p.ats+1
    T.check(not M.parent_current(r.header,p,x.context),"a changed queued parent header cannot be reused after flush")
    local binding=M.bind({other,x.parent()},M.reference(a),x.context,10.01)
    T.check(binding.state=="ambiguous" and not binding.parent,"BeginPlay candidate binding cannot pick the last recent origin")
end
do
    local x=fixture();local p=x.parent(31);x.list={x.constraint(30,p)}
    x.count=129;local r=x.run()
    T.check(r.state=="overflow" and x.scan==0 and x.entries==0,"native count above cap refuses iteration before any entries")
    x.count=1.5;r=x.run()
    T.check(not r.complete and x.scan==0,"nonintegral native count cannot authorize a scan")
    x.array.GetArrayNum=function()error("unavailable")end;r=x.run()
    T.check(not r.complete and x.scan==0,"unreadable native count cannot authorize a scan")
    x=fixture();local dead={IsValid=function()return false end};for i=1,128 do x.list[i]=dead end
    r=x.run();T.check(r.complete and r.state=="none" and x.entries==128,"cap-sized array exhausts positively invalid entries")
    x=fixture();x.count=1;x.list={x.constraint(30,x.parent(31)),x.constraint(32,x.parent(31))};r=x.run()
    T.check(r.state=="overflow" and x.stopped and x.entries==2 and x.evidence_reads==0,
        "iterator exceeding announced count returns boolean true and discards its readable prefix")
    x=fixture();x.list={x.constraint(30,x.parent(31))};x.array.GetArrayDataAddress=function()return 0 end;r=x.run()
    T.check(not r.complete and x.scan==0,"nonempty array with null native storage never enters ForEach")
end
do
    local x=fixture();local c=x.constraint(30,x.parent(31));x.list={c}
    c["Bone Name 2"]={ToString=function()return "spine_03"end};local r=x.run()
    T.check(r.complete and r.state=="none","readable different native bone is a proven nonmatch")
    c["Bone Name 2"]=nil;r=x.run()
    T.check(not r.complete and x.stopped,"missing native bone is incomplete and terminates the scan")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    c["My Weapon"]=obj(50,"Other weapon",x.world);r=x.run()
    T.check(r.complete and r.state=="none","readable different native weapon excludes the entry")
    c["My Weapon"]=nil;r=x.run()
    T.check(not r.complete,"missing membership identity never becomes a fabricated nonmatch")
end
do
    local x=fixture();local c=x.constraint(30,x.parent(31));x.list={c}
    x.array.ForEach=function(_,fn)x.scan=x.scan+1;fn(0,{get=function()return c end})end
    local r=x.run();T.check(not r.complete,"zero-based mock or inconsistent native iterator is rejected")
    x=fixture();x.count=1;r=x.run();T.check(not r.complete,"short iteration cannot prove complete membership")
    x=fixture();x.list={x.constraint(30,x.parent(31))}
    local reads=0;x.array.GetArrayDataAddress=function()reads=reads+1;return reads==1 and 200 or 201 end
    r=x.run();T.check(not r.complete and not r.parent,"native storage identity change invalidates the complete-looking scan")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    local calls=0;c.GetAddress=function()calls=calls+1;return calls==1 and 30 or 35 end
    r=x.run();T.check(not r.complete and not r.parent,"constraint identity changed during native field reads cannot inherit a binding")
end
do
    local x=fixture();local c=x.constraint(30,x.parent(31));x.list={c}
    c.GetWorld=function()return obj(99,"World other")end
    local r=x.run();T.check(not r.complete,"a constraint in another world cannot prove membership")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    x.body.GetOwner=function()return obj(80,"Actor other",obj(99,"World other"))end
    r=x.run();T.check(not r.complete and x.scan==0,"component owner world mismatch refuses the scan")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    x.body.GetOwner=function()return obj(80,"Other victim",x.world)end
    r=x.run();T.check(not r.complete and x.scan==0,"same-world body owned by another actor cannot prove victim membership")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    local binding=x.parents[M.key(M.reference(c),x.context)];binding.context={}
    r=x.run();T.check(r.complete and r.state=="unavailable","missing stored world/life context cannot supply a parent")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c,x.constraint(32,x.parent(31))}
    local touched_after=0
    x.env.unwrap=function(v)x.scope=false;return v:get()end
    c.IsValid=function()if not x.scope then touched_after=touched_after+1 end;return true end
    r=x.run();T.check(not r.complete and x.stopped and x.entries==1 and touched_after==0,
        "scope change returns true before any subsequent old-world native getter or entry")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c};r=x.run()
    x.collider.GetAddress=function()return 55 end
    T.check(not M.current(x.q,r,x.env.guard),"source component identity change before forwarding fails the fresh guard")
end
do
    local x=fixture();x.context.bone="lowerarm_l"
    local p=x.parent(31);p.bone="lowerarm_l"
    local c=x.constraint(30,p);x.list={c}
    local bone="lowerarm_l";c["Bone Name 2"]={ToString=function()return bone end}
    local binding=x.parents[M.key(M.reference(c),x.context)]
    bone="hand_l";x.context.bone="hand_l"
    local r=x.run()
    T.check(r.complete and r.state=="unique" and r.parent==p and r.header.bone=="lowerarm_l"
        and r.parent_context.bone=="lowerarm_l" and r.context.bone=="hand_l",
        "proved same-constraint forearm-to-hand rebind keeps its immutable original parent")
    T.check(binding.context.bone=="lowerarm_l" and binding.header.bone=="lowerarm_l"
        and M.parent_current(r.header,p,r.parent_context),"rebind never rewrites the original binding/header into the current bone")
    x.context.bone="lowerarm_l"
    T.check(not M.current(x.q,r,x.env.guard),"current callback snapshot remains strict even for an otherwise proved binding alias")
    x.context.bone="hand_l";c.GetAddress=function()return 32 end
    x.parents[M.key(M.reference(c),x.context)]=binding;r=x.run()
    T.check(r.complete and r.state=="unavailable","new constraint identity cannot reuse the old forearm-to-hand binding")
    x=fixture();x.context.bone="hand_l";p=x.parent(31);p.bone="hand_l";c=x.constraint(30,p);x.list={c}
    x.context.bone="lowerarm_l";c["Bone Name 2"]={ToString=function()return "lowerarm_l"end};r=x.run()
    T.check(r.complete and r.state=="unavailable","reverse hand-to-forearm rebind is not an approved native alias")
    x=fixture();x.context.bone="lowerarm_r";p=x.parent(31);p.bone="lowerarm_r";c=x.constraint(30,p);x.list={c}
    x.context.bone="hand_r";c["Bone Name 2"]={ToString=function()return "hand_r"end};r=x.run()
    T.check(r.complete and r.state=="unavailable","unmeasured adjacent-bone rebind is not inferred from the left arm")
    x=fixture();c=x.constraint(30,x.parent(31));x.list={c}
    x.env.evidence=function()c["Bone Name 2"]={ToString=function()return "hand_l"end};return "observed"end
    r=x.run();T.check(not r.complete and not r.parent,"constraint bone changing during final evidence invalidates the captured membership")
end

local function original_pelvis_return(parent_bone)
    local x=fixture();x.context.bone="spine_02"
    local p=x.parent(39);p.bone=parent_bone or "pelvis"
    local c=x.constraint(30,p);x.list={c}
    local binding=x.parents[M.key(M.reference(c),x.context)]
    x.context.bone="pelvis";c["Bone Name 2"]={ToString=function()return x.context.bone end}
    return x,p,c,binding
end
do
    local x,p,c,binding=original_pelvis_return()
    local r=x.run()
    T.check(r.complete and r.state=="unique" and r.parent==p and r.constraint.address==30
        and r.header.bone=="pelvis" and r.parent_context.bone=="spine_02" and r.context.bone=="pelvis",
        "same native constraint can return from its factory spine anchor to the immutable original pelvis contact")
    T.check(binding.context.bone=="spine_02" and binding.header.bone=="pelvis" and p.bone=="pelvis"
        and M.parent_current(r.header,p,r.parent_context),
        "original-contact return preserves distinct binding anchor and parent/header contact without rewriting either")
    x,p,c,binding=original_pelvis_return("spine_02");r=x.run()
    T.check(r.complete and r.state=="unavailable" and not r.parent,
        "an original spine contact cannot borrow the pelvis-contact return rule")
    for _,field in ipairs({"bone","ats"})do
        x,p,c,binding=original_pelvis_return()
        p[field]=field=="bone" and "spine_02" or p[field]+1;r=x.run()
        T.check(r.complete and r.state=="unavailable" and not r.parent,
            "mutated original parent "..field.." refuses the return")
        x,p,c,binding=original_pelvis_return()
        binding.header[field]=field=="bone" and "spine_02" or binding.header[field]+1;r=x.run()
        T.check(r.complete and r.state=="unavailable" and not r.parent,
            "mutated captured header "..field.." refuses the return")
    end
    for _,identity in ipairs({"address","name"})do
        x,p,c,binding=original_pelvis_return()
        if identity=="address" then c.GetAddress=function()return 32 end
        else c.GetFullName=function()return "Replacement constraint"end end
        x.parents[M.key(M.reference(c),x.context)]=binding;r=x.run()
        T.check(r.complete and r.state=="unavailable" and not r.parent,
            "replacement constraint "..identity.." cannot reuse the original-contact binding")
    end
    for _,field in ipairs({"world","drops","attacker_peer","victim_peer","match_id","round",
        "attacker_life","victim_life","source","source_class","ordinal"})do
        x,p,c,binding=original_pelvis_return()
        x.context[field]=type(x.context[field])=="number" and x.context[field]+1 or x.context[field].." changed"
        -- Retained metadata found under another lookup key must still fail its full-scope proof.
        x.parents[M.key(M.reference(c),x.context)]=binding;r=x.run()
        T.check(r.complete and r.state=="unavailable" and not r.parent,
            "changed return scope "..field.." cannot inherit the original parent")
    end
    x,p,c,binding=original_pelvis_return()
    c["Weapon Hit Module"]=obj(12,"Weapon other.Box",nil,{GetOwner=function()return x.weapon end});r=x.run()
    T.check(r.complete and r.state=="none" and not r.parent,
        "different current weapon module cannot acquire the original pelvis contact")
    for _,bone in ipairs({"spine_01","spine_03","hand_l","hand_r"})do
        x,p,c,binding=original_pelvis_return();x.context.bone=bone;r=x.run()
        T.check(r.complete and r.state=="unavailable" and not r.parent,
            "original pelvis contact does not authorize a return to "..bone)
    end
end
