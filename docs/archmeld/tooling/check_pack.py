#!/usr/bin/env python3
"""Validate the ARCHMELD specification pack, NOT a product implementation.

No network, project execution, downloads, or automatic file changes.
Use --output PATH to write this checker's results explicitly.
"""
from __future__ import annotations
import argparse
import hashlib
import importlib.metadata
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote, urlparse

try:
    from jsonschema import Draft202012Validator, FormatChecker
    from referencing import Registry, Resource
except ImportError as exc:
    raise SystemExit("Install tooling/requirements-validation.txt before running this checker") from exc

ROOT = Path(__file__).resolve().parents[1]
NS = "https://archmeld.invalid/contracts/1.0/"
BASE = NS + "archmeld.schema.json"
results: list[dict] = []

def no_duplicate_keys(pairs):
    out = {}
    for key, value in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key: {key}")
        out[key] = value
    return out

def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=no_duplicate_keys,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))

def digest_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()

def canonical(value) -> bytes:
    """Restricted AM-C14N-1; not a general RFC 8785 implementation."""
    def check(x):
        if x is None or isinstance(x, bool):
            return
        if isinstance(x, int):
            if abs(x) > 2**53 - 1:
                raise ValueError("integer exceeds AM-C14N-1 range")
            return
        if isinstance(x, str):
            x.encode("utf-8", errors="strict")
            return
        if isinstance(x, list):
            for item in x:
                check(item)
            return
        if isinstance(x, dict):
            for key, item in x.items():
                if not isinstance(key, str) or not key.isascii():
                    raise ValueError("AM-C14N-1 object keys must be ASCII")
                check(item)
            return
        raise ValueError(f"unsupported canonical type {type(x).__name__}")
    check(value)
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"),
                      allow_nan=False).encode("utf-8")

def h(value) -> str:
    return digest_bytes(canonical(value))

def checked(name, action):
    try:
        details = action()
        results.append({"check": name, "status": "passed", "details": details or "OK"})
        print(f"PASS {name}")
    except Exception as exc:
        results.append({"check": name, "status": "failed", "details": str(exc)})
        print(f"FAIL {name}: {exc}")

def require(condition, message):
    if not condition:
        raise AssertionError(message)

def walk_values(value):
    if isinstance(value, dict):
        yield value
        for item in value.values():
            yield from walk_values(item)
    elif isinstance(value, list):
        for item in value:
            yield from walk_values(item)

def acyclic(edges):
    seen, stack = set(), set()
    def visit(node):
        require(node in edges, f"missing dependency {node}")
        require(node not in stack, f"cycle at {node}")
        if node in seen:
            return
        stack.add(node)
        for dep in edges[node]:
            visit(dep)
        stack.remove(node)
        seen.add(node)
    for node in edges:
        visit(node)

def semantic_errors(kind, x):
    """A small explicit specification-example checker, not the product's policy engine."""
    errors = set()
    for item in walk_values(x):
        if item.get("precision") == "bytes":
            if item["byte_start"] > item["byte_end"]:
                errors.add("anchor_range")
        if {"expected_count", "observed_count", "omitted_count", "state"} <= item.keys():
            e, o, omitted = item["expected_count"], item["observed_count"], item["omitted_count"]
            if e is not None and o + omitted > e:
                errors.add("coverage_counts")
            if item["state"] == "complete" and (e is None or e != o or omitted or item["unresolved_count"]):
                errors.add("coverage_counts")
    if kind == "Plan":
        try:
            acyclic({s["id"]: s["depends_on"] for s in x["steps"]})
        except AssertionError:
            errors.add("plan_acyclic")
    if kind == "BehaviorContract":
        expected = h({k: v for k, v in x.items() if k not in {"digest", "frozen"}})
        if x["digest"] != expected:
            errors.add("contract_digest")
        for c in x["checks"]:
            if (c["command"] is None) == (c["checker_id"] is None):
                errors.add("check_runner")
    if kind in {"ChangeSet", "ChangeProposal"}:
        for op in x["operations"]:
            b, a, content, dst = (op[k] for k in ["before_digest", "after_digest", "content_artifact", "destination"])
            ok = {"create": b is None and a is not None and a == content and dst is None,
                  "update": b is not None and a is not None and a == content and dst is None,
                  "delete": b is not None and a is None and content is None and dst is None,
                  "move": b is not None and a == b and dst is not None and dst != op["path"] and content in {None, a}}[op["op"]]
            if not ok:
                errors.add("write_operation")
    return errors

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Explicit output path; no product tests are executed")
    args = parser.parse_args()
    all_json = [p for p in ROOT.rglob("*.json") if "validation" not in p.parts]
    def json_parse():
        for path in all_json:
            read_json(path)
        return {"files": len(all_json)}
    checked("JSON syntax and duplicate-key rejection", json_parse)
    schema = read_json(ROOT / "contracts/schemas/archmeld.schema.json")
    registry = Registry()
    schemas = sorted((ROOT / "contracts/schemas").glob("*.json"))
    for path in schemas:
        content = read_json(path)
        registry = registry.with_resource(content["$id"], Resource.from_contents(content))
    def schema_meta():
        for path in schemas:
            Draft202012Validator.check_schema(read_json(path))
        return {"schema_files": len(schemas), "definitions": len(schema["$defs"])}
    checked("JSON Schema meta-validation", schema_meta)
    idx = read_json(ROOT / "examples/index.json")
    def validator(kind):
        return Draft202012Validator({"$ref": BASE + "#/$defs/" + kind}, registry=registry,
                                    format_checker=FormatChecker())
    def valid_examples():
        for f in idx["valid"]:
            x = read_json(ROOT / f["path"])
            errors = list(validator(f["schema_definition"]).iter_errors(x))
            require(not errors, f"{f['path']}: " + "; ".join(e.message for e in errors[:3]))
            require(not semantic_errors(f["schema_definition"], x), f"semantic failure {f['path']}")
        return {"valid_examples": len(idx["valid"])}
    checked("Valid contract examples", valid_examples)
    def invalid_examples():
        for f in idx["invalid_schema"]:
            require(not validator(f["schema_definition"]).is_valid(read_json(ROOT/f["path"])),
                    f"invalid example unexpectedly accepted: {f['path']}")
        return {"expected_rejections": len(idx["invalid_schema"])}
    checked("Invalid schema examples rejected", invalid_examples)
    def semantics():
        for f in idx["invalid_semantics"]:
            x = read_json(ROOT/f["path"])
            require(validator(f["schema_definition"]).is_valid(x), f"semantic fixture is not schema-valid: {f['path']}")
            require(f["semantic_rule"] in semantic_errors(f["schema_definition"], x),
                    f"missing semantic rejection: {f['path']}")
        return {"semantic_cases": len(idx["invalid_semantics"]), "scope": "Only documented fixture rules, not full S19 implementation"}
    checked("Schema-valid semantic negatives", semantics)
    def hashes():
        vectors = read_json(ROOT/"examples/hash-vectors.json")
        for item in vectors["vectors"]:
            require(canonical(item["input"]).decode()==item["canonical_utf8"], "canonical bytes mismatch")
            require(h(item["input"])==item["digest"], "canonical hash mismatch")
        for item in vectors["reject_cases"]:
            try:
                canonical(item["input"])
            except ValueError:
                continue
            raise AssertionError("canonical reject input accepted")
        for item in vectors["blob_vectors"]:
            require(digest_bytes((ROOT/item["path"]).read_bytes())==item["digest"], "blob hash mismatch")
        snap=read_json(ROOT/"examples/valid/snapshot.json")
        require(h({k:v for k,v in snap.items() if k not in {"id","created_at"}})==snap["id"], "snapshot digest mismatch")
        return {"canonical_vectors": len(vectors["vectors"]), "canonical_rejections": len(vectors["reject_cases"]), "blob_vectors": len(vectors["blob_vectors"])}
    checked("Canonicalization and source-byte digests", hashes)
    def examples_integrity():
        artifacts=read_json(ROOT/"examples/artifact-map.json")["artifacts"]
        blobs={}
        for a in artifacts:
            data=(ROOT/a["path"]).read_bytes()
            require(digest_bytes(data)==a["digest"], f"bad artifact {a['path']}")
            blobs[a["digest"]]=data
        b=read_json(ROOT/"examples/valid/evidence-batch.json")
        entities={x["id"] for x in b["entities"]};ev={x["id"] for x in b["evidence"]}
        for x in b["entities"]+b["evidence"]+b["relations"]:
            require(x["project_id"]==b["project_id"] and x["snapshot_id"]==b["snapshot_id"] and x["config_digest"]==b["config_digest"],"scope mismatch")
        for e in b["entities"]:
            for anchor in e["anchors"]:
                raw=blobs[anchor["blob_digest"]];lo,hi=anchor["byte_start"],anchor["byte_end"]
                require(0<=lo<=hi<=len(raw),"out of byte range")
                raw[:lo].decode("utf-8");raw[lo:hi].decode("utf-8")
        for edge in b["relations"]:
            require(edge["source_id"] in entities and edge["target_id"] in entities,"dangling endpoint")
            require(set(edge["evidence_ids"])<=ev,"dangling evidence")
        c=read_json(ROOT/"examples/valid/context-pack.json")
        for excerpt in c["source_excerpts"]:
            a=excerpt["anchor"];text=blobs[a["blob_digest"]][a["byte_start"]:a["byte_end"]].decode()
            require(text==excerpt["text"],"context source not exact")
        prepared=read_json(ROOT/"examples/assets/prepared-manifest.json")
        approval=read_json(ROOT/"examples/valid/approval.json")
        require(h(prepared)==approval["target_digest"],"synthetic approval does not match prepared input")
        return {"artifacts":len(artifacts),"note":"No real authorization is created"}
    checked("Synthetic cross-object references and anchors", examples_integrity)
    def graph_and_traceability():
        tasks=read_json(ROOT/"execution/work-items.json")["tasks"]
        reqs=read_json(ROOT/"acceptance/requirements.json")["requirements"]
        tests=read_json(ROOT/"acceptance/test-cases.json")["cases"]
        tm={x["id"]:x for x in tasks};rm={x["id"]:x for x in reqs};am={x["id"]:x for x in tests}
        require(len(tm)==len(tasks) and len(rm)==len(reqs) and len(am)==len(tests),"duplicate task/requirement/test ID")
        acyclic({t["id"]:t["depends_on"] for t in tasks})
        acyclic({t["id"]:list(set(t["depends_on"]+t.get("acceptance_depends_on",[]))) for t in tasks})
        specids={p.name.split('_')[0] for p in (ROOT/"specs").glob('S*.md')}
        for t in tasks:
            require(t["status"]=="not_started","pack must not predeclare task completion")
            require(set(t["spec_ids"])<=specids,"missing spec")
            for aid in t["acceptance_test_ids"]:
                require(am[aid]["owner_task"]==t["id"],"task/test mismatch")
            for rid in t["requirement_ids"]:
                require(rid in rm,"unknown requirement")
        for r in reqs:
            require(r["owner_task"] in tm and r["spec_id"] in specids,"bad requirement owner/spec")
            require(r["acceptance_tests"],"requirement without acceptance")
            for aid in r["acceptance_tests"]:
                require(r["id"] in am[aid]["requirement_ids"],"reverse traceability missing")
        for t in tests:
            require(t["status"]=="not_run","product tests must remain not_run")
            require(t["id"] in tm[t["owner_task"]]["acceptance_test_ids"],"unowned test")
        return {"tasks":len(tasks),"requirements":len(reqs),"product_acceptance_specifications":len(tests),"product_tests_executed":0}
    checked("Task DAG and bidirectional traceability", graph_and_traceability)
    def source_references():
        entries=read_json(ROOT/"research/sources.json")["sources"]
        ids={s["id"] for s in entries}
        require(len(ids)==len(entries),"duplicate source ID")
        missing=[]
        for p in ROOT.rglob('*.md'):
            for ident in re.findall(r'\bSRC-[A-Z0-9-]+\b',p.read_text(encoding='utf-8')):
                if ident not in ids:missing.append((str(p.relative_to(ROOT)),ident))
        require(not missing,str(missing))
        return {"sources":len(entries),"network_revalidation":False}
    checked("Source registry references", source_references)
    def links():
        checked_count=0;missing=[]
        for p in ROOT.rglob('*.md'):
            text=re.sub(r'```.*?```','',p.read_text(encoding='utf-8'),flags=re.S)
            for target in re.findall(r'\]\(([^)]+)\)',text):
                target=target.strip().split(' "')[0].strip('<>')
                if urlparse(target).scheme or target.startswith('#'):continue
                target=unquote(target.split('#')[0])
                if not target:continue
                checked_count+=1
                dest=(p.parent/target).resolve()
                if not dest.exists():missing.append((str(p.relative_to(ROOT)),target))
        require(not missing,str(missing))
        return {"local_markdown_links":checked_count}
    checked("Local Markdown links", links)
    def refs():
        count=0
        for path in all_json:
            root=read_json(path)
            for node in walk_values(root):
                if '$ref' not in node:continue
                value=node['$ref'];base,_,frag=value.partition('#')
                if base.startswith(NS):target=ROOT/'contracts/schemas'/base[len(NS):]
                elif base.startswith('http'):raise AssertionError(f"remote schema ref disallowed: {value}")
                elif base:target=path.parent/base
                else:target=path
                doc=read_json(target)
                if frag:
                    for part in frag.lstrip('/').split('/'):
                        part=part.replace('~1','/').replace('~0','~')
                        # MCP tools have nested schema roots, not the enclosing catalog root.
                        if target==path and path.name=='mcp-tools.json':break
                        doc=doc[int(part)] if isinstance(doc,list) else doc[part]
                count+=1
        return {"references_resolved":count,"note":"MCP schema roots separately checked below"}
    checked("Local JSON references", refs)
    def api_mcp():
        api=read_json(ROOT/'contracts/openapi.json')
        require(api['openapi']=='3.1.0','wrong OpenAPI version')
        ops={o['operationId'] for methods in api['paths'].values() for o in methods.values()}
        require(sum(len(x) for x in api['paths'].values())==len(ops),'duplicate operationId')
        coverage=read_json(ROOT/'contracts/operation-coverage.json')['operations']
        require({x['operation_id'] for x in coverage}==ops,'API coverage mapping mismatch')
        testids={x['id'] for x in read_json(ROOT/'acceptance/test-cases.json')['cases']}
        for op in coverage:require(set(op['acceptance_tests'])<=testids and op['acceptance_tests'],'missing API acceptance')
        tools=read_json(ROOT/'contracts/mcp-tools.json')['tools']
        require(len(tools)<=14,'MCP surface exceeds frozen limit')
        require(len({t['name'] for t in tools})==len(tools),'duplicate tool')
        for t in tools:
            Draft202012Validator.check_schema(t['inputSchema'])
            require(set(t['x-operations'])<=ops,'unknown MCP target')
            require('recordApproval' not in t['x-operations'],'Agent cannot mint approval')
            for n in walk_values(t['inputSchema']):
                if '$ref' in n:
                    d=t['inputSchema']
                    for part in n['$ref'].lstrip('#/').split('/'):d=d[part]
        return {"OpenAPI_operations":len(ops),"MCP_tools":len(tools),"validation_scope":"Structural and reference checks; no HTTP server or full OpenAPI conformance suite run"}
    checked("OpenAPI/MCP structure and ownership", api_mcp)
    def state_machines():
        sm=read_json(ROOT/'contracts/state-machines.json')['changeset']
        states=set(sm['states']);pairs=set()
        for t in sm['transitions']:
            require(t['from'] in states and t['to'] in states,'unknown state')
            require((t['from'],t['event']) not in pairs,'ambiguous transition')
            pairs.add((t['from'],t['event']))
            require(t['from'] not in sm['terminal_history_immutable'],'mutating terminal history')
        return {"states":len(states),"transitions":len(sm['transitions'])}
    checked("State-machine consistency", state_machines)
    def manifest():
        path=ROOT/'PACK_MANIFEST.json'
        if not path.exists():
            return {"state":"not_created_yet","meaning":"Packaging integrity will be checked after manifest creation"}
        m=read_json(path);listed=set()
        for entry in m['files']:
            p=ROOT/entry['path'];listed.add(entry['path'])
            require(p.is_file(),f"missing {entry['path']}")
            data=p.read_bytes()
            require(len(data)==entry['size_bytes'] and digest_bytes(data)==entry['digest'],f"file modified {entry['path']}")
        actual={p.relative_to(ROOT).as_posix() for p in ROOT.rglob('*') if p.is_file() and p.name not in {'PACK_MANIFEST.json','SHA256SUMS'} and '__pycache__' not in p.parts}
        require(listed==actual,f"manifest set differs: {sorted(listed^actual)}")
        sums_path=ROOT/'SHA256SUMS'
        require(sums_path.is_file(), "SHA256SUMS missing")
        sums={}
        for line in sums_path.read_text(encoding='utf-8').splitlines():
            dig, name=line.split('  ',1)
            require(name not in sums,"duplicate checksum path")
            sums[name]=dig
            require(hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==dig, f"SHA256SUMS mismatch {name}")
        require(set(sums)==listed|{'PACK_MANIFEST.json'},"SHA256SUMS coverage mismatch")
        return {"verified_files":len(listed),"checksum_entries":len(sums)}
    checked("Package manifest integrity", manifest)
    failed=sum(r['status']=='failed' for r in results)
    report={'scope':'specification_pack_only','checked_at':datetime.now(timezone.utc).isoformat(),'status':'passed' if not failed else 'failed','check_groups':len(results),'passed_groups':len(results)-failed,'failed_groups':failed,'product_built':False,'upstream_built':False,'product_acceptance_tests_executed':0,'dependencies':{k:importlib.metadata.version(k) for k in ['jsonschema','referencing']},'checks':results}
    if args.output:
        out=args.output.resolve();out.parent.mkdir(parents=True,exist_ok=True)
        out.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k!='checks'},ensure_ascii=False,indent=2))
    return 1 if failed else 0

if __name__=='__main__':
    sys.exit(main())
