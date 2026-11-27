import json, os, subprocess
state=r'<path>'
exe=r'<path>'
env=dict(os.environ); env['PATH']=r'<path>'+env.get('PATH','')
p=subprocess.Popen([exe,'--state',state,'--project','archmeld.self'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf-8',env=env,bufsize=1)
n=0
def call(method,params=None):
    global n; n+=1
    req={"jsonrpc":"2.0","id":n,"method":method}
    if params is not None: req["params"]=params
    p.stdin.write(json.dumps(req)+"\n"); p.stdin.flush()
    return json.loads(p.stdout.readline())
def tool(name,args):
    r=call("tools/call",{"name":name,"arguments":args})
    return json.loads(r["result"]["content"][0]["text"])
call("initialize",{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"p2","version":"1"}})
p.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n'); p.stdin.flush()
res={}
q=tool("archmeld_query",{"kind":"symbols","query":"archmeld-api/src/lib.rs","max_nodes":100})
fileent=[e for e in q["entities"] if e["kind"]=="file" and e["qualified_name"]=="crates/archmeld-api/src/lib.rs"]
q2=tool("archmeld_query",{"kind":"symbols","query":"::Store","max_nodes":100})
store=[e for e in q2["entities"] if e["qualified_name"]=="crates/archmeld-core/src/store/mod.rs::Store"][0]
res["path"]=tool("archmeld_query",{"kind":"path","entity_ids":[fileent[0]["id"],store["id"]],"max_depth":6,"max_nodes":50}) if fileent else {"error":"no api file entity"}
res["text_format"]=tool("archmeld_query",{"kind":"text","query":"archmeld-export-v1","max_nodes":20})
res["source_scip"]=tool("archmeld_query",{"kind":"source","query":"scip_tool_info_missing","max_nodes":10})
res["inspect_snapshot"]=tool("archmeld_inspect",{"kind":"snapshot","id":q["snapshot_id"]})
json.dump(res,open(r'<path>','w',encoding='utf-8'),indent=1)
print("path:",json.dumps({k:res["path"].get(k) for k in ("agent_summary","found","pagination")},ensure_ascii=False)[:500])
print("text hits:",[e["qualified_name"] for e in res["text_format"].get("entities",[])][:10])
print("source hits:",[(e["qualified_name"],e.get("anchors",[{}])[0].get("path")) for e in res["source_scip"].get("entities",[])][:10])
print("snapshot:",{k:res["inspect_snapshot"]["object"].get(k) for k in ("id","git_commit","created_at")})
p.stdin.close(); p.terminate()
