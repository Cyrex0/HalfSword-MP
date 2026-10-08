import collections, json, pathlib, re
P=pathlib.Path(__file__).resolve().parent
B=json.loads((P/'blueprints.json').read_text()); G=json.loads((P/'geometry-assets.json').read_text()); D=json.loads((P/'data-assets.json').read_text())
byclass={x['class']:x for x in B}; bypkg={x['package']:x for x in B}
extra={x['class'] for x in json.loads((P.parents[2]/'server/src/native_melee_classes.json').read_text())}
def refname(r):return r.get('ObjectName','').split("'")[-2].rsplit(':',1)[-1] if "'" in r.get('ObjectName','') else ''
def pkg(r):return r.get('ObjectPath','').rsplit('.',1)[0]
def walk(o,path='',statement=None):
    if isinstance(o,dict):
        statement=o.get('StatementIndex',statement)
        yield o,path,statement
        for k,v in o.items():yield from walk(v,path+'/'+k,statement)
    elif isinstance(o,list):
        for i,v in enumerate(o):yield from walk(v,path+'/'+str(i),statement)
def documents(path):
    s=path.read_text(encoding='utf-8-sig');i=0;decoder=json.JSONDecoder()
    while i<len(s):
        while i<len(s) and s[i].isspace():i+=1
        if i==len(s):return
        x,i=decoder.raw_decode(s,i);yield x
edges=[]; calls=[]
for b in B:
    for o in documents(P/b['source_file']):
        if o.get('Type')!='Function':continue
        for node,path,statement in walk(o.get('ScriptBytecode',[])):
            if node.get('Token')=='EX_ObjectConst' and isinstance(node.get('Value'),dict):
                r=node['Value'];c=refname(r)
                if c in byclass:edges.append({'from':b['class'],'to':c,'function':o['Name'],'statement':statement,'kind':'literal_class_choice','source':b['source_file']})
            if node.get('Token') in ('EX_FinalFunction','EX_CallMath','EX_LocalFinalFunction') and isinstance(node.get('Function'),dict):
                f=refname(node['Function'])
                if any(v in f.lower() for v in ('damage','module','spawn','physics','constraint','material','random')):
                    calls.append({'class':b['class'],'function':o['Name'],'statement':statement,'call':f,'source':b['source_file']})
    for node,path,_ in walk(b['cdo_explicit']):
        if 'ObjectName' in node and refname(node) in byclass:edges.append({'from':b['class'],'to':refname(node),'property':path,'kind':'class_default_reference','source':b['source_file']})
    parent=refname(b.get('parent') or {})
    if parent in byclass:edges.append({'from':b['class'],'to':parent,'kind':'inherits','source':b['source_file']})
adj=collections.defaultdict(set)
for e in edges:adj[e['from']].add(e['to'])
def reach(c):
    seen=set();todo=[c]
    while todo:
        c=todo.pop()
        if c in seen:continue
        seen.add(c);todo.extend(adj[c]-seen)
    return seen
def fnv(s):
    h=2166136261
    for ch in s.encode():h=((h^ch)*16777619)&0xffffffff
    return h
meshes={g['package']:g for g in G if g['type'] in ('StaticMesh','SkeletalMesh')}
physics=collections.defaultdict(list)
for g in G:
    if g['type']=='SkeletalBodySetup':
        p=g['data'].get('Properties',{});physics[g['package']].append({'body':g['export'],'bone':p.get('BoneName'),'physics_type':p.get('PhysicsType','native_default'),'shapes':p.get('AggGeom',{}),'body_defaults':p.get('DefaultInstance',{}),'collision_trace':p.get('CollisionTraceFlag','native_default'),'source':g['source_file']})
skeletal=[]
for b in B:
    for a in b['active_skeletal_assets']:
        if '/Assets/Weapons/' not in b['package']:continue
        m=meshes.get(pkg(a['mesh'])); mp=m['data'].get('Properties',{}) if m else {}
        pa=a.get('physics_asset') or mp.get('PhysicsAsset')
        skeletal.append({'class':b['class'],'component':a['component'],'skeletal_mesh':a['mesh'],'physics_asset':pa,'physics_bodies':physics.get(pkg(pa or {}),[]),'status':'unsupported_by_current_weapon_bounds; native articulated body sampling required','source':b['source_file']})
skclasses={x['class'] for x in skeletal}
coverage=[]
for b in B:
    if b['category']!='built_weapon':continue
    c=b['class']; ancestry=reach(c)
    if c in ('Weapon_Fists_C','Weapon_Feet_C'):role='body_striker';status='implemented_native_primitive_capture; limited_live_validation'
    elif '/Ranged/Quiver/' in b['package']:role='ammo_container';status='native_inventory_only; no_network_ammo_state'
    elif '/Ranged/Projectile/' in b['package']:role='projectile';status='missing_fired_source_authority_and_lifecycle'
    elif '/Ranged/Weapon/' in b['package']:role='ranged_weapon';status='missing_fire_reload_ammo_replication'
    elif 'BP_Weapon_Trap' in c:role='world_trap';status='missing_unheld_source_authority'
    else:role='held_melee';status='implemented_common_DCD_route; not_exhaustively_native_tested'
    modules=sorted(x for x in ancestry if x in byclass and byclass[x]['category']=='module')
    sk=sorted(ancestry&skclasses)
    coverage.append({'class':c,'package':b['package'],'native_parent':b['parent'],'role':role,'catalogue':b['configured_catalogue'],'extra_melee_registered':c in extra,'class_utf8_bytes':len(c.encode()),'class_hash_fnv1a32':fnv(c),'source_class_fits_Str48':len(c.encode())<48,'static_possible_module_classes':modules,'static_possible_skeletal_classes':sk,'coverage':status,'known_gaps':(['active_skeletal_module_geometry'] if sk else [])+(['embedded_constraint_Inside_continuation','sharp_vs_blunt_native_outcome_matrix','complete_armour_and_persistent_limb_outcomes'] if role in ('held_melee','projectile') else []),'source':b['source_file']})
hashes=collections.defaultdict(list)
for b in coverage:hashes[b['class_hash_fnv1a32']].append(b['class'])
collisions={str(h):c for h,c in hashes.items() if len(c)>1}
module_rows=[]
for b in B:
    if b['category']!='module':continue
    module_rows.append({'class':b['class'],'package':b['package'],'parent':b['parent'],'cdo_effective':b['cdo_effective'],'components':b['components_inherited'],'native_damage_tags':b['tagged_damage_features'],'possible_modules':sorted(reach(b['class']) & {x['class'] for x in B if x['category']=='module'}),'source':b['source_file']})
summary={'built_weapon_classes':len(coverage),'catalogue_registered_unique':sum(bool(x['catalogue']) for x in coverage),'extra_melee_unique':len(extra),'roles':dict(collections.Counter(x['role'] for x in coverage)),'max_source_class_bytes':max(x['class_utf8_bytes'] for x in coverage),'source_class_hash_collisions':collisions,'module_classes':len(module_rows),'literal_or_default_or_inheritance_edges':len(edges),'skeletal_configurations':len(skeletal),'weapon_skeletal_physics_assets':len({pkg(x['physics_asset']) for x in skeletal if x['physics_asset']}),'runtime_exhaustive_coverage':False,'static_options_semantics':'Reachability is an upper bound of literal construction choices and inheritance, not proof that every Cartesian combination is valid or that runtime random choice was observed.'}
for file,obj in [('coverage-manifest.json',coverage),('module-configurations.json',module_rows),('module-choice-edges.json',edges),('native-function-call-sites.json',calls),('skeletal-configurations.json',skeletal),('coverage-summary.json',summary)]:
    (P/file).write_text(json.dumps(obj,indent=2))
print(json.dumps(summary,indent=2))
