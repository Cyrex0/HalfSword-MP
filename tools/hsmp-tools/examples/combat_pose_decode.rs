//! Decode opt-in native pose evidence with the exact production codec.
//! cargo run -p hsmp-tools --example combat_pose_decode -- <tap.jsonl> <out.jsonl>
use std::{fs::File,io::{BufRead,BufReader,BufWriter,Write}};

fn main()->anyhow::Result<()> {
    let mut args=std::env::args().skip(1);
    let input=args.next().ok_or_else(||anyhow::anyhow!("missing IPC tap path"))?;
    let output=args.next().ok_or_else(||anyhow::anyhow!("missing output path"))?;
    let mut out=BufWriter::new(File::create(output)?);
    let mut count=0;
    for line in BufReader::new(File::open(input)?).lines() {
        let v:serde_json::Value=match serde_json::from_str(&line?){Ok(v)=>v,Err(_)=>continue};
        if v["ev"]!="pose_tx" {continue;}
        let hex=v["payload_hex"].as_str().ok_or_else(||anyhow::anyhow!("pose evidence missing bytes"))?;
        anyhow::ensure!(hex.len()%2==0 && hex.is_ascii(),"invalid evidence hex");
        let bytes:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16))
            .collect::<Result<_,_>>()?;
        let view=hsmp_ipc::record::view::<hsmp_ipc::schema::pose::PoseHead>(&bytes).map_err(|e|anyhow::anyhow!("invalid native pose head: {e:?}"))?;
        let frame=hsmp_pose::posecodec::v2::decode(&view.rows).ok_or_else(||anyhow::anyhow!("invalid native pose codec"))?;
        let bones:serde_json::Map<String,serde_json::Value>=hsmp_pose::posecodec::v2::BONES.iter().enumerate()
            .map(|(i,n)|(n.to_string(),serde_json::json!({"p":frame.bones[i].p,"q":frame.bones[i].q,"v":frame.bones[i].v,"w":frame.bones[i].w}))).collect();
        let weapons:Vec<_>=frame.weapons.iter().map(|w|serde_json::json!({"hands":w.hands,"id":w.id,"p":w.p,"q":w.q,"v":w.v,"w":w.w,
            "boxes":w.boxes.iter().map(|b|serde_json::json!({"component":b.component,"child_of":b.child_of,"class_hash":b.class_hash,
                "p":b.p,"q":b.q,"half":b.half,"native_scale":b.native_scale})).collect::<Vec<_>>()})).collect();
        writeln!(out,"{}",serde_json::json!({"t":v["t"],"world_epoch":v["world_epoch"],"tick":view.head.tick,"ts":frame.ts,"k":frame.k,"step":frame.step,
            "context":frame.context.map(|c|serde_json::json!({"match_id":c.match_id,"round":c.round,"life":c.life})),"bones":bones,"weapons":weapons}))?;
        count+=1;
    }
    out.flush()?;
    println!("Decoded {count} exact transmitted native pose frames.");
    Ok(())
}
