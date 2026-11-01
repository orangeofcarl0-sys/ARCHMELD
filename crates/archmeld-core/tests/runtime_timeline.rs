use archmeld_core::runtime::{import_trace, timeline, RuntimeImport};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
const P:&str="project.runtime";
fn trace(cfg:&str,snapshot:&str,lossy:bool)->Value{json!({
 "version":"1.0","snapshot_id":snapshot,"config_digest":cfg,
 "build_identity":{"kind":"native","variant":"test"},
 "capture":{"tool":"viztracer","version":"0.16","lossy":lossy,"loss_notes":if lossy {json!(["summary export drops sub-microsecond events"])} else {json!([])}},
 "clock":{"kind":"monotonic","unit":"ns","origin_ns":"0"},
 "spans":[
  {"span_id":"s1","parent_span_id":null,"name":"main","kind":"function","start_ns":"9007199254740993","end_ns":"9007199254740999","process_id":"1","thread_id":"1","attributes":{}},
  {"span_id":"s2","parent_span_id":"s1","name":"work","kind":"function","start_ns":"9007199254740994","end_ns":"9007199254740995","process_id":"1","thread_id":"1","attributes":{"x":1}}
 ]})}
#[test]
fn runtime_imports_big_decimal_ns_and_reports_binding_and_loss(){
 let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); fs::create_dir_all(root.join("src")).unwrap(); fs::write(root.join("src/a.py"),"x=1\n").unwrap();
 let mut st=Store::open(dir.path().join("state")).unwrap(); st.ensure_project(P,"expedition").unwrap();
 let snap=capture_snapshot(&mut st,P,&root,&SnapshotInputs{build_config:json!({"variant":"test"}),environment:json!({"os":"test"})},&SnapshotOptions{include_untracked:true,..SnapshotOptions::default()}).unwrap();
 let cfg=snap.snapshot["config_digest"].as_str().unwrap().to_string();
 let report=import_trace(&mut st,&RuntimeImport{project_id:P,run_id:"run.trace.1",snapshot_id:Some(&snap.snapshot_id),config_digest:&cfg,allow_unbound:false,scenario_run_id:None},&trace(&cfg,&snap.snapshot_id,false)).unwrap();
 assert_eq!(report["bound"],true); assert_eq!(report["clock"]["unit"],"ns");
 assert_eq!(report["spans"][1]["start_ns"],"9007199254740994");
 assert_ne!(report["spans"][1]["start_ns"],report["spans"][1]["end_ns"]);
 let window=timeline(&st,P,"run.trace.1",Some("9007199254740994"),Some("9007199254740994"),10).unwrap();
 let ids:Vec<&str>=window["spans"].as_array().unwrap().iter().map(|s|s["span_id"].as_str().unwrap()).collect();
 assert!(ids.contains(&"s2"),"{ids:?}");
 assert!(ids.contains(&"s1"),"enclosing span must be included");
 let wrong=import_trace(&mut st,&RuntimeImport{project_id:P,run_id:"run.trace.2",snapshot_id:Some(&snap.snapshot_id),config_digest:&cfg,allow_unbound:false,scenario_run_id:None},&{let mut t=trace(&cfg,&snap.snapshot_id,false);t["config_digest"]=json!(format!("sha256:{}","a".repeat(64)));t});
 assert!(wrong.is_err());
 let unbound=import_trace(&mut st,&RuntimeImport{project_id:P,run_id:"run.trace.3",snapshot_id:Some(&snap.snapshot_id),config_digest:&cfg,allow_unbound:true,scenario_run_id:None},&{let mut t=trace(&cfg,&snap.snapshot_id,true);t["config_digest"]=json!(format!("sha256:{}","b".repeat(64)));t}).unwrap();
 assert_eq!(unbound["bound"],false); assert_eq!(unbound["lossy"],true); assert!(!unbound["loss_notes"].as_array().unwrap().is_empty());
 let dup=import_trace(&mut st,&RuntimeImport{project_id:P,run_id:"run.trace.4",snapshot_id:Some(&snap.snapshot_id),config_digest:&cfg,allow_unbound:false,scenario_run_id:None},&{let mut t=trace(&cfg,&snap.snapshot_id,false);t["spans"][1]["span_id"]=json!("s1");t});
 assert!(dup.is_err());
 let float=import_trace(&mut st,&RuntimeImport{project_id:P,run_id:"run.trace.5",snapshot_id:Some(&snap.snapshot_id),config_digest:&cfg,allow_unbound:false,scenario_run_id:None},&{let mut t=trace(&cfg,&snap.snapshot_id,false);t["spans"][0]["start_ns"]=json!(9007199254740993u64);t});
 assert!(float.is_err());
 assert!(st.integrity_check().unwrap().is_empty());
}
