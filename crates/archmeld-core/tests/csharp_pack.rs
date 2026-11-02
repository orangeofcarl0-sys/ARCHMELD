use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
const P:&str="project.csharp";
fn copy(a:&Path,b:&Path){fs::create_dir_all(b).unwrap();for e in fs::read_dir(a).unwrap(){let e=e.unwrap();let t=b.join(e.file_name());if e.file_type().unwrap().is_dir(){copy(&e.path(),&t)}else{fs::copy(e.path(),t).unwrap();}}}
fn pos(t:&str,n:&str)->(u64,u64){let i=t.find(n).unwrap();let b=&t[..i];(b.matches('\n').count() as u64,b.rsplit('\n').next().unwrap().encode_utf16().count() as u64)}
fn has(e:&Value)->bool{e["documentChanges"].as_array().is_some_and(|c|c.iter().any(|x|x["edits"].as_array().is_some_and(|v|!v.is_empty())))||e["changes"].as_object().is_some_and(|c|c.values().any(|v|v.as_array().is_some_and(|a|!a.is_empty())))}
#[test]
fn csharp_ls_partial_delegates_and_cross_file_rename(){
 let exe=PathBuf::from("<workspace>/_archmeld_tools/csharp-tools/csharp-ls.exe");
 if !exe.is_file(){eprintln!("csharp-ls missing");return;}
 let dir=tempfile::tempdir().unwrap();let root=dir.path().join("repo");
 copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures/csharp"),&root);
 let mut st=Store::open(dir.path().join("state")).unwrap();st.ensure_project(P,"expedition").unwrap();
 let snap=capture_snapshot(&mut st,P,&root,&SnapshotInputs{build_config:json!({"kind":"csharp","tfm":"net8.0"}),environment:json!({"os":"test"})},&SnapshotOptions{include_untracked:true,..SnapshotOptions::default()}).unwrap();
 let dn="<workspace>/_archmeld_tools/dotnet8";
 let s=LspSession::start(&LspServerSpec{id:"csharp-ls".into(),program:exe,args:vec![],cwd:root.clone(),env:vec![("DOTNET_ROOT".into(),dn.into()),("PATH".into(),format!("{dn};{}",std::env::var("PATH").unwrap_or_default()))],initialization_options:None},&SpawnPermission::grant("grant.b013.csharp","C# pack test"),&PathBuf::from("<workspace>/_archmeld_temp/csharp-audit.ndjson")).expect("start");
 s.initialize(&root).expect("initialize");
 for f in ["Util.cs","Other.cs","Program.cs"]{let t=fs::read_to_string(root.join(f)).unwrap();s.did_open(&root.join(f),"csharp",&t).unwrap();}
 let util=fs::read_to_string(root.join("Util.cs")).unwrap();let (l,c)=pos(&util,"Helper");
 let mut edit=Value::Null;
 for _ in 0..30{if let Ok(v)=s.rename(&root.join("Util.cs"),l,c,"Assist",Duration::from_secs(30)){if has(&v){edit=v;break;}}std::thread::sleep(Duration::from_millis(1000));}
 assert!(has(&edit),"no csharp-ls rename edits");
 let p=preview_workspace_edit(&st,&PreviewRequest{project_id:P,snapshot_id:&snap.snapshot_id,allowed_paths:&[],max_files:8,workspace_root:Some(&root)},&edit).expect("preview");
 assert!(p.complete(),"{:?}",p.refusals);
 let files:Vec<String>=p.files.iter().map(|f|f.path.clone()).collect();
 assert!(files.contains(&"Util.cs".into())&&files.contains(&"Program.cs".into()),"{files:?}");
 assert!(!files.contains(&"Other.cs".into()),"{files:?}");
 let prog=fs::read_to_string(root.join("Program.cs")).unwrap();let (fl,fc)=pos(&prog,"f(2)");
 let def=s.request("textDocument/definition",json!({"textDocument":{"uri":file_uri(&root.join("Program.cs")).unwrap()},"position":{"line":fl,"character":fc}}),Duration::from_secs(20)).unwrap_or(Value::Null).to_string();
 assert!(!def.contains("Other.cs"),"delegate call fabricated Other.Helper: {def}");
 let sum=s.shutdown().unwrap();assert!(sum.orphan_pids.is_empty());
 let out=json!({"language":"csharp","server":"csharp-ls 0.14.0","snapshot_id":snap.snapshot_id,"rename_files":files,"delegate_target":"unresolved_or_variable","process_audit":{"orphan_pids":sum.orphan_pids,"exit_code":sum.exit_code},"runtime_trace":"pending:B016"});
 fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/evidence/csharp.json"),serde_json::to_string_pretty(&out).unwrap()).unwrap();
}
