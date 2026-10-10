// Per-frame puppet pose publication. Rust proves both components live and of the
// exact classes before the call; this checks the pinned layouts, then copies.
int32_t puppet_publish(void* calculator,void* render,uint32_t count,char* reason,uint32_t capacity){
    try{
        require(calculator&&render&&calculator!=render&&count>0&&count<=1024,"puppet pose arguments");
        require(pose_build_admit&&pose_build_admit(),"puppet pose shipping profile unsupported");
        require(scene_vtable_read(calculator)==pose_image+0x7646b38&&scene_vtable_read(render)==pose_image+0x7659cb0,"puppet pose component class");
        pose_local(calculator,count);pose_buffers(calculator,count);pose_buffers(render,count);
        reinterpret_cast<void(*)(void*,void*)>(pose_image+0x3bc7c70)(calculator,nullptr);
        const auto source=pose_buffers(calculator,count);const auto target=pose_buffers(render,count);
        const auto bytes=static_cast<size_t>(count)*sizeof(EngineTransform);
        const auto from=reinterpret_cast<uintptr_t>(source.arrays[source.read].data),to=reinterpret_cast<uintptr_t>(target.arrays[target.editable].data);
        require(from+bytes<=to||to+bytes<=from,"puppet pose storage overlaps");
        std::memcpy(target.arrays[target.editable].data,source.arrays[source.read].data,bytes);
        static_cast<uint8_t*>(render)[0x798]|=0x40; // FillCS publication flag
        reinterpret_cast<void(*)(void*)>(pose_image+0x3c02930)(render);
        reinterpret_cast<void(*)(void*,uint32_t,uint8_t)>(pose_image+0x3bf7370)(render,0,0);
        reinterpret_cast<void(*)(void*)>(pose_image+0x3c23f20)(render);
        reinterpret_cast<void(*)(void*)>(pose_image+0x3b51a10)(render);
        reinterpret_cast<void(*)(void*)>(pose_image+0x3b517d0)(render);
        const auto published=pose_buffers(render,count);
        require(published.read==target.editable&&!(published.flags&0x40),"puppet pose publication incomplete");
        return 1;
    }catch(const std::exception& error){
        if(reason&&capacity){const auto n=std::min<size_t>(std::strlen(error.what()),capacity-1);std::memcpy(reason,error.what(),n);reason[n]='\0';}
        return 0;
    }
}
