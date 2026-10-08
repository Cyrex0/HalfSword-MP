"""Read-only actor retirement analysis on the existing isolated game IDB copy."""
import json
from pathlib import Path
import traceback
import ida_auto, ida_bytes, ida_funcs, ida_hexrays, ida_nalt, ida_pro, ida_segment, ida_ua, ida_lines, idautils

OUT=Path(r"D:\HalfswordMultiplayert\test-results\dev-feature-checks\weapon-catalogue-ida\actor-retirement")
OUT.mkdir(parents=True,exist_ok=True)
def save(name,value):
    (OUT/name).write_text(json.dumps(value,indent=2),encoding="utf-8")
def main():
    save("progress.json",{"stage":"auto_wait"})
    ida_auto.auto_wait()
    save("progress.json",{"stage":"strings"})
    strings=idautils.Strings()
    strings.setup(strtypes=[ida_nalt.STRTYPE_C,ida_nalt.STRTYPE_C_16])
    matches=[];candidates={}
    needles=("K2_DestroyActor","DestroyActor","IsActorBeingDestroyed","LifeSpanExpired","MarkAsGarbage","GetAllActorsOfClass")
    for item in strings:
        try:s=str(item)
        except Exception:continue
        if not any(n in s for n in needles):continue
        if len(s)>1200:continue
        row={"address":hex(item.ea),"string":s,"xrefs":[]}
        for x in idautils.XrefsTo(item.ea):
            f=ida_funcs.get_func(x.frm)
            entry={"address":hex(x.frm),"function":hex(f.start_ea) if f else None,"adjacent_function_pointers":[],"raw_words":[]}
            if f:candidates[f.start_ea]="direct string reference"
            # Native registration pairs often hold {name pointer,function ptr}.
            # Record all observed offsets, rather than assuming the layout.
            for off in (-16,-8,8,16,24):
                value=ida_bytes.get_qword(x.frm+off)
                entry['raw_words'].append({'offset':off,'value':hex(value)})
                target=ida_funcs.get_func(value)
                if target and target.start_ea==value:
                    entry["adjacent_function_pointers"].append({"offset":off,"target":hex(value)})
                    candidates[value]="adjacent native registration pointer offset "+str(off)
                elif off==8 and ida_segment.getseg(value) and ida_segment.getseg(value).type==ida_segment.SEG_CODE:
                    # Tiny exec thunks may not be entered in the imported IDB
                    # function index. Decode observed code without adding one.
                    at=value;decoded=[]
                    for _ in range(24):
                        insn=ida_ua.insn_t();size=ida_ua.decode_insn(insn,at)
                        if not size:break
                        decoded.append({'address':hex(at),'text':ida_lines.tag_remove(ida_lines.generate_disasm_line(at,0) or '')})
                        for op in insn.ops:
                            if op.type==ida_ua.o_near:
                                cf=ida_funcs.get_func(op.addr)
                                if cf:candidates[cf.start_ea]='callee of observed unindexed native thunk '+hex(value)
                        at+=size
                        if insn.get_canon_mnem() in ('retn','ret','jmp'):break
                    entry['unindexed_thunk']=decoded
            row["xrefs"].append(entry)
        matches.append(row)
    save("strings.json",matches)
    save("progress.json",{"stage":"decompile","candidates":len(candidates)})
    queue=list(candidates);exports=[];done=set()
    for address in queue:
        if len(exports)>=70:break
        if address in done:continue
        done.add(address);f=ida_funcs.get_func(address)
        if not f:continue
        row={"address":hex(address),"name":ida_funcs.get_func_name(address),"reason":candidates[address],"size":f.end_ea-f.start_ea}
        try:
            c=ida_hexrays.decompile(address)
            if c:
                file=f"actor-{address:x}.c";(OUT/file).write_text(str(c),encoding="utf-8");row["file"]=file
        except Exception as ex:row["error"]=str(ex)
        # Follow small wrapper direct callees only, bounded overall. This
        # distinguishes UFunction getters from the actual native exec thunk.
        if f.end_ea-f.start_ea<600:
            callees=[]
            for item in idautils.FuncItems(address):
                for target in idautils.CodeRefsFrom(item,False):
                    cf=ida_funcs.get_func(target)
                    if cf and cf.start_ea==target and target!=address:
                        callees.append(hex(target))
                        if target not in candidates:
                            candidates[target]="direct callee of "+hex(address);queue.append(target)
            row['direct_callees']=sorted(set(callees))
        exports.append(row)
    digest=ida_nalt.retrieve_input_file_sha256()
    save("index.json",{"input":ida_nalt.get_input_file_path(),"sha256":digest.hex() if digest else None,"exports":exports})
    save("progress.json",{"stage":"complete","exports":len(exports)})
try:main()
except Exception:
    (OUT/"error.txt").write_text(traceback.format_exc(),encoding="utf-8")
finally:ida_pro.qexit(0)
