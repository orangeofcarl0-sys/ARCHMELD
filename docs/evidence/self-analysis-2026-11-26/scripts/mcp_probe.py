import json, os, subprocess, sys, time
state = r'<path>'
exe = r'<path>'
env = dict(os.environ)
env['PATH'] = r'<path>' + env.get('PATH','')
p = subprocess.Popen([exe,'--state',state,'--project','archmeld.self'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf-8',env=env,bufsize=1)
out=[]
def call(i,method,params=None):
    req={"jsonrpc":"2.0","id":i,"method":method}
    if params is not None: req["params"]=params
    p.stdin.write(json.dumps(req)+"\n"); p.stdin.flush()
    line=p.stdout.readline()
    r=json.loads(line)
    out.append(r)
    return r
def tool(i,name,args):
    return call(i,"tools/call",{"name":name,"arguments":args})
def payload(r):
    t=r["result"]["content"][0]["text"]
    try: return json.loads(t)
    except Exception: return t
call(1,"initialize",{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"1"}})
p.stdin.write(json.dumps({"jsonrpc":"2.0","method":"notifications/initialized"})+"\n"); p.stdin.flush()
sar=payload(tool(2,"archmeld_query",{"kind":"symbols","query":"::Store","max_nodes":50}))
store=[e for e in sar.get("entities",[]) if e["qualified_name"].endswith("::Store") and e["kind"]=="type"]
print("store candidates",[(e["id"],e["qualified_name"]) for e in store])
sid=store[0]["id"]
insp=payload(tool(3,"archmeld_inspect",{"kind":"entity","id":sid}))
nb=payload(tool(4,"archmeld_query",{"kind":"neighbors","entity_ids":[sid],"max_nodes":200,"max_depth":1,"relation_kinds":["contains","imports"]}))
imp=payload(tool(5,"archmeld_query",{"kind":"impact","entity_ids":[sid],"max_nodes":100,"max_depth":1}))
ctx=payload(tool(6,"archmeld_context",{"objective":"explain how the SQLite ledger is opened, validated and used by ARCHMELD","entity_ids":[sid],"budget_tokens":4000}))
src=payload(tool(7,"archmeld_source",{"entity_id":sid}))
lib=payload(tool(8,"archmeld_query",{"kind":"symbols","query":"archmeld-api/src/lib.rs","max_nodes":5}))
print("api candidates",[(e["id"],e["kind"],e["qualified_name"]) for e in lib.get("entities",[])][:5])
api_ids=[e["id"] for e in lib.get("entities",[]) if e["kind"]=="file"]
if api_ids:
    path=payload(tool(9,"archmeld_query",{"kind":"path","entity_ids":[api_ids[0],sid],"max_nodes":50,"max_depth":6}))
else:
    path={}
cap=payload(tool(10,"archmeld_capabilities",{}))
json.dump({"store_id":sid,"inspect":insp,"neighbors":nb,"impact":imp,"context":ctx,"source":src,"path":path,"capabilities":cap},open(r'<path>','w',encoding='utf-8'),indent=1)
print("neighbors",nb.get("pagination"),len(nb.get("relations",[])),"impact",len(imp.get("relations",[])),"context tokens",ctx.get("budget",{}),"source",len(src.get("text","") if isinstance(src,dict) else ""))
p.stdin.close(); p.terminate()
