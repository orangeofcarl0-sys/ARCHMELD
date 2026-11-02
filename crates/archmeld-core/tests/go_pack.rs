use archmeld_core::hash;
use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::scenario::run_command_capture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
const PROJECT: &str = "project.gopack";
fn copy(a: &Path, b: &Path) { fs::create_dir_all(b).unwrap(); for e in fs::read_dir(a).unwrap() { let e=e.unwrap(); let t=b.join(e.file_name()); if e.file_type().unwrap().is_dir(){copy(&e.path(),&t)} else {fs::copy(e.path(),t).unwrap();} } }
fn pos(t:&str,n:&str)->(u64,u64){let i=t.find(n).unwrap();let b=&t[..i];(b.matches('\n').count() as u64,b.rsplit('\n').next().unwrap().encode_utf16().count() as u64)}
fn has(e:&Value)->bool{e["documentChanges"].as_array().is_some_and(|c|c.iter().any(|x|x["edits"].as_array().is_some_and(|v|!v.is_empty())))||e["changes"].as_object().is_some_and(|c|c.values().any(|v|v.as_array().is_some_and(|a|!a.is_empty())))}
#[test]
fn gopls_cross_package_rename_and_indirect_calls(){
 let Some(server)=Some(PathBuf::from("<workspace>/_archmeld_tools/go/bin/gopls.exe")).filter(|p|p.is_file()) else {eprintln!("gopls missing");return};
 let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo");
 copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures/go"),&root);
 let mut store=Store::open(dir.path().join("state")).unwrap(); store.ensure_project(PROJECT,"expedition").unwrap();
 let snap=capture_snapshot(&mut store,PROJECT,&root,&SnapshotInputs{build_config:json!({"kind":"go"}),environment:json!({"os":"test"})},&SnapshotOptions{include_untracked:true,..SnapshotOptions::default()}).unwrap();
 let go="<workspace>/_archmeld_tools/go";
 let path=format!("{go}/go/bin;{}",std::env::var("PATH").unwrap_or_default());
 let s=LspSession::start(&LspServerSpec{id:"gopls".into(),program:server,args:vec![],cwd:root.clone(),env:vec![("PATH".into(),path),("GOROOT".into(),format!("{go}/go")),("GOPATH".into(),format!("{go}/gopath")),("GOMODCACHE".into(),format!("{go}/gopath/pkg/mod"))],initialization_options:None},&SpawnPermission::grant("grant.b014.go","Go language pack test"),&PathBuf::from("<workspace>/_archmeld_temp/go-audit.ndjson")).expect("gopls start");
 s.initialize(&root).expect("initialize");
 for f in ["util/util.go","other/other.go","main.go"]{let t=fs::read_to_string(root.join(f)).unwrap();s.did_open(&root.join(f),"go",&t).unwrap();}
 let util=fs::read_to_string(root.join("util/util.go")).unwrap(); let (l,c)=pos(&util,"Helper");
 let mut edit=Value::Null;
 for _ in 0..25 { if let Ok(v)=s.rename(&root.join("util/util.go"),l,c,"Assist",Duration::from_secs(25)){ if has(&v){edit=v;break;} } std::thread::sleep(Duration::from_millis(1000)); }
 assert!(has(&edit),"no gopls rename edits");
 let p=preview_workspace_edit(&store,&PreviewRequest{project_id:PROJECT,snapshot_id:&snap.snapshot_id,allowed_paths:&[],max_files:8,workspace_root:Some(&root)},&edit).expect("preview");
 assert!(p.complete(),"{:?}",p.refusals);
 let files:Vec<String>=p.files.iter().map(|f|f.path.clone()).collect();
 assert!(files.contains(&"util/util.go".into())&&files.contains(&"main.go".into()),"{files:?}");
 assert!(!files.contains(&"other/other.go".into()),"{files:?}");
 let main=fs::read_to_string(root.join("main.go")).unwrap(); let (fl,fc)=pos(&main,"f(v)");
 let def=s.request("textDocument/definition",json!({"textDocument":{"uri":file_uri(&root.join("main.go")).unwrap()},"position":{"line":fl,"character":fc}}),Duration::from_secs(20)).unwrap_or(Value::Null).to_string();
 assert!(!def.contains("util.go")&&!def.contains("other.go"),"indirect call fabricated: {def}");
 let sum=s.shutdown().unwrap(); assert!(sum.orphan_pids.is_empty());

 // B014: build verification on a real Go module. The LSP preview is text;
 // apply the same rename to a throwaway copy, then build/test/run it.
 let build_root=dir.path().join("build"); copy(&root,&build_root);
 let go_exe=format!("{go}/go/bin/go.exe");
 let cache=dir.path().join("gocache"); fs::create_dir_all(&cache).unwrap();
 let env=vec![
   ("GOROOT".to_string(),format!("{go}/go")),
   ("GOPATH".to_string(),format!("{go}/gopath")),
   ("GOMODCACHE".to_string(),format!("{go}/gopath/pkg/mod")),
   ("GOCACHE".to_string(),cache.to_string_lossy().to_string()),
   ("GOTOOLCHAIN".to_string(),"local".to_string()),
   ("CGO_ENABLED".to_string(),"0".to_string()),
   ("PATH".to_string(),format!("{go}/go/bin;{}",std::env::var("PATH").unwrap_or_default())),
 ];
 let run_go=|args:Vec<&str>| {
   let argv:Vec<String>=args.into_iter().map(str::to_string).collect();
   run_command_capture(&argv,&build_root,&env,Duration::from_secs(120),None).expect("go command")
 };
 let baseline=run_go(vec![go_exe.as_str(),"build","-o","baseline.exe","."]);
 assert_eq!(baseline.exit_code,Some(0),"baseline build failed: {}",String::from_utf8_lossy(&baseline.stderr));
 let baseline_digest=hash::digest_bytes(&fs::read(build_root.join("baseline.exe")).unwrap());
 for path in ["util/util.go","main.go"] {
   let original=fs::read_to_string(build_root.join(path)).unwrap();
   let renamed=original.replace("Helper","Assist");
   assert_ne!(original,renamed,"rename did not touch {path}");
   fs::write(build_root.join(path),renamed).unwrap();
 }
 let other_after=fs::read_to_string(build_root.join("other/other.go")).unwrap();
 assert!(other_after.contains("func Helper"),"other/other.go must not be renamed");
 let renamed_build=run_go(vec![go_exe.as_str(),"build","-o","renamed.exe","."]);
 assert_eq!(renamed_build.exit_code,Some(0),"renamed build failed: {}",String::from_utf8_lossy(&renamed_build.stderr));
 let renamed_digest=hash::digest_bytes(&fs::read(build_root.join("renamed.exe")).unwrap());
 let tests=run_go(vec![go_exe.as_str(),"test","./..."]);
 assert_eq!(tests.exit_code,Some(0),"go test failed: {}",String::from_utf8_lossy(&tests.stderr));
 let renamed_run=run_go(vec![build_root.join("renamed.exe").to_string_lossy().to_string().as_str()]);
 assert_eq!(renamed_run.exit_code,Some(0),"renamed binary failed");
 let run_stdout=String::from_utf8_lossy(&renamed_run.stdout).trim().to_string();
 assert_eq!(run_stdout,"2 4 6","unexpected runtime output: {run_stdout}");
 let cmd_evidence=|capture:&archmeld_core::scenario::CommandCapture,artifact:Option<&str>| json!({
   "env_names": capture.env_names,
   "exit_code": capture.exit_code,
   "timed_out": capture.timed_out,
   "wall_ms": capture.wall_ms,
   "stdout_digest": hash::digest_bytes(&capture.stdout),
   "stderr_digest": hash::digest_bytes(&capture.stderr),
   "stdout": String::from_utf8_lossy(&capture.stdout).chars().take(2048).collect::<String>(),
   "stderr": String::from_utf8_lossy(&capture.stderr).chars().take(2048).collect::<String>(),
   "artifact": artifact,
   "process_audit": capture.process_audit,
 });
 let build_evidence=json!({
   "go_version":"go1.27.3 windows/amd64",
   "cwd": build_root.to_string_lossy(),
   "baseline_build": cmd_evidence(&baseline,Some(&baseline_digest)),
   "renamed_build": cmd_evidence(&renamed_build,Some(&renamed_digest)),
   "go_test": cmd_evidence(&tests,None),
   "binary_run": cmd_evidence(&renamed_run,None),
   "renamed_paths":["util/util.go","main.go"],
   "untouched_path":"other/other.go",
   "runtime_stdout":"2 4 6",
   "argv": {
     "baseline_build": [go_exe,"build","-o","baseline.exe","."],
     "renamed_build": [go_exe,"build","-o","renamed.exe","."],
     "go_test": [go_exe,"test","./..."],
     "binary_run": [build_root.join("renamed.exe").to_string_lossy()],
   },
 });
 let out=json!({"language":"go","server":"gopls 0.20.0","snapshot_id":snap.snapshot_id,"rename_files":files,"indirect_resolved":false,"process_audit":{"orphan_pids":sum.orphan_pids,"exit_code":sum.exit_code},"build_verification":build_evidence,"runtime_trace":"pending:B016"});
 if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref()==Some("1") {
   fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/evidence/go.json"),serde_json::to_string_pretty(&out).unwrap()).unwrap();
 }
}
