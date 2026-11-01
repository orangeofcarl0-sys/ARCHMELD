use archmeld_core::scenario::{
    run_scenario, run_scenario_with_cancel, ExecutionGrant, ScenarioSpec,
};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
const P:&str="project.scenario";
#[test]
fn scenarios_require_grant_capture_artifacts_and_timeout(){
 let dir=tempfile::tempdir().unwrap();let root=dir.path().join("repo");fs::create_dir_all(&root).unwrap();fs::write(root.join("a.py"),"x=1\n").unwrap();
 let mut st=Store::open(dir.path().join("state")).unwrap();st.ensure_project(P,"expedition").unwrap();
 let snap=capture_snapshot(&mut st,P,&root,&SnapshotInputs{build_config:json!({"variant":"test"}),environment:json!({"os":"test"})},&SnapshotOptions{include_untracked:true,..SnapshotOptions::default()}).unwrap();
 let cfg=snap.snapshot["config_digest"].as_str().unwrap().to_string();
 let spec=|run:&str,argv:Vec<String>,timeout:Duration|ScenarioSpec{project_id:P.into(),run_id:run.into(),snapshot_id:snap.snapshot_id.clone(),config_digest:cfg.clone(),argv,cwd:root.clone(),env:vec![],timeout,sandbox:None};
 let denied=run_scenario(&mut st,&spec("run.denied",vec!["python".into(),"-c".into(),"print(1)".into()],Duration::from_secs(5)),&ExecutionGrant::deny("no approval"));
 assert!(denied.is_err());
 let ok=run_scenario(&mut st,&spec("run.ok",vec!["python".into(),"-c".into(),"print('hello')".into()],Duration::from_secs(10)),&ExecutionGrant::grant("g1","test")).unwrap();
 assert_eq!(ok["exit_code"],0);assert_eq!(ok["timed_out"],false);assert_eq!(ok["argv"][0],"python");
 assert!(ok["stdout"]["digest"].as_str().unwrap().starts_with("sha256:"));
 assert_eq!(ok["stdout"]["truncated"],false);
 let slow=run_scenario(&mut st,&spec("run.slow",vec!["python".into(),"-c".into(),"import time;time.sleep(5)".into()],Duration::from_millis(400)),&ExecutionGrant::grant("g2","timeout test")).unwrap();
 assert_eq!(slow["timed_out"],true);
 assert_eq!(slow["completion"],"timed_out");
 assert_eq!(slow["process_audit"]["state"],"complete");
 assert!(st.integrity_check().unwrap().is_empty());
}

#[test]
fn cancellation_kills_the_process_tree_and_is_never_complete(){
 let dir=tempfile::tempdir().unwrap();let root=dir.path().join("repo");fs::create_dir_all(&root).unwrap();fs::write(root.join("a.py"),"x=1\n").unwrap();
 let mut st=Store::open(dir.path().join("state")).unwrap();st.ensure_project(P,"expedition").unwrap();
 let snap=capture_snapshot(&mut st,P,&root,&SnapshotInputs{build_config:json!({"variant":"test"}),environment:json!({"os":"test"})},&SnapshotOptions{include_untracked:true,..SnapshotOptions::default()}).unwrap();
 let cfg=snap.snapshot["config_digest"].as_str().unwrap().to_string();
 let script="import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); time.sleep(60)";
 let spec=ScenarioSpec{project_id:P.into(),run_id:"run.cancel".into(),snapshot_id:snap.snapshot_id.clone(),config_digest:cfg.clone(),argv:vec!["python".into(),"-c".into(),script.into()],cwd:root.clone(),env:vec![],timeout:Duration::from_secs(30),sandbox:None};
 let cancel=AtomicBool::new(false);
 let manifest=std::thread::scope(|scope|{
  scope.spawn(||{std::thread::sleep(Duration::from_millis(1200));cancel.store(true,Ordering::SeqCst);});
  run_scenario_with_cancel(&mut st,&spec,&ExecutionGrant::grant("g3","cancel test"),Some(&cancel)).unwrap()
 });
 assert_eq!(manifest["cancelled"],true);
 assert_eq!(manifest["timed_out"],false);
 assert_eq!(manifest["completion"],"cancelled");
 assert_eq!(manifest["coverage"]["state"],"partial");
 assert_eq!(manifest["process_audit"]["state"],"complete");
 assert!(manifest["process_audit"]["observed"].as_u64().unwrap()>=1,"child process must be observed: {}",manifest["process_audit"]);
 assert_eq!(manifest["process_audit"]["survivors"].as_array().unwrap().len(),0,"no descendant may survive: {}",manifest["process_audit"]);
 assert!(manifest["process_audit"]["cleanup"].as_array().unwrap().iter().any(|item|item["result"]=="terminated"));
 assert_eq!(manifest["overhead"]["state"],"unknown");
 assert!(manifest["stdout"]["digest"].as_str().unwrap().starts_with("sha256:"));
 assert!(st.integrity_check().unwrap().is_empty());
}

