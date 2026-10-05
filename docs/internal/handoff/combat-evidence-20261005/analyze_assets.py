import collections, copy, hashlib, json, pathlib, re
ROOT=pathlib.Path(__file__).resolve().parent
decoder=json.JSONDecoder()
def documents(path):
    text=path.read_text(encoding='utf-8-sig'); i=0
    while i<len(text):
        while i<len(text) and text[i].isspace():i+=1
        if i==len(text):break
        obj,i=decoder.raw_decode(text,i);yield obj
def refs(obj,path=''):
    if isinstance(obj,dict):
        if 'ObjectPath' in obj:yield path,obj
        for k,v in obj.items():yield from refs(v,path+'/'+k)
    elif isinstance(obj,list):
        for i,v in enumerate(obj):yield from refs(v,path+'/'+str(i))
def pkg(ref):
    p=ref.get('ObjectPath',''); return p.rsplit('.',1)[0] if not p.startswith('/Script/') else p
def name(ref):return ref.get('ObjectName','').split("'")[-2].rsplit(':',1)[-1] if "'" in ref.get('ObjectName','') else ''
def merge(a,b):
    a=copy.deepcopy(a)
    for k,v in b.items():a[k]=merge(a[k],v) if isinstance(a.get(k),dict) and isinstance(v,dict) else copy.deepcopy(v)
    return a
manifests=json.loads((ROOT/'extraction.json').read_text())
if (ROOT/'supplemental-extraction.json').exists():manifests+=json.loads((ROOT/'supplemental-extraction.json').read_text())
packages={};objects={};errors=[];types=collections.Counter()
for row in manifests:
    if not row['success']:errors.append(row);continue
    try:
        exports=list(documents(ROOT/row['file']))
        path=row['package'].removesuffix('.uasset');packages[path]={'source':row,'exports':exports}
        for o in exports:objects[(path,o.get('Name',''))]=o;types[o.get('Type','')]+=1
    except Exception as ex:errors.append({'package':row['package'],'parse_error':str(ex)})
def resolve(ref):return objects.get((pkg(ref),name(ref)))
def effective(o,seen=None):
    seen=set() if seen is None else seen
    key=id(o)
    if key in seen:return {}
    seen.add(key)
    parent=resolve(o.get('Template',{}))
    return merge(effective(parent,seen) if parent else {},o.get('Properties',{}))
allpaths=set((ROOT/'all-pak-asset-packages.txt').read_text().splitlines())
missing=set();classes=[];assets=[];data=[];blueprints={}
cattext=(ROOT.parents[2]/'mods/HSMPLoadout/Scripts/hsmp_catalog.lua').read_text()
catalogue={}
for match in re.finditer(r'^W\("([^"]+)",\s*"([^"]+)",\s*\d+,\s*"([^"]+)"',cattext,re.M):
    ident,hand,path=match.groups();catalogue.setdefault(path.rsplit('/',1)[-1]+'_C',[]).append({'id':ident,'hand':hand})
for path,record in packages.items():
    exports=record['exports']; localrefs=[]
    for o in exports:
        for field,r in refs(o):
            target=pkg(r)
            if target and not target.startswith('/Script/') and target!=path:
                localrefs.append({'export':o.get('Name'),'field':field,'target':r})
                if target+'.uasset' in allpaths and target not in packages:
                    # Follow gameplay classes, native mesh/physics and config assets;
                    # leave material/texture/audio data outside combat semantics.
                    if re.match(r"(BlueprintGeneratedClass|StaticMesh|SkeletalMesh|PhysicsAsset|DataAsset|.*Equipment.*)'",r.get('ObjectName','')):missing.add(target+'.uasset')
    bp=next((o for o in exports if o.get('Type')=='BlueprintGeneratedClass'),None)
    if bp:
        cdo=next((o for o in exports if o.get('Name','').startswith('Default__')),None)
        components=[o for o in exports if o.get('Type','').endswith('Component')]
        category='built_weapon' if '/Built_Weapons/' in path else 'module' if '/Assets/Weapons/' in path and (('/Modules/' in path) or any(x in path.rsplit('/',1)[-1] for x in ['Weapon_Part','Weapon_Module'])) else 'weapon_support'
        row={'package':path,'class':bp['Name'],'category':category,'configured_catalogue':catalogue.get(bp['Name'],[]),'parent':bp.get('Super',bp.get('SuperStruct')),'source_file':record['source']['file'],
             'cdo_explicit':cdo.get('Properties',{}) if cdo else {},'cdo_effective':effective(cdo) if cdo else {},
             'components':[{'name':o['Name'],'type':o['Type'],'template':o.get('Template'),'explicit':o.get('Properties',{}),'effective':effective(o)} for o in components],
             'functions':[o['Name'] for o in exports if o.get('Type')=='Function'],
             'references':localrefs,'runtime_coverage':'not_exhaustively_tested'}
        classes.append(row);blueprints[path]=row
    for o in exports:
        if o.get('Type') in ('PhysicsAsset','SkeletalBodySetup','BodySetup','StaticMesh','SkeletalMesh','PhysicsConstraintTemplate'):
            assets.append({'package':path,'export':o['Name'],'type':o['Type'],'source_file':record['source']['file'],'data':o})
        elif 'DataAsset' in o.get('Class','') or o.get('Type','').startswith('DA_'):
            data.append({'package':path,'export':o['Name'],'type':o['Type'],'source_file':record['source']['file'],'properties':o.get('Properties',{})})
# Classify modules by authoritative inheritance, including the two generic
# Grip/SubModule bases outside the asset-family Modules directories.
def derives_part(row,seen=None):
    seen=set() if seen is None else seen
    if row['class']=='Modular_Weapon_Part_Master_C':return True
    if row['package'] in seen:return False
    seen.add(row['package']);parent=blueprints.get(pkg(row.get('parent') or {}))
    return derives_part(parent,seen) if parent else False
for row in classes:
    if derives_part(row):row['category']='module'
# Complete inherited component list; explicit same-name templates replace parents.
def component_tree(row,seen=None):
    seen=set() if seen is None else seen
    if row['package'] in seen:return {}
    seen.add(row['package']);parent=blueprints.get(pkg(row.get('parent') or {}))
    out=component_tree(parent,seen) if parent else {}
    for c in row['components']:out[c['name']]=c
    return out
for row in classes:
    row['components_inherited']=list(component_tree(row).values())
    row['active_skeletal_assets']=[{'component':c['name'],'mesh':c['effective'].get('SkeletalMeshAsset') or c['effective'].get('SkeletalMesh'),'physics_asset':c['effective'].get('PhysicsAssetOverride')} for c in row['components_inherited'] if c['type']=='SkeletalMeshComponent' and (c['effective'].get('SkeletalMeshAsset') or c['effective'].get('SkeletalMesh'))]
    row['tagged_damage_features']=[{'component':c['name'],'tags':c['effective']['ComponentTags']} for c in row['components_inherited'] if c['effective'].get('ComponentTags')]
for file,content in [('blueprints.json',classes),('geometry-assets.json',assets),('data-assets.json',data),('parse-errors.json',errors)]:
    (ROOT/file).write_text(json.dumps(content,indent=2))
(ROOT/'missing-reference-packages.txt').write_text('\n'.join(sorted(missing)))
summary={'pak_asset_count':len(allpaths),'exported_packages':len(packages),'export_types':dict(types),'blueprint_categories':dict(collections.Counter(r['category'] for r in classes)),'configured_catalogue_entries':sum(map(len,catalogue.values())),'blueprints_with_skeletal_assets':sum(bool(r['active_skeletal_assets']) for r in classes),'geometry_asset_exports':len(assets),'data_asset_exports':len(data),'missing_gameplay_references':len(missing),'parse_errors':len(errors)}
(ROOT/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))
