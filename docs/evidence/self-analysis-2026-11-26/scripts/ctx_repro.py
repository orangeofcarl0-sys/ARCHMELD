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
call("initialize",{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ctx","version":"1"}})
p.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n'); p.stdin.flush()
q=tool("archmeld_query",{"kind":"symbols","query":"::Store","max_nodes":50})
sid=[e["id"] for e in q["entities"] if e["qualified_name"].endswith("::Store")][0]
obj="explain how the SQLite ledger is opened, validated and used by ARCHMELD"
for budget in (4000,4500,5000,6000,8000):
    v=tool("archmeld_context",{"objective":obj,"entity_ids":[sid],"budget_tokens":budget})
    print(json.dumps({"budget":budget,"bytes":len(json.dumps(v,ensure_ascii=False)),"page":v.get("page"),"selected":[e["qualified_name"] for e in v.get("selected_entities",[])],"snippets":len(v.get("source_snippets",[])),"slices":len(v.get("graph_slices",[])),"omissions":len(v.get("omissions",[])),"truncated":v.get("budget",{}).get("truncated")},ensure_ascii=False))
p.stdin.close(); p.terminate()
