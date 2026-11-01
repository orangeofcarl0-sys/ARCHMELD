// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! ARCHMELD structural export v1.
//!
//! Unlike [`super::json::export_json`], this export iterates the graph's real
//! node/edge maps. Upstream IDs are monotonically allocated and become sparse
//! after deletions, so iterating `0..node_count` silently drops live objects
//! (see the ARCHMELD sparse-ID probe). This module is the export the bridge
//! consumes; backend IDs stay local and are mapped by the importer.

use crate::{CodeGraph, PropertyMap, PropertyValue};
use serde_json::{json, Value};

/// Export protocol tag. Consumers must reject other versions.
pub const FORMAT: &str = "archmeld-export-v1";

/// Stream-ready NDJSON: one manifest line, then one line per node and edge.
pub fn export_ndjson_v1(graph: &CodeGraph) -> crate::Result<String> {
    let nodes: Vec<(u64, &crate::Node)> = graph.iter_nodes().collect();
    let edges: Vec<(u64, &crate::Edge)> = graph.iter_edges().collect();

    let mut lines = Vec::with_capacity(nodes.len() + edges.len() + 1);
    lines.push(
        serde_json::to_string(&json!({
            "type": "manifest",
            "format": FORMAT,
            "source": "codegraph",
            "version": env!("CARGO_PKG_VERSION"),
            "node_count": nodes.len(),
            "edge_count": edges.len(),
            "iteration": "real-iterators",
        }))
        .map_err(|e| crate::GraphError::serialization("manifest", Some(e)))?,
    );

    for (id, node) in &nodes {
        lines.push(
            serde_json::to_string(&json!({
                "type": "node",
                "id": id,
                "node_type": format!("{:?}", node.node_type),
                "properties": properties_to_json(&node.properties),
            }))
            .map_err(|e| crate::GraphError::serialization("node", Some(e)))?,
        );
    }

    for (id, edge) in &edges {
        lines.push(
            serde_json::to_string(&json!({
                "type": "edge",
                "id": id,
                "source": edge.source_id,
                "target": edge.target_id,
                "edge_type": format!("{:?}", edge.edge_type),
                "properties": properties_to_json(&edge.properties),
            }))
            .map_err(|e| crate::GraphError::serialization("edge", Some(e)))?,
        );
    }

    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

fn properties_to_json(properties: &PropertyMap) -> Value {
    let mut object = serde_json::Map::new();
    for (key, value) in properties.iter() {
        let json_value = match value {
            PropertyValue::String(text) => json!(text),
            PropertyValue::Int(number) => json!(number),
            PropertyValue::Float(number) => json!(number),
            PropertyValue::Bool(flag) => json!(flag),
            PropertyValue::StringList(items) => json!(items),
            PropertyValue::IntList(items) => json!(items),
            PropertyValue::Null => Value::Null,
        };
        object.insert(key.clone(), json_value);
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeType, NodeType};

    #[test]
    fn export_includes_sparse_ids() {
        let mut graph = CodeGraph::in_memory().unwrap();
        let a = graph
            .add_node(NodeType::Function, PropertyMap::new().with("name", "a"))
            .unwrap();
        let b = graph
            .add_node(NodeType::Function, PropertyMap::new().with("name", "b"))
            .unwrap();
        let c = graph
            .add_node(NodeType::Function, PropertyMap::new().with("name", "c"))
            .unwrap();
        graph.delete_node(b).unwrap();
        let d = graph
            .add_node(NodeType::Function, PropertyMap::new().with("name", "d"))
            .unwrap();
        let _ = graph
            .add_edge(a, c, EdgeType::Calls, PropertyMap::new())
            .unwrap();
        let _ = graph
            .add_edge(c, d, EdgeType::References, PropertyMap::new())
            .unwrap();

        let text = export_ndjson_v1(&graph).unwrap();
        let mut node_ids = std::collections::BTreeSet::new();
        let mut edge_ids = std::collections::BTreeSet::new();
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            match value["type"].as_str().unwrap() {
                "manifest" => {
                    assert_eq!(value["format"], FORMAT);
                    assert_eq!(value["node_count"], 3);
                    assert_eq!(value["edge_count"], 2);
                }
                "node" => {
                    node_ids.insert(value["id"].as_u64().unwrap());
                }
                "edge" => {
                    edge_ids.insert(value["id"].as_u64().unwrap());
                }
                other => panic!("unexpected line type {other}"),
            }
        }
        assert_eq!(node_ids.into_iter().collect::<Vec<_>>(), vec![0, 2, 3]);
        assert_eq!(edge_ids.into_iter().collect::<Vec<_>>(), vec![0, 1]);
    }
}
