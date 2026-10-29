//! Boards: canvases of library files, notes and steps that people and agents edit together.
//!
//! A board is one JSON document (nodes, edges and the state of step runs) with a version number.
//! Every change is a list of operations applied in one write transaction and sent to every member
//! as a `board` event. An operation on an existing node or edge names the revision it was based on
//! (`base`); when that item changed since, the whole request is refused and the caller gets the
//! item's current state. Edits to different items never conflict.
//!
//! A step is a generation waiting to happen: a model, a prompt and settings. Edges into a step feed
//! it files (an asset node) or the newest results of another step. Running a step queues an
//! ordinary generation as the person who ran it, so budgets, the wallet and the queue apply as
//! usual; the results land on the board as asset nodes linked to the step. The server's board
//! watcher moves runs forward, so a run started by another process (an agent over stdio MCP) still
//! lands, and later steps of "run downstream" start when the steps they need have finished.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Mutex,
    time::Duration,
};

use pictesa_core::{now_ms, InputRole, MediaKind, ModelSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    files::Stored,
    store::{self, AssetRow, JobRow},
    Asset, CreateGeneration, Engine, EngineError, ImportMetadata, JobInput, JobStatus, Mode, Result,
};

const MAX_NODES: usize = 500;
const MAX_EDGES: usize = 1000;
const MAX_OPS: usize = 200;
const MAX_NOTE: usize = 10_000;
const MAX_PROMPT: usize = 20_000;
const MAX_STEP_COUNT: u32 = 8;
const MAX_COORD: f64 = 1_000_000.0;
const MIN_WIDTH: f64 = 80.0;
const MAX_WIDTH: f64 = 2000.0;
/// Room for the document of a full board; requests are limited to 2 MB anyway.
const MAX_DOCUMENT: usize = 4 * 1024 * 1024;
/// Width of the result nodes a step adds, and the gap between them.
const RESULT_WIDTH: f64 = 240.0;
const GAP: f64 = 24.0;
/// Viewers who have not checked in for this long have left.
const PRESENCE_MS: i64 = 45_000;
/// A step claimed for starting this long ago belongs to a process that stopped.
const STARTING_MS: i64 = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A library file.
    Asset,
    /// Free text.
    Note,
    /// A model, a prompt and settings that can run.
    Step,
}

/// One item on the canvas. Fields that do not apply to its kind stay empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    /// Width on the board; heights follow the content. 0 on a new node means the kind's default.
    #[serde(default)]
    pub w: f64,
    /// The board version of this node's last change.
    #[serde(default)]
    pub rev: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// A link between two nodes. Into a step it is an input: `role` picks the model's slot (none picks
/// the first free slot that takes the file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<InputRole>,
    #[serde(default)]
    pub rev: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Part of "run downstream": starts when the steps in `after` have succeeded.
    Waiting,
    /// Claimed by a process that is queuing its generation.
    Starting,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Waiting | Self::Starting | Self::Running)
    }
}

/// The latest run of a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRun {
    pub status: RunStatus,
    /// Who ran it. Its generations run, and are paid, as this user.
    pub by: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// Results of the step's last finished run; steps it feeds take the ones still on the board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// Steps of the same run that must succeed before this one starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: i64,
    #[serde(default)]
    pub rev: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Doc {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// The latest run of each step, by node id.
    #[serde(default)]
    pub runs: BTreeMap<String, StepRun>,
}

impl Doc {
    fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    fn edge(&self, id: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }
    fn is_step(&self, id: &str) -> bool {
        self.node(id).is_some_and(|n| n.kind == NodeKind::Step)
    }
}

/// One change. Operations on an existing item carry `base`, the item's `rev` the caller saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    AddNode {
        node: Node,
    },
    MoveNode {
        id: String,
        base: i64,
        x: f64,
        y: f64,
    },
    /// Sets the given fields; fields that are left out keep their value.
    UpdateNode {
        id: String,
        base: i64,
        #[serde(default)]
        w: Option<f64>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        negative_prompt: Option<String>,
        #[serde(default)]
        params: Option<Map<String, Value>>,
        #[serde(default)]
        count: Option<u32>,
    },
    DeleteNode {
        id: String,
        base: i64,
    },
    AddEdge {
        edge: Edge,
    },
    /// Sets the input role of an edge (null: the first free slot).
    UpdateEdge {
        id: String,
        base: i64,
        #[serde(default)]
        role: Option<InputRole>,
    },
    DeleteEdge {
        id: String,
        base: i64,
    },
    Rename {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardRole {
    Owner,
    Editor,
    Viewer,
}

impl BoardRole {
    pub fn can_edit(self) -> bool {
        self != Self::Viewer
    }
    fn parse(s: &str) -> Self {
        match s {
            "owner" => Self::Owner,
            "editor" => Self::Editor,
            _ => Self::Viewer,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Editor => "editor",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Person {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    #[serde(flatten)]
    pub person: Person,
    pub role: BoardRole,
    pub added_at: i64,
}

/// A whole board as one person sees it.
#[derive(Debug, Clone, Serialize)]
pub struct Board {
    pub id: String,
    pub name: String,
    /// The caller's role on it.
    pub role: BoardRole,
    pub owner: Person,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub runs: BTreeMap<String, StepRun>,
    /// The files of its asset nodes, by id.
    pub assets: HashMap<String, Asset>,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardSummary {
    pub id: String,
    pub name: String,
    pub role: BoardRole,
    pub owner: Person,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub node_count: usize,
    pub member_count: i64,
    /// Up to four of its newest files, for a cover.
    pub covers: Vec<Asset>,
}

/// What one change did, in the shape every member receives: the current state of each item it
/// touched, the items it removed, and the files of new asset nodes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Changes {
    pub board_id: String,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<String, StepRun>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed_nodes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed_edges: Vec<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub assets: HashMap<String, Asset>,
    /// Who made the change; empty for the server's own (results landing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Person>,
}

/// A refused request: the items it named changed since the caller saw them.
#[derive(Debug, Clone, Serialize)]
pub struct Stale {
    pub message: String,
    /// The current state of every item the request named.
    pub current: Changes,
}

/// A step run that was just started, and every step it will run.
#[derive(Debug, Clone, Serialize)]
pub struct Started {
    pub steps: Vec<String>,
    pub run: StepRun,
}

/// Live state that is not stored: who is looking at which board, and the newest version of each
/// board this process announced (a newer one in the database came from another process).
#[derive(Default)]
pub(crate) struct Live {
    versions: Mutex<HashMap<String, i64>>,
    /// Board id, then client id: the person and their last check-in.
    viewers: Mutex<HashMap<String, HashMap<String, (Person, i64)>>>,
}

// ---------- applying operations ----------

/// Why a change was not applied.
enum Refused {
    /// An item it names changed or disappeared since the caller saw it.
    Stale,
    Error(EngineError),
}

type Check = std::result::Result<(), Refused>;

/// The items a change touched; their current state is what members receive.
#[derive(Debug, Default)]
struct Touched {
    nodes: Vec<String>,
    edges: Vec<String>,
    runs: Vec<String>,
    removed_nodes: Vec<String>,
    removed_edges: Vec<String>,
    renamed: bool,
    new_assets: Vec<String>,
}

impl Touched {
    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty() && self.runs.is_empty() && self.removed_nodes.is_empty() && self.removed_edges.is_empty() && !self.renamed
    }
}

fn bad(message: impl Into<String>) -> Refused {
    Refused::Error(EngineError::invalid(message, None))
}

fn check_id(id: &str) -> Check {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(bad("Ids use 1 to 64 letters, digits, dashes or underscores."));
    }
    Ok(())
}

fn check_place(x: f64, y: f64) -> Check {
    if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_COORD && y.abs() <= MAX_COORD) {
        return Err(bad("Positions are numbers between -1,000,000 and 1,000,000."));
    }
    Ok(())
}

fn check_width(w: f64) -> Check {
    if !(w.is_finite() && (MIN_WIDTH..=MAX_WIDTH).contains(&w)) {
        return Err(bad(format!("Widths go from {MIN_WIDTH} to {MAX_WIDTH}.")));
    }
    Ok(())
}

fn check_text(what: &str, text: &str, max: usize) -> Check {
    if text.chars().count() > max {
        return Err(bad(format!("The {what} is longer than {max} characters.")));
    }
    Ok(())
}

fn check_step_fields(model: Option<&str>, prompt: Option<&str>, negative: Option<&str>, params: Option<&Map<String, Value>>, count: Option<u32>) -> Check {
    if model.is_some_and(|m| m.trim().is_empty() || m.len() > 300) {
        return Err(bad("Choose a model for the step."));
    }
    check_text("prompt", prompt.unwrap_or(""), MAX_PROMPT)?;
    check_text("negative prompt", negative.unwrap_or(""), MAX_PROMPT)?;
    if let Some(params) = params {
        if params.len() > 64 || serde_json::to_vec(params).map_or(true, |b| b.len() > 64 * 1024) {
            return Err(bad("Step settings must fit in 64 KB and 64 keys."));
        }
    }
    if count.is_some_and(|c| !(1..=MAX_STEP_COUNT).contains(&c)) {
        return Err(bad(format!("A step makes 1 to {MAX_STEP_COUNT} results.")));
    }
    Ok(())
}

fn default_width(kind: NodeKind) -> f64 {
    match kind {
        NodeKind::Asset | NodeKind::Note => 240.0,
        NodeKind::Step => 300.0,
    }
}

fn check_new_node(node: &mut Node) -> Check {
    check_id(&node.id)?;
    check_place(node.x, node.y)?;
    if node.w == 0.0 {
        node.w = default_width(node.kind);
    }
    check_width(node.w)?;
    let has_step_fields = node.model.is_some() || node.prompt.is_some() || node.negative_prompt.is_some() || !node.params.is_empty() || node.count.is_some();
    match node.kind {
        NodeKind::Asset => {
            if node.asset_id.as_deref().is_none_or(str::is_empty) {
                return Err(bad("An asset node needs an asset_id from the library."));
            }
            if node.text.is_some() || has_step_fields {
                return Err(bad("Asset nodes take only an asset_id."));
            }
        }
        NodeKind::Note => {
            if node.asset_id.is_some() || has_step_fields {
                return Err(bad("Note nodes take only text."));
            }
            check_text("note", node.text.get_or_insert_with(String::new), MAX_NOTE)?;
        }
        NodeKind::Step => {
            if node.asset_id.is_some() || node.text.is_some() {
                return Err(bad("Step nodes take a model, a prompt and settings."));
            }
            if node.model.is_none() {
                return Err(bad("Choose a model for the step."));
            }
            check_step_fields(node.model.as_deref(), node.prompt.as_deref(), node.negative_prompt.as_deref(), Some(&node.params), node.count)?;
            node.negative_prompt = node.negative_prompt.take().filter(|n| !n.trim().is_empty());
        }
    }
    Ok(())
}

/// The node `id` at revision `base`, or a stale refusal when it changed or was removed.
fn current_node<'a>(doc: &'a mut Doc, id: &str, base: i64) -> std::result::Result<&'a mut Node, Refused> {
    match doc.node_mut(id) {
        Some(node) if node.rev == base => Ok(node),
        _ => Err(Refused::Stale),
    }
}

fn current_edge<'a>(doc: &'a mut Doc, id: &str, base: i64) -> std::result::Result<&'a mut Edge, Refused> {
    match doc.edges.iter_mut().find(|e| e.id == id) {
        Some(edge) if edge.rev == base => Ok(edge),
        _ => Err(Refused::Stale),
    }
}

/// Only inputs of a step have a role: from a file or from another step.
fn role_for(doc: &Doc, from: &str, to: &str, role: Option<InputRole>) -> Option<InputRole> {
    let takes = doc.is_step(to) && doc.node(from).is_some_and(|n| n.kind != NodeKind::Note);
    role.filter(|_| takes)
}

/// True when `from` reaches `to` by following edges between steps.
fn steps_reach(doc: &Doc, from: &str, to: &str) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        stack.extend(doc.edges.iter().filter(|e| e.from == id && doc.is_step(&e.to)).map(|e| e.to.as_str()));
    }
    false
}

fn apply_op(doc: &mut Doc, name: &mut String, op: &Op, v: i64, t: &mut Touched) -> Check {
    match op {
        Op::AddNode { node } => {
            let mut node = node.clone();
            check_new_node(&mut node)?;
            if doc.node(&node.id).is_some() {
                return Err(bad("A node with this id is already on the board."));
            }
            if doc.nodes.len() >= MAX_NODES {
                return Err(bad(format!("A board holds up to {MAX_NODES} items.")));
            }
            node.rev = v;
            t.nodes.push(node.id.clone());
            t.new_assets.extend(node.asset_id.clone());
            doc.nodes.push(node);
        }
        Op::MoveNode { id, base, x, y } => {
            check_place(*x, *y)?;
            let node = current_node(doc, id, *base)?;
            (node.x, node.y, node.rev) = (*x, *y, v);
            t.nodes.push(id.clone());
        }
        Op::UpdateNode { id, base, w, text, model, prompt, negative_prompt, params, count } => {
            let node = current_node(doc, id, *base)?;
            let step_fields = model.is_some() || prompt.is_some() || negative_prompt.is_some() || params.is_some() || count.is_some();
            let fits = match node.kind {
                NodeKind::Asset => text.is_none() && !step_fields,
                NodeKind::Note => !step_fields,
                NodeKind::Step => text.is_none(),
            };
            if !fits {
                return Err(bad("That setting does not apply to this kind of node."));
            }
            if let Some(w) = w {
                check_width(*w)?;
                node.w = *w;
            }
            if let Some(text) = text {
                check_text("note", text, MAX_NOTE)?;
                node.text = Some(text.clone());
            }
            check_step_fields(model.as_deref(), prompt.as_deref(), negative_prompt.as_deref(), params.as_ref(), *count)?;
            if let Some(model) = model {
                node.model = Some(model.trim().to_string());
            }
            if let Some(prompt) = prompt {
                node.prompt = Some(prompt.clone());
            }
            if let Some(negative) = negative_prompt {
                node.negative_prompt = Some(negative.clone()).filter(|n| !n.trim().is_empty());
            }
            if let Some(params) = params {
                node.params = params.clone();
            }
            if let Some(count) = count {
                node.count = Some(*count);
            }
            node.rev = v;
            t.nodes.push(id.clone());
        }
        Op::DeleteNode { id, base } => {
            current_node(doc, id, *base)?;
            doc.nodes.retain(|n| n.id != *id);
            t.removed_edges.extend(doc.edges.iter().filter(|e| e.from == *id || e.to == *id).map(|e| e.id.clone()));
            doc.edges.retain(|e| e.from != *id && e.to != *id);
            doc.runs.remove(id);
            t.removed_nodes.push(id.clone());
        }
        Op::AddEdge { edge } => {
            check_id(&edge.id)?;
            if doc.edge(&edge.id).is_some() {
                return Err(bad("An edge with this id is already on the board."));
            }
            if edge.from == edge.to {
                return Err(bad("Connect two different items."));
            }
            // A missing end was removed by someone else: the caller's view is out of date.
            if doc.node(&edge.from).is_none() || doc.node(&edge.to).is_none() {
                return Err(Refused::Stale);
            }
            if doc.edges.iter().any(|e| e.from == edge.from && e.to == edge.to) {
                return Err(bad("These items are already connected."));
            }
            if doc.edges.len() >= MAX_EDGES {
                return Err(bad(format!("A board holds up to {MAX_EDGES} connections.")));
            }
            if doc.is_step(&edge.from) && doc.is_step(&edge.to) && steps_reach(doc, &edge.to, &edge.from) {
                return Err(bad("Steps cannot feed each other in a loop."));
            }
            let role = role_for(doc, &edge.from, &edge.to, edge.role);
            doc.edges.push(Edge { role, rev: v, ..edge.clone() });
            t.edges.push(edge.id.clone());
        }
        Op::UpdateEdge { id, base, role } => {
            let (from, to) = {
                let edge = current_edge(doc, id, *base)?;
                (edge.from.clone(), edge.to.clone())
            };
            let role = role_for(doc, &from, &to, *role);
            let edge = current_edge(doc, id, *base)?;
            (edge.role, edge.rev) = (role, v);
            t.edges.push(id.clone());
        }
        Op::DeleteEdge { id, base } => {
            current_edge(doc, id, *base)?;
            doc.edges.retain(|e| e.id != *id);
            t.removed_edges.push(id.clone());
        }
        Op::Rename { name: new } => {
            *name = board_name(new).map_err(|e| bad(e.to_string()))?;
            t.renamed = true;
        }
    }
    Ok(())
}

/// Applies operations in order to a document; every item they touch gets revision `version`.
/// One refused operation refuses them all.
fn apply_ops(doc: &mut Doc, name: &mut String, ops: &[Op], version: i64) -> std::result::Result<Touched, Refused> {
    let mut t = Touched::default();
    for (i, op) in ops.iter().enumerate() {
        // Problems name the operation they come from.
        apply_op(doc, name, op, version, &mut t).map_err(|refused| match refused {
            Refused::Error(EngineError::Validation { message, .. }) => Refused::Error(EngineError::invalid(message, Some(&format!("ops.{i}")))),
            other => other,
        })?;
    }
    Ok(t)
}

/// The nodes and edges a list of operations names (and the ends of new edges).
fn targets(ops: &[Op]) -> (Vec<String>, Vec<String>) {
    let (mut nodes, mut edges) = (Vec::new(), Vec::new());
    for op in ops {
        match op {
            Op::AddNode { node } => nodes.push(node.id.clone()),
            Op::MoveNode { id, .. } | Op::UpdateNode { id, .. } | Op::DeleteNode { id, .. } => nodes.push(id.clone()),
            Op::AddEdge { edge } => {
                edges.push(edge.id.clone());
                nodes.extend([edge.from.clone(), edge.to.clone()]);
            }
            Op::UpdateEdge { id, .. } | Op::DeleteEdge { id, .. } => edges.push(id.clone()),
            Op::Rename { .. } => {}
        }
    }
    (nodes, edges)
}

fn changes_of(doc: &Doc, board_id: &str, version: i64, t: &Touched, name: Option<String>) -> Changes {
    let mut c = Changes { board_id: board_id.to_string(), version, name, ..Default::default() };
    let mut seen = HashSet::new();
    for id in &t.nodes {
        if let Some(n) = doc.node(id).filter(|_| seen.insert(id.as_str())) {
            c.nodes.push(n.clone());
        }
    }
    for id in &t.edges {
        if let Some(e) = doc.edge(id).filter(|_| seen.insert(id.as_str())) {
            c.edges.push(e.clone());
        }
    }
    for id in &t.runs {
        if let Some(r) = doc.runs.get(id) {
            c.runs.insert(id.clone(), r.clone());
        }
    }
    c.removed_nodes = t.removed_nodes.iter().filter(|id| doc.node(id).is_none() && seen.insert(id.as_str())).cloned().collect();
    c.removed_edges = t.removed_edges.iter().filter(|id| doc.edge(id).is_none() && seen.insert(id.as_str())).cloned().collect();
    c
}

/// The current state of the named items, with the edges of the named nodes.
fn snapshot(doc: &Doc, board_id: &str, version: i64, nodes: &[String], edges: &[String]) -> Changes {
    let mut t = Touched::default();
    for id in nodes {
        if doc.node(id).is_some() {
            t.nodes.push(id.clone());
            t.runs.push(id.clone());
        } else {
            t.removed_nodes.push(id.clone());
        }
        t.edges.extend(doc.edges.iter().filter(|e| &e.from == id || &e.to == id).map(|e| e.id.clone()));
    }
    for id in edges {
        if doc.edge(id).is_some() {
            t.edges.push(id.clone());
        } else {
            t.removed_edges.push(id.clone());
        }
    }
    changes_of(doc, board_id, version, &t, None)
}

fn board_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(EngineError::invalid("Give the board a name of 1 to 100 characters.", Some("name")));
    }
    Ok(name.to_string())
}

// ---------- step inputs and run order ----------

/// One file offered to a step by an incoming edge.
#[derive(Debug, Clone)]
struct Feed {
    asset_id: String,
    kind: MediaKind,
    /// The edge's role; none takes the first slot with room for this kind (never a mask).
    role: Option<InputRole>,
    /// A result of another step. Results beyond a full slot are left out instead of refused,
    /// so a step that made four images can feed a model that takes one.
    from_step: bool,
}

fn role_name(role: InputRole) -> &'static str {
    match role {
        InputRole::Reference => "reference",
        InputRole::StartFrame => "start frame",
        InputRole::EndFrame => "end frame",
        InputRole::Video => "video",
        InputRole::Audio => "audio track",
        InputRole::Mask => "mask",
    }
}

fn kind_plural(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "images",
        MediaKind::Video => "videos",
        MediaKind::Audio => "audio files",
        MediaKind::Lora => "LoRA files",
    }
}

/// Turns a step's incoming files into the model's inputs, in edge order.
fn wire_inputs(spec: &ModelSpec, feeds: &[Feed]) -> std::result::Result<Vec<JobInput>, String> {
    let mut used: HashMap<InputRole, u32> = HashMap::new();
    let room = |role: InputRole, max: u32, used: &HashMap<InputRole, u32>| used.get(&role).copied().unwrap_or(0) < max;
    let mut out = Vec::new();
    for feed in feeds {
        let slot = match feed.role {
            Some(role) => {
                let slot = spec.accepts(role).ok_or_else(|| format!("{} does not take a {}.", spec.name, role_name(role)))?;
                if slot.kind != feed.kind {
                    return Err(format!("{} takes {} as its {}, not {}.", spec.name, kind_plural(slot.kind), role_name(role), kind_plural(feed.kind)));
                }
                slot
            }
            None => {
                let fitting: Vec<_> = spec.inputs.iter().filter(|s| s.kind == feed.kind && s.role != InputRole::Mask).collect();
                let first = *fitting.first().ok_or_else(|| format!("{} does not take {}.", spec.name, kind_plural(feed.kind)))?;
                fitting.iter().copied().find(|s| room(s.role, s.max, &used)).unwrap_or(first)
            }
        };
        if !room(slot.role, slot.max, &used) {
            if feed.from_step {
                continue;
            }
            let plural = if slot.max == 1 { "" } else { "s" };
            return Err(format!("{} takes at most {} {}{plural}. Remove a connection.", spec.name, slot.max, role_name(slot.role)));
        }
        *used.entry(slot.role).or_default() += 1;
        out.push(JobInput { asset_id: feed.asset_id.clone(), role: slot.role });
    }
    Ok(out)
}

/// The steps a run of `start` reaches through edges between steps, each after every step that
/// feeds it, with the steps of the run it waits for. `start` comes first and waits for nothing.
fn downstream_order(doc: &Doc, start: &str) -> Vec<(String, Vec<String>)> {
    let links: Vec<(&str, &str)> = doc.edges.iter().filter(|e| doc.is_step(&e.from) && doc.is_step(&e.to)).map(|e| (e.from.as_str(), e.to.as_str())).collect();
    let mut reach = vec![start];
    let mut i = 0;
    while i < reach.len() {
        let id = reach[i];
        for (_, to) in links.iter().filter(|(from, _)| *from == id) {
            if !reach.contains(to) {
                reach.push(*to);
            }
        }
        i += 1;
    }
    let feeders = |id: &str| -> Vec<String> { links.iter().filter(|(from, to)| *to == id && *from != id && reach.contains(from)).map(|(from, _)| from.to_string()).collect() };
    let mut waiting: HashMap<&str, usize> = reach.iter().map(|id| (*id, if *id == start { 0 } else { feeders(*id).len() })).collect();
    let position = |id: &str| doc.nodes.iter().position(|n| n.id == id).unwrap_or(usize::MAX);
    let mut ready = vec![start];
    let mut order = Vec::new();
    while !ready.is_empty() {
        ready.sort_by_key(|id| std::cmp::Reverse(position(*id)));
        let id = ready.pop().unwrap_or(start);
        order.push((id.to_string(), if id == start { Vec::new() } else { feeders(id) }));
        for (_, to) in links.iter().filter(|(from, _)| *from == id) {
            if let Some(n) = waiting.get_mut(*to).filter(|n| **n > 0) {
                *n -= 1;
                if *n == 0 {
                    ready.push(*to);
                }
            }
        }
    }
    order
}

/// Where a step's new results go: a column right of the step, or right of its earlier results.
fn place_results(doc: &mut Doc, step: &Node, results: &[Asset], v: i64, t: &mut Touched) {
    let earlier: Vec<&Node> = doc.edges.iter().filter(|e| e.from == step.id).filter_map(|e| doc.node(&e.to)).filter(|n| n.kind == NodeKind::Asset).collect();
    let x = earlier.iter().map(|n| n.x + n.w + GAP).fold(step.x + step.w + 2.5 * GAP, f64::max);
    let mut y = step.y;
    for asset in results {
        let ratio = match (asset.width, asset.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (h as f64 / w as f64).clamp(0.25, 3.0),
            _ => 1.0,
        };
        let node = Node {
            id: crate::new_id(),
            kind: NodeKind::Asset,
            x: x.min(MAX_COORD),
            y: y.min(MAX_COORD),
            w: RESULT_WIDTH,
            rev: v,
            asset_id: Some(asset.id.clone()),
            text: None,
            model: None,
            prompt: None,
            negative_prompt: None,
            params: Map::new(),
            count: None,
        };
        let edge = Edge { id: crate::new_id(), from: step.id.clone(), to: node.id.clone(), role: None, rev: v };
        y += RESULT_WIDTH * ratio + GAP;
        t.nodes.push(node.id.clone());
        t.edges.push(edge.id.clone());
        t.new_assets.push(asset.id.clone());
        doc.nodes.push(node);
        doc.edges.push(edge);
    }
}

// ---------- rows ----------

#[derive(Debug, Clone, sqlx::FromRow)]
struct BoardRow {
    id: String,
    owner_id: String,
    name: String,
    document: String,
    version: i64,
    created_at: i64,
    updated_at: i64,
}

const COLUMNS: &str = "b.id, b.owner_id, b.name, b.document, b.version, b.created_at, b.updated_at";

impl BoardRow {
    fn doc(&self) -> Result<Doc> {
        serde_json::from_str(&self.document).map_err(|e| EngineError::Internal(anyhow::anyhow!("board {} cannot be read: {e}", self.id)))
    }
}

#[derive(sqlx::FromRow)]
struct ListedRow {
    #[sqlx(flatten)]
    board: BoardRow,
    role: String,
    member_count: i64,
}

impl Engine {
    async fn board_row(&self, id: &str) -> Result<Option<BoardRow>> {
        Ok(sqlx::query_as(&format!("SELECT {COLUMNS} FROM boards b WHERE b.id = ?")).bind(id).fetch_optional(&self.inner.db).await?)
    }

    /// The board and the caller's role. Boards that were not shared with the caller do not exist for them.
    async fn board_access(&self, user: &str, id: &str) -> Result<(BoardRow, BoardRole)> {
        let row = self.board_row(id).await?.ok_or_else(|| EngineError::not_found("board"))?;
        if row.owner_id == user {
            return Ok((row, BoardRole::Owner));
        }
        let role: Option<String> = sqlx::query_scalar("SELECT role FROM board_members WHERE board_id = ? AND user_id = ?").bind(id).bind(user).fetch_optional(&self.inner.db).await?;
        match role {
            Some(role) => Ok((row, BoardRole::parse(&role))),
            None => Err(EngineError::not_found("board")),
        }
    }

    async fn editable_board(&self, user: &str, id: &str) -> Result<(BoardRow, BoardRole)> {
        let (row, role) = self.board_access(user, id).await?;
        if !role.can_edit() {
            return Err(EngineError::Forbidden("You can view this board but not change it.".into()));
        }
        Ok((row, role))
    }

    async fn person(&self, user: &str) -> Person {
        match self.user(user).await {
            Ok(u) => Person { id: u.id, name: u.name, email: u.email },
            Err(_) => Person { id: user.to_string(), name: None, email: None },
        }
    }

    async fn members(&self, board_id: &str) -> Result<Vec<Member>> {
        let rows: Vec<(String, Option<String>, Option<String>, String, i64)> = sqlx::query_as(
            "SELECT u.id, u.name, u.email, m.role, m.created_at FROM board_members m JOIN users u ON u.id = m.user_id WHERE m.board_id = ? ORDER BY m.created_at, u.id",
        )
        .bind(board_id)
        .fetch_all(&self.inner.db)
        .await?;
        Ok(rows.into_iter().map(|(id, name, email, role, added_at)| Member { person: Person { id, name, email }, role: BoardRole::parse(&role), added_at }).collect())
    }

    /// The owner and every member: everyone who receives the board's events.
    async fn audience(&self, board_id: &str, owner: &str) -> Vec<String> {
        let mut users = vec![owner.to_string()];
        let members: Vec<String> = sqlx::query_scalar("SELECT user_id FROM board_members WHERE board_id = ?").bind(board_id).fetch_all(&self.inner.db).await.unwrap_or_default();
        users.extend(members);
        users
    }

    fn tell(&self, users: &[String], kind: &'static str, data: &Value) {
        for user in users {
            self.emit(user, kind, data.clone());
        }
    }

    fn note_version(&self, board_id: &str, version: i64) {
        let mut versions = self.inner.boards.versions.lock().unwrap();
        let known = versions.entry(board_id.to_string()).or_insert(version);
        *known = (*known).max(version);
    }

    /// Asset rows by id, whoever owns them. Callers decide who may see them.
    async fn asset_rows(&self, ids: &[String]) -> Result<HashMap<String, AssetRow>> {
        let unique: Vec<&String> = ids.iter().collect::<HashSet<_>>().into_iter().collect();
        let mut out = HashMap::new();
        for chunk in unique.chunks(200) {
            let sql = format!("SELECT * FROM assets WHERE id IN ({})", vec!["?"; chunk.len()].join(", "));
            let mut query = sqlx::query_as::<_, AssetRow>(&sql);
            for id in chunk {
                query = query.bind(*id);
            }
            for row in query.fetch_all(&self.inner.db).await? {
                out.insert(row.id.clone(), row);
            }
        }
        Ok(out)
    }

    async fn asset_views(&self, ids: &[String]) -> Result<HashMap<String, Asset>> {
        Ok(self.asset_rows(ids).await?.into_iter().map(|(id, row)| (id, row.to_asset())).collect())
    }

    // ---------- reading and creating ----------

    pub async fn boards(&self, user: &str) -> Result<Vec<BoardSummary>> {
        let rows: Vec<ListedRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, CASE WHEN b.owner_id = ? THEN 'owner' ELSE m.role END AS role,
                (SELECT count(*) FROM board_members x WHERE x.board_id = b.id) AS member_count
             FROM boards b LEFT JOIN board_members m ON m.board_id = b.id AND m.user_id = ?
             WHERE b.owner_id = ? OR m.user_id IS NOT NULL ORDER BY b.updated_at DESC, b.id DESC"
        ))
        .bind(user)
        .bind(user)
        .bind(user)
        .fetch_all(&self.inner.db)
        .await?;
        let mut covers: Vec<(usize, Vec<String>)> = Vec::new();
        for row in &rows {
            let doc = row.board.doc()?;
            let newest: Vec<String> = doc.nodes.iter().rev().filter_map(|n| n.asset_id.clone()).take(4).collect();
            covers.push((doc.nodes.len(), newest));
        }
        let all: Vec<String> = covers.iter().flat_map(|(_, ids)| ids.iter().cloned()).collect();
        let assets = self.asset_views(&all).await?;
        let mut owners: HashMap<String, Person> = HashMap::new();
        let mut out = Vec::new();
        for (row, (node_count, ids)) in rows.into_iter().zip(covers) {
            let owner = match owners.get(&row.board.owner_id) {
                Some(p) => p.clone(),
                None => {
                    let p = self.person(&row.board.owner_id).await;
                    owners.insert(p.id.clone(), p.clone());
                    p
                }
            };
            let b = row.board;
            out.push(BoardSummary {
                covers: ids.iter().filter_map(|id| assets.get(id)).filter(|a| a.deleted_at.is_none()).cloned().collect(),
                id: b.id,
                name: b.name,
                role: BoardRole::parse(&row.role),
                owner,
                version: b.version,
                created_at: b.created_at,
                updated_at: b.updated_at,
                node_count,
                member_count: row.member_count,
            });
        }
        Ok(out)
    }

    pub async fn board(&self, user: &str, id: &str) -> Result<Board> {
        let (row, role) = self.board_access(user, id).await?;
        let doc = row.doc()?;
        let ids: Vec<String> = doc.nodes.iter().filter_map(|n| n.asset_id.clone()).collect();
        Ok(Board {
            assets: self.asset_views(&ids).await?,
            members: self.members(&row.id).await?,
            owner: self.person(&row.owner_id).await,
            id: row.id,
            name: row.name,
            role,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
            nodes: doc.nodes,
            edges: doc.edges,
            runs: doc.runs,
        })
    }

    pub async fn create_board(&self, user: &str, name: &str) -> Result<Board> {
        let name = board_name(name)?;
        let id = crate::new_id();
        let now = now_ms();
        let doc = serde_json::to_string(&Doc::default()).map_err(anyhow::Error::from)?;
        sqlx::query("INSERT INTO boards (id, owner_id, name, document, version, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
            .bind(&id)
            .bind(user)
            .bind(&name)
            .bind(doc)
            .bind(now)
            .bind(now)
            .execute(&self.inner.db)
            .await?;
        self.note_version(&id, 0);
        self.emit(user, "boards", json!({ "board_id": id }));
        self.board(user, &id).await
    }

    pub async fn delete_board(&self, user: &str, id: &str) -> Result<()> {
        let (row, role) = self.board_access(user, id).await?;
        if role != BoardRole::Owner {
            return Err(EngineError::Forbidden("Only the owner can delete this board.".into()));
        }
        let audience = self.audience(id, &row.owner_id).await;
        sqlx::query("DELETE FROM boards WHERE id = ? AND owner_id = ?").bind(id).bind(user).execute(&self.inner.db).await?;
        self.inner.boards.viewers.lock().unwrap().remove(id);
        self.tell(&audience, "boards", &json!({ "board_id": id, "deleted": true }));
        Ok(())
    }

    // ---------- changing ----------

    /// Applies `change` to the board's document in one write transaction, saves it and tells every
    /// member. `named` are the items the change refers to: when it is refused as stale, their
    /// current state is returned instead.
    async fn change_board<F>(&self, id: &str, by: Option<Person>, named: (&[String], &[String]), change: F) -> Result<std::result::Result<Option<Changes>, Stale>>
    where
        F: FnOnce(&mut Doc, &mut String, i64) -> std::result::Result<Touched, Refused>,
    {
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        let row: BoardRow = sqlx::query_as(&format!("SELECT {COLUMNS} FROM boards b WHERE b.id = ?"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| EngineError::not_found("board"))?;
        let mut doc = row.doc()?;
        let mut name = row.name.clone();
        let version = row.version + 1;
        let touched = match change(&mut doc, &mut name, version) {
            Ok(t) if t.is_empty() => return Ok(Ok(None)),
            Ok(t) => t,
            Err(Refused::Error(e)) => return Err(e),
            Err(Refused::Stale) => {
                let current = snapshot(&row.doc()?, id, row.version, named.0, named.1);
                return Ok(Err(Stale { message: "Someone changed this at the same time. It now shows their version.".into(), current }));
            }
        };
        let text = serde_json::to_string(&doc).map_err(anyhow::Error::from)?;
        if text.len() > MAX_DOCUMENT {
            return Err(EngineError::invalid("This board is full. Remove some items first.", Some("ops")));
        }
        let active = doc.runs.values().filter(|r| r.status.is_active()).count() as i64;
        sqlx::query("UPDATE boards SET name = ?, document = ?, version = ?, active_runs = ?, updated_at = ? WHERE id = ?")
            .bind(&name)
            .bind(&text)
            .bind(version)
            .bind(active)
            .bind(now_ms())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.note_version(id, version);
        let mut changes = changes_of(&doc, id, version, &touched, touched.renamed.then_some(name));
        changes.assets = self.asset_views(&touched.new_assets).await?;
        changes.by = by;
        let audience = self.audience(id, &row.owner_id).await;
        self.tell(&audience, "board", &json!(changes));
        if touched.renamed {
            self.tell(&audience, "boards", &json!({ "board_id": id }));
        }
        Ok(Ok(Some(changes)))
    }

    /// A change the server makes to runs and results. It names no items, so it is never stale.
    async fn change_runs<F>(&self, id: &str, change: F) -> Result<Option<Changes>>
    where
        F: FnOnce(&mut Doc, i64) -> Result<Touched>,
    {
        self.change_board(id, None, (&[], &[]), |doc, _, v| change(doc, v).map_err(Refused::Error)).await?.map_err(|stale| EngineError::Conflict(stale.message))
    }

    /// Applies a person's or an agent's operations. `Err(Stale)` means nothing was applied because
    /// an item they name changed since the caller saw it.
    pub async fn apply_board_ops(&self, user: &str, id: &str, ops: Vec<Op>) -> Result<std::result::Result<Changes, Stale>> {
        if ops.is_empty() || ops.len() > MAX_OPS {
            return Err(EngineError::invalid(format!("Send 1 to {MAX_OPS} operations."), Some("ops")));
        }
        let (row, _) = self.editable_board(user, id).await?;
        // New asset nodes show the caller's own files, or files already on this board. Most
        // requests (moves, text) add none, and skip reading the document here.
        let added: Vec<(usize, &str)> = ops.iter().enumerate().filter_map(|(i, op)| match op {
            Op::AddNode { node } => node.asset_id.as_deref().map(|a| (i, a)),
            _ => None,
        }).collect();
        let doc = if added.is_empty() { Doc::default() } else { row.doc()? };
        let on_board: HashSet<&str> = doc.nodes.iter().filter_map(|n| n.asset_id.as_deref()).collect();
        for (i, asset) in added.into_iter().filter(|(_, a)| !on_board.contains(a)) {
            let field = format!("ops.{i}");
            let row = store::asset_row(&self.inner.db, user, asset).await.map_err(|_| EngineError::invalid("That file is not in your library.", Some(&field)))?;
            if row.deleted_at.is_some() {
                return Err(EngineError::invalid("That file is in the trash.", Some(&field)));
            }
        }
        let (nodes, edges) = targets(&ops);
        let by = self.person(user).await;
        let done = self.change_board(id, Some(by), (&nodes, &edges), |doc, name, v| apply_ops(doc, name, &ops, v)).await?;
        Ok(done.map(|c| c.unwrap_or_else(|| Changes { board_id: id.to_string(), version: row.version, ..Default::default() })))
    }

    pub async fn rename_board(&self, user: &str, id: &str, name: &str) -> Result<Changes> {
        match self.apply_board_ops(user, id, vec![Op::Rename { name: name.to_string() }]).await? {
            Ok(changes) => Ok(changes),
            Err(stale) => Err(EngineError::Conflict(stale.message)),
        }
    }

    // ---------- sharing and presence ----------

    /// Shares the board with an account, or changes its role (hosted mode, owner only).
    pub async fn share_board(&self, user: &str, id: &str, email: &str, role: BoardRole) -> Result<Vec<Member>> {
        let (row, mine) = self.board_access(user, id).await?;
        if mine != BoardRole::Owner {
            return Err(EngineError::Forbidden("Only the owner can share this board.".into()));
        }
        if self.mode() != Mode::Hosted {
            return Err(EngineError::invalid("Sharing works in hosted mode, where people have accounts.", Some("email")));
        }
        if role == BoardRole::Owner {
            return Err(EngineError::invalid("Share as an editor or a viewer.", Some("role")));
        }
        let email = email.trim().to_lowercase();
        let target: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE email = ?").bind(&email).fetch_optional(&self.inner.db).await?;
        let Some(target) = target else {
            // The owner needs to know, but guessing emails is slowed to 10 misses an hour per person.
            let now = now_ms();
            let mut misses = self.inner.share_misses.lock().unwrap();
            let entry = misses.entry(user.to_string()).or_insert((now, 0));
            if now - entry.0 > 3_600_000 {
                *entry = (now, 0);
            }
            entry.1 += 1;
            if entry.1 > 10 {
                return Err(EngineError::TooMany("Too many emails without an account. Try again in an hour.".into()));
            }
            return Err(EngineError::invalid("No account uses that email.", Some("email")));
        };
        if target == row.owner_id {
            return Err(EngineError::invalid("You own this board.", Some("email")));
        }
        sqlx::query("INSERT INTO board_members (board_id, user_id, role, created_at) VALUES (?, ?, ?, ?) ON CONFLICT (board_id, user_id) DO UPDATE SET role = excluded.role")
            .bind(id)
            .bind(&target)
            .bind(role.as_str())
            .bind(now_ms())
            .execute(&self.inner.db)
            .await?;
        let audience = self.audience(id, &row.owner_id).await;
        self.tell(&audience, "boards", &json!({ "board_id": id }));
        self.members(id).await
    }

    /// Removes a member. The owner removes anyone; a member can leave.
    pub async fn remove_board_member(&self, user: &str, id: &str, member: &str) -> Result<()> {
        let (row, mine) = self.board_access(user, id).await?;
        if mine != BoardRole::Owner && member != user {
            return Err(EngineError::Forbidden("Only the owner can remove people.".into()));
        }
        let audience = self.audience(id, &row.owner_id).await;
        let done = sqlx::query("DELETE FROM board_members WHERE board_id = ? AND user_id = ?").bind(id).bind(member).execute(&self.inner.db).await?;
        if done.rows_affected() == 0 {
            return Err(EngineError::not_found("member"));
        }
        if let Some(viewers) = self.inner.boards.viewers.lock().unwrap().get_mut(id) {
            viewers.retain(|_, (p, _)| p.id != member);
        }
        self.tell(&audience, "boards", &json!({ "board_id": id, "removed": member }));
        Ok(())
    }

    /// Records that a client has the board open (or closed it) and returns who is looking at it.
    /// Members hear about every change to that list.
    pub async fn board_presence(&self, user: &str, id: &str, client: &str, leave: bool) -> Result<Vec<Person>> {
        let (row, _) = self.board_access(user, id).await?;
        if client.is_empty() || client.len() > 64 {
            return Err(EngineError::invalid("Send a client id of 1 to 64 characters.", Some("client")));
        }
        let person = self.person(user).await;
        let now = now_ms();
        let unique = |viewers: &HashMap<String, (Person, i64)>| {
            let mut people: Vec<Person> = Vec::new();
            for (p, _) in viewers.values() {
                if !people.iter().any(|q| q.id == p.id) {
                    people.push(p.clone());
                }
            }
            people.sort_by(|a, b| a.id.cmp(&b.id));
            people
        };
        let (before, after) = {
            let mut all = self.inner.boards.viewers.lock().unwrap();
            let viewers = all.entry(id.to_string()).or_default();
            let before = unique(viewers);
            viewers.retain(|_, (_, seen)| now - *seen < PRESENCE_MS);
            if leave {
                viewers.remove(client);
            } else {
                viewers.insert(client.to_string(), (person, now));
            }
            (before, unique(viewers))
        };
        if before != after {
            let audience = self.audience(id, &row.owner_id).await;
            self.tell(&audience, "board_presence", &json!({ "board_id": id, "viewers": after }));
        }
        Ok(after)
    }

    // ---------- files ----------

    /// An asset a user may see: their own, or one shown on a board they can open (unless its owner
    /// put it in the trash).
    pub(crate) async fn visible_asset_row(&self, user: &str, id: &str) -> Result<AssetRow> {
        if let Ok(row) = store::asset_row(&self.inner.db, user, id).await {
            return Ok(row);
        }
        let row: Option<AssetRow> = sqlx::query_as(
            "SELECT a.* FROM assets a WHERE a.id = ? AND a.deleted_at IS NULL AND EXISTS (
                SELECT 1 FROM boards b
                WHERE (b.owner_id = ? OR EXISTS (SELECT 1 FROM board_members m WHERE m.board_id = b.id AND m.user_id = ?))
                  AND EXISTS (SELECT 1 FROM json_each(b.document, '$.nodes') n WHERE json_extract(n.value, '$.asset_id') = a.id))",
        )
        .bind(id)
        .bind(user)
        .bind(user)
        .fetch_optional(&self.inner.db)
        .await?;
        row.ok_or_else(|| EngineError::not_found("file"))
    }

    /// The caller's own library item for a file: the file itself when it is theirs, else their
    /// copy of the same content (made now if needed; the bytes are stored once).
    async fn own_copy(&self, user: &str, row: &AssetRow) -> Result<Asset> {
        if row.user_id == user {
            return Ok(row.to_asset());
        }
        if row.deleted_at.is_some() {
            return Err(EngineError::invalid("A file on this board is in its owner's trash.", Some("inputs")));
        }
        let stored = Stored {
            hash: row.hash.clone(),
            ext: row.ext.clone(),
            mime: row.mime.clone(),
            kind: store::parse_kind(&row.kind),
            size: row.size.max(0) as u64,
            width: row.width.map(|w| w as u32),
            height: row.height.map(|h| h as u32),
            duration_ms: row.duration_ms.map(|d| d.max(0) as u64),
            has_thumb: row.has_thumb,
        };
        let copy = self.insert_named_asset(user, &stored, "import", None, None, row.name.as_deref(), None).await?;
        let meta = ImportMetadata { prompt: row.prompt.clone(), model: row.model.clone(), model_label: row.model_label.clone(), origin: Some("board".into()), ..Default::default() };
        self.fill_asset_metadata(user, &copy.id, &meta).await
    }

    /// Copies files shown on a board into the caller's library (their own files come back as they
    /// are), so they can be used anywhere, like references in Create.
    pub async fn copy_board_assets(&self, user: &str, id: &str, asset_ids: &[String]) -> Result<Vec<Asset>> {
        if asset_ids.is_empty() || asset_ids.len() > 100 {
            return Err(EngineError::invalid("Choose 1 to 100 files.", Some("asset_ids")));
        }
        let (row, _) = self.board_access(user, id).await?;
        let doc = row.doc()?;
        if let Some(missing) = asset_ids.iter().find(|a| !doc.nodes.iter().any(|n| n.asset_id.as_ref() == Some(*a))) {
            return Err(EngineError::NotFound(format!("file {missing} is not on this board")));
        }
        let rows = self.asset_rows(asset_ids).await?;
        let mut out = Vec::new();
        for asset in asset_ids {
            let row = rows.get(asset).ok_or_else(|| EngineError::not_found("file"))?;
            out.push(self.own_copy(user, row).await?);
        }
        Ok(out)
    }

    // ---------- running steps ----------

    /// The generation a step asks for: its model, prompt and settings, with the files of its
    /// incoming edges as inputs. Files of other members join the runner's library first.
    async fn step_request(&self, user: &str, doc: &Doc, node: &Node) -> Result<CreateGeneration> {
        let model = node.model.clone().filter(|m| !m.trim().is_empty()).ok_or_else(|| EngineError::invalid("Choose a model for this step.", Some("model")))?;
        let spec = self.model(user, &model).await?;
        let on_board: HashSet<&str> = doc.nodes.iter().filter_map(|n| n.asset_id.as_deref()).collect();
        let mut wanted: Vec<(String, Option<InputRole>, bool)> = Vec::new();
        for edge in doc.edges.iter().filter(|e| e.to == node.id) {
            let Some(source) = doc.node(&edge.from) else { continue };
            match source.kind {
                NodeKind::Asset => wanted.extend(source.asset_id.clone().map(|a| (a, edge.role, false))),
                NodeKind::Step => {
                    let outputs = doc.runs.get(&source.id).map(|r| r.outputs.as_slice()).unwrap_or_default();
                    wanted.extend(outputs.iter().filter(|a| on_board.contains(a.as_str())).map(|a| (a.clone(), edge.role, true)));
                }
                NodeKind::Note => {}
            }
        }
        let ids: Vec<String> = wanted.iter().map(|w| w.0.clone()).collect();
        let rows = self.asset_rows(&ids).await?;
        let mut feeds = Vec::new();
        for (asset_id, role, from_step) in wanted {
            let row = rows.get(&asset_id).ok_or_else(|| EngineError::invalid("A file connected to this step was deleted.", Some("inputs")))?;
            feeds.push(Feed { asset_id, kind: store::parse_kind(&row.kind), role, from_step });
        }
        let mut inputs = wire_inputs(&spec, &feeds).map_err(|m| EngineError::invalid(m, Some("inputs")))?;
        for input in &mut inputs {
            if let Some(row) = rows.get(&input.asset_id).filter(|r| r.user_id != user) {
                input.asset_id = self.own_copy(user, row).await?.id;
            }
        }
        // Settings of a model the step used before do not carry over to the current one.
        let passthrough = spec.meta.get("passthrough").and_then(Value::as_bool) == Some(true);
        let params = node.params.iter().filter(|(k, _)| passthrough || spec.param_spec(k).is_some()).map(|(k, v)| (k.clone(), v.clone())).collect();
        Ok(CreateGeneration {
            model: spec.id.clone(),
            prompt: node.prompt.clone().unwrap_or_default(),
            negative_prompt: node.negative_prompt.clone(),
            params,
            inputs,
            count: Some(node.count.unwrap_or(1)),
            ..Default::default()
        })
    }

    /// Runs a step, and with `downstream` every step that takes its results after it, in order.
    /// The first step is queued before this returns, so its problems (a missing input, a budget)
    /// come back at once.
    pub async fn run_board_step(&self, user: &str, id: &str, node_id: &str, downstream: bool) -> Result<Started> {
        let (row, _) = self.editable_board(user, id).await?;
        let doc = row.doc()?;
        if !doc.is_step(node_id) {
            return Err(EngineError::not_found("step"));
        }
        let plan = if downstream { downstream_order(&doc, node_id) } else { vec![(node_id.to_string(), Vec::new())] };
        // Claim every step of the plan in one change, so a double click cannot queue a step twice.
        let now = now_ms();
        self.change_runs(id, |doc, v| {
            let mut t = Touched::default();
            for (step, after) in &plan {
                if !doc.is_step(step) {
                    continue;
                }
                let previous = doc.runs.get(step);
                if previous.is_some_and(|r| r.status.is_active()) {
                    let message = if step == node_id { "This step is already running." } else { "A step after this one is already running." };
                    return Err(EngineError::Conflict(message.into()));
                }
                let status = if step == node_id { RunStatus::Starting } else { RunStatus::Waiting };
                let outputs = previous.map(|r| r.outputs.clone()).unwrap_or_default();
                doc.runs.insert(step.clone(), StepRun { status, by: user.to_string(), jobs: Vec::new(), outputs, after: after.clone(), error: None, updated_at: now, rev: v });
                t.runs.push(step.clone());
            }
            Ok(t)
        })
        .await?;
        let started = self.start_step(id, node_id).await;
        if started.is_err() {
            // Later steps of this run cannot start now; settle them.
            let _ = self.advance_board(id).await;
        }
        started?;
        let doc = self.board_row(id).await?.ok_or_else(|| EngineError::not_found("board"))?.doc()?;
        let run = doc.runs.get(node_id).cloned().ok_or_else(|| EngineError::not_found("step"))?;
        Ok(Started { steps: plan.into_iter().map(|(s, _)| s).collect(), run })
    }

    /// Queues the generation of a step claimed as starting, as the person who ran it.
    async fn start_step(&self, id: &str, node_id: &str) -> Result<()> {
        let Some(row) = self.board_row(id).await? else { return Ok(()) };
        let doc = row.doc()?;
        let Some(run) = doc.runs.get(node_id).filter(|r| r.status == RunStatus::Starting).cloned() else { return Ok(()) };
        let created = match doc.node(node_id) {
            Some(node) => match self.step_request(&run.by, &doc, node).await {
                Ok(request) => self.create_generation(&run.by, request).await,
                Err(e) => Err(e),
            },
            None => Err(EngineError::not_found("step")),
        };
        let jobs: Vec<String> = created.as_ref().map(|c| c.jobs.iter().map(|j| j.id.clone()).collect()).unwrap_or_default();
        let error = created.as_ref().err().map(ToString::to_string);
        let mut orphaned = false;
        self.change_runs(id, |doc, v| {
            let mut t = Touched::default();
            match doc.runs.get_mut(node_id).filter(|r| r.status == RunStatus::Starting) {
                Some(r) => {
                    match &error {
                        Some(message) => (r.status, r.error) = (RunStatus::Failed, Some(message.clone())),
                        None => (r.status, r.jobs) = (RunStatus::Running, jobs.clone()),
                    }
                    (r.updated_at, r.rev) = (now_ms(), v);
                    t.runs.push(node_id.to_string());
                }
                // Canceled or removed while its generation was being queued.
                None => orphaned = true,
            }
            Ok(t)
        })
        .await?;
        if orphaned {
            for job in &jobs {
                let _ = self.cancel(&run.by, job).await;
            }
        }
        created.map(|_| ())
    }

    /// Stops a step's run and every later step waiting for it.
    pub async fn cancel_board_step(&self, user: &str, id: &str, node_id: &str) -> Result<StepRun> {
        let (row, _) = self.editable_board(user, id).await?;
        let run = row.doc()?.runs.get(node_id).cloned().ok_or_else(|| EngineError::not_found("run"))?;
        if run.status.is_active() {
            self.change_runs(id, |doc, v| {
                let mut t = Touched::default();
                if let Some(r) = doc.runs.get_mut(node_id).filter(|r| r.status.is_active()) {
                    (r.status, r.error, r.updated_at, r.rev) = (RunStatus::Canceled, None, now_ms(), v);
                    t.runs.push(node_id.to_string());
                }
                Ok(t)
            })
            .await?;
            for job in &run.jobs {
                let _ = self.cancel(&run.by, job).await;
            }
            self.advance_board(id).await?;
        }
        let doc = self.board_row(id).await?.ok_or_else(|| EngineError::not_found("board"))?.doc()?;
        doc.runs.get(node_id).cloned().ok_or_else(|| EngineError::not_found("run"))
    }

    /// Moves the board's runs forward: lands the results of finished generations, starts waiting
    /// steps whose earlier steps succeeded and settles those that can no longer start. Safe to call
    /// from several processes at once: every move is checked again inside its write.
    pub async fn advance_board(&self, id: &str) -> Result<()> {
        let Some(row) = self.board_row(id).await? else { return Ok(()) };
        for (node_id, run) in row.doc()?.runs.into_iter().filter(|(_, r)| r.status == RunStatus::Running) {
            let mut jobs = Vec::new();
            for job in &run.jobs {
                if let Ok(row) = store::job_row_any(&self.inner.db, job).await {
                    jobs.push(row);
                }
            }
            if jobs.iter().all(|j| j.status().is_final()) {
                self.land_step(id, &node_id, &run, &jobs).await?;
            }
        }
        // Settling one waiting step can settle the ones waiting for it, so repeat a few times.
        for _ in 0..8 {
            let Some(row) = self.board_row(id).await? else { return Ok(()) };
            let doc = row.doc()?;
            let now = now_ms();
            let mut settle: Vec<(String, RunStatus, &str)> = Vec::new();
            let mut ready = Vec::new();
            for (node_id, run) in &doc.runs {
                match run.status {
                    RunStatus::Starting if now - run.updated_at > STARTING_MS => settle.push((node_id.clone(), RunStatus::Failed, "Pictesa stopped before this step started.")),
                    RunStatus::Waiting => {
                        let before: Vec<Option<RunStatus>> = run.after.iter().map(|s| doc.runs.get(s).map(|r| r.status)).collect();
                        if before.iter().all(|s| *s == Some(RunStatus::Succeeded)) {
                            ready.push(node_id.clone());
                        } else if before.contains(&Some(RunStatus::Canceled)) {
                            settle.push((node_id.clone(), RunStatus::Canceled, "An earlier step was canceled."));
                        } else if before.iter().any(|s| s.is_none_or(|s| matches!(s, RunStatus::Failed))) {
                            settle.push((node_id.clone(), RunStatus::Failed, "An earlier step did not finish."));
                        }
                    }
                    _ => {}
                }
            }
            if settle.is_empty() && ready.is_empty() {
                break;
            }
            if !settle.is_empty() {
                self.change_runs(id, |doc, v| {
                    let mut t = Touched::default();
                    for (node_id, status, message) in &settle {
                        if let Some(r) = doc.runs.get_mut(node_id).filter(|r| matches!(r.status, RunStatus::Waiting | RunStatus::Starting)) {
                            (r.status, r.error, r.updated_at, r.rev) = (*status, Some(message.to_string()), now_ms(), v);
                            t.runs.push(node_id.clone());
                        }
                    }
                    Ok(t)
                })
                .await?;
            }
            for node_id in ready {
                // Only the process whose claim lands queues the step.
                let claimed = self
                    .change_runs(id, |doc, v| {
                        let mut t = Touched::default();
                        if let Some(r) = doc.runs.get_mut(&node_id).filter(|r| r.status == RunStatus::Waiting) {
                            (r.status, r.updated_at, r.rev) = (RunStatus::Starting, now_ms(), v);
                            t.runs.push(node_id.clone());
                        }
                        Ok(t)
                    })
                    .await?;
                if claimed.is_some() {
                    if let Err(e) = self.start_step(id, &node_id).await {
                        tracing::debug!("board {id}: step {node_id} did not start: {e}");
                    }
                }
            }
        }
        Ok(())
    }

    /// Puts a finished run's results on the board, linked to the step, and records how it ended.
    async fn land_step(&self, id: &str, node_id: &str, run: &StepRun, jobs: &[JobRow]) -> Result<()> {
        let mut results = Vec::new();
        for job in jobs {
            results.extend(store::outputs_of(&self.inner.db, &job.id).await?);
        }
        let failed = jobs.iter().find(|j| j.status() == JobStatus::Failed);
        let status = match failed {
            Some(_) => RunStatus::Failed,
            None if jobs.iter().any(|j| j.status() == JobStatus::Canceled) => RunStatus::Canceled,
            None => RunStatus::Succeeded,
        };
        let mut error = failed.map(|j| j.error.clone().unwrap_or_else(|| "The generation failed.".into()));
        self.change_runs(id, |doc, v| {
            let mut t = Touched::default();
            // Another process may have landed it already.
            if !doc.runs.get(node_id).is_some_and(|r| r.status == RunStatus::Running && r.jobs == run.jobs) {
                return Ok(t);
            }
            if let Some(step) = doc.node(node_id).cloned() {
                if doc.nodes.len() + results.len() <= MAX_NODES && doc.edges.len() + results.len() <= MAX_EDGES {
                    place_results(doc, &step, &results, v, &mut t);
                } else {
                    error = Some("The board is full, so the results are only in the library.".into());
                }
            }
            if let Some(r) = doc.runs.get_mut(node_id) {
                if !results.is_empty() {
                    r.outputs = results.iter().map(|a| a.id.clone()).collect();
                }
                (r.status, r.error, r.updated_at, r.rev) = (status, error.clone(), now_ms(), v);
                t.runs.push(node_id.to_string());
            }
            Ok(t)
        })
        .await?;
        Ok(())
    }

    /// The server's board watcher: moves runs forward when generations finish (and every two
    /// seconds, for work done by other processes), and tells members about board changes another
    /// process wrote to the database.
    pub(crate) async fn watch_boards(self) {
        let mut events = self.subscribe();
        let known: Vec<(String, i64)> = sqlx::query_as("SELECT id, version FROM boards").fetch_all(&self.inner.db).await.unwrap_or_default();
        for (id, version) in known {
            self.note_version(&id, version);
        }
        let mut since = now_ms();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(e) if e.kind == "job" && e.data.get("status").and_then(Value::as_str).is_some_and(|s| matches!(s, "succeeded" | "failed" | "canceled")) => self.advance_boards().await,
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => self.advance_boards().await,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    self.advance_boards().await;
                    since = self.announce_outside_changes(since).await;
                }
            }
        }
    }

    async fn advance_boards(&self) {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM boards WHERE active_runs > 0").fetch_all(&self.inner.db).await.unwrap_or_default();
        for id in ids {
            if let Err(e) = self.advance_board(&id).await {
                tracing::warn!("board {id}: {e}");
            }
        }
    }

    /// Boards changed by another process (an agent over stdio MCP) since `since`: members reload them.
    async fn announce_outside_changes(&self, since: i64) -> i64 {
        let now = now_ms();
        let rows: Vec<(String, String, i64)> = sqlx::query_as("SELECT id, owner_id, version FROM boards WHERE updated_at >= ?").bind(since - 5_000).fetch_all(&self.inner.db).await.unwrap_or_default();
        for (id, owner, version) in rows {
            let known = self.inner.boards.versions.lock().unwrap().get(&id).copied();
            if known.is_some_and(|k| k >= version) {
                continue;
            }
            self.note_version(&id, version);
            let audience = self.audience(&id, &owner).await;
            self.tell(&audience, "board", &json!({ "board_id": id, "version": version, "reload": true }));
            if known.is_none() {
                self.tell(&audience, "boards", &json!({ "board_id": id }));
            }
        }
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::{InputSlot, Task};

    fn node(id: &str, kind: NodeKind) -> Node {
        Node {
            id: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            rev: 0,
            asset_id: (kind == NodeKind::Asset).then(|| format!("asset-{id}")),
            text: (kind == NodeKind::Note).then(String::new),
            model: (kind == NodeKind::Step).then(|| "mock:image".to_string()),
            prompt: None,
            negative_prompt: None,
            params: Map::new(),
            count: None,
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> Edge {
        Edge { id: id.into(), from: from.into(), to: to.into(), role: None, rev: 0 }
    }

    /// Applies ops the way the server does: the next version, all or nothing.
    fn apply(doc: &mut Doc, version: &mut i64, ops: Vec<Op>) -> std::result::Result<(), &'static str> {
        let mut copy = doc.clone();
        let mut name = "Board".to_string();
        match apply_ops(&mut copy, &mut name, &ops, *version + 1) {
            Ok(_) => {
                *doc = copy;
                *version += 1;
                Ok(())
            }
            Err(Refused::Stale) => Err("stale"),
            Err(Refused::Error(_)) => Err("invalid"),
        }
    }

    fn board_with(nodes: &[(&str, NodeKind)]) -> (Doc, i64) {
        let mut doc = Doc::default();
        let mut version = 0;
        let ops = nodes.iter().map(|(id, kind)| Op::AddNode { node: node(id, *kind) }).collect();
        apply(&mut doc, &mut version, ops).unwrap();
        (doc, version)
    }

    #[test]
    fn operations_update_items_and_their_revisions() {
        let (mut doc, mut v) = board_with(&[("a", NodeKind::Asset), ("s", NodeKind::Step), ("n", NodeKind::Note)]);
        assert_eq!(doc.node("s").unwrap().w, 300.0, "new nodes get their kind's width");
        assert!(doc.nodes.iter().all(|n| n.rev == 1));
        apply(&mut doc, &mut v, vec![Op::MoveNode { id: "a".into(), base: 1, x: 40.0, y: -12.5 }]).unwrap();
        apply(&mut doc, &mut v, vec![Op::AddEdge { edge: Edge { role: Some(InputRole::Reference), ..edge("e", "a", "s") } }]).unwrap();
        let a = doc.node("a").unwrap();
        assert_eq!((a.x, a.y, a.rev), (40.0, -12.5, 2));
        assert_eq!(doc.node("s").unwrap().rev, 1, "a new edge leaves its ends alone");
        assert_eq!(doc.edge("e").unwrap().rev, 3);
        // A role only means something on an input of a step.
        apply(&mut doc, &mut v, vec![Op::AddEdge { edge: Edge { role: Some(InputRole::Mask), ..edge("x", "s", "n") } }]).unwrap();
        assert_eq!(doc.edge("x").unwrap().role, None);
        // Deleting a node removes its edges and its run.
        doc.runs.insert("s".into(), StepRun { status: RunStatus::Succeeded, by: "u".into(), jobs: vec![], outputs: vec![], after: vec![], error: None, updated_at: 0, rev: 1 });
        apply(&mut doc, &mut v, vec![Op::DeleteNode { id: "s".into(), base: 1 }]).unwrap();
        assert!(doc.edges.is_empty());
        assert!(doc.runs.is_empty());
    }

    #[test]
    fn edits_to_different_nodes_both_land_and_a_stale_write_is_refused() {
        let (mut doc, mut v) = board_with(&[("a", NodeKind::Note), ("b", NodeKind::Note)]);
        // Two people saw both notes at revision 1.
        let first = vec![Op::UpdateNode { id: "a".into(), base: 1, w: None, text: Some("one".into()), model: None, prompt: None, negative_prompt: None, params: None, count: None }];
        let second = vec![Op::MoveNode { id: "b".into(), base: 1, x: 300.0, y: 0.0 }];
        apply(&mut doc, &mut v, first).unwrap();
        apply(&mut doc, &mut v, second).unwrap();
        assert_eq!(doc.node("a").unwrap().text.as_deref(), Some("one"));
        assert_eq!(doc.node("b").unwrap().x, 300.0);
        // The second person still thinks "a" is at revision 1.
        let stale = vec![
            Op::MoveNode { id: "b".into(), base: 3, x: 0.0, y: 0.0 },
            Op::UpdateNode { id: "a".into(), base: 1, w: None, text: Some("two".into()), model: None, prompt: None, negative_prompt: None, params: None, count: None },
        ];
        assert_eq!(apply(&mut doc, &mut v, stale), Err("stale"));
        assert_eq!(doc.node("a").unwrap().text.as_deref(), Some("one"), "the newer edit stays");
        assert_eq!(doc.node("b").unwrap().x, 300.0, "a refused request changes nothing");
        assert_eq!(v, 3);
        // Deleting a node someone changed, or touching one someone deleted, is stale too.
        assert_eq!(apply(&mut doc, &mut v, vec![Op::DeleteNode { id: "a".into(), base: 1 }]), Err("stale"));
        apply(&mut doc, &mut v, vec![Op::DeleteNode { id: "a".into(), base: 2 }]).unwrap();
        assert_eq!(apply(&mut doc, &mut v, vec![Op::MoveNode { id: "a".into(), base: 2, x: 1.0, y: 1.0 }]), Err("stale"));
        assert_eq!(apply(&mut doc, &mut v, vec![Op::AddEdge { edge: edge("e", "a", "b") }]), Err("stale"));
        // The snapshot sent back names what is gone and what is current.
        let current = snapshot(&doc, "board", v, &["a".into(), "b".into()], &[]);
        assert_eq!(current.removed_nodes, ["a"]);
        assert_eq!(current.nodes[0].id, "b");
    }

    #[test]
    fn bad_operations_are_refused_whole() {
        let (mut doc, mut v) = board_with(&[("s", NodeKind::Step), ("t", NodeKind::Step), ("n", NodeKind::Note)]);
        let before = doc.clone();
        for ops in [
            vec![Op::AddNode { node: node("s", NodeKind::Note) }],
            vec![Op::AddNode { node: Node { text: Some("x".into()), ..node("z", NodeKind::Asset) } }],
            vec![Op::AddNode { node: Node { id: "bad id".into(), ..node("z", NodeKind::Note) } }],
            vec![Op::AddNode { node: Node { x: f64::INFINITY, ..node("z", NodeKind::Note) } }],
            vec![Op::AddNode { node: Node { count: Some(9), ..node("z", NodeKind::Step) } }],
            vec![Op::UpdateNode { id: "n".into(), base: 1, w: None, text: None, model: Some("mock:image".into()), prompt: None, negative_prompt: None, params: None, count: None }],
            vec![Op::AddEdge { edge: edge("e", "s", "s") }],
            vec![Op::AddEdge { edge: edge("e", "s", "t") }, Op::AddEdge { edge: edge("f", "t", "s") }],
            vec![Op::AddEdge { edge: edge("e", "s", "t") }, Op::AddEdge { edge: edge("f", "s", "t") }],
            vec![Op::Rename { name: " ".into() }],
        ] {
            assert_eq!(apply(&mut doc, &mut v, ops), Err("invalid"));
        }
        assert_eq!(doc, before);
        assert_eq!(v, 1);
    }

    fn spec() -> ModelSpec {
        ModelSpec::new("mock", "video", "Clip", MediaKind::Video, &[Task::ImageToVideo])
            .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
            .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 2))
            .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1))
            .input(InputSlot::new(InputRole::Video, MediaKind::Video, 1))
    }

    fn feed(id: &str, kind: MediaKind, role: Option<InputRole>, from_step: bool) -> Feed {
        Feed { asset_id: id.into(), kind, role, from_step }
    }

    #[test]
    fn edges_wire_into_the_models_input_slots() {
        let spec = spec();
        let roles = |feeds: &[Feed]| wire_inputs(&spec, feeds).map(|inputs| inputs.into_iter().map(|i| (i.asset_id, i.role)).collect::<Vec<_>>());
        // Without a role, files fill the free slots in the model's order; a mask is never guessed.
        let wired = roles(&[
            feed("a", MediaKind::Image, None, false),
            feed("b", MediaKind::Image, None, false),
            feed("c", MediaKind::Image, Some(InputRole::Mask), false),
            feed("d", MediaKind::Video, None, false),
            feed("e", MediaKind::Image, None, false),
        ])
        .unwrap();
        assert_eq!(
            wired,
            [("a".into(), InputRole::StartFrame), ("b".into(), InputRole::EndFrame), ("c".into(), InputRole::Mask), ("d".into(), InputRole::Video), ("e".into(), InputRole::Reference)]
        );
        // A chosen role wins over the order.
        assert_eq!(roles(&[feed("a", MediaKind::Image, Some(InputRole::Reference), false)]).unwrap(), [("a".into(), InputRole::Reference)]);
        // A connected file beyond a full slot is refused; extra results of a step are left out.
        let over = [feed("a", MediaKind::Image, Some(InputRole::StartFrame), false), feed("b", MediaKind::Image, Some(InputRole::StartFrame), false)];
        assert!(roles(&over).unwrap_err().contains("at most 1 start frame"));
        let results = [feed("a", MediaKind::Image, Some(InputRole::StartFrame), true), feed("b", MediaKind::Image, Some(InputRole::StartFrame), true)];
        assert_eq!(roles(&results).unwrap(), [("a".into(), InputRole::StartFrame)]);
        // Kinds and roles the model does not take are named.
        assert!(roles(&[feed("v", MediaKind::Video, Some(InputRole::StartFrame), false)]).unwrap_err().contains("takes images as its start frame"));
        assert!(roles(&[feed("s", MediaKind::Audio, None, true)]).unwrap_err().contains("does not take audio"));
        let text_only = ModelSpec::new("mock", "t", "Text", MediaKind::Image, &[Task::TextToImage]);
        assert!(wire_inputs(&text_only, &[feed("a", MediaKind::Image, Some(InputRole::Reference), false)]).unwrap_err().contains("does not take a reference"));
    }

    #[test]
    fn downstream_runs_follow_the_step_graph() {
        let (mut doc, mut v) = board_with(&[("a", NodeKind::Step), ("b", NodeKind::Step), ("c", NodeKind::Step), ("d", NodeKind::Step), ("x", NodeKind::Step), ("f", NodeKind::Asset)]);
        // a feeds b and c; b and c feed d; x (not reached from a) also feeds d.
        let links = [("1", "a", "c"), ("2", "a", "b"), ("3", "b", "d"), ("4", "c", "d"), ("5", "x", "d"), ("6", "f", "a")];
        apply(&mut doc, &mut v, links.iter().map(|(id, from, to)| Op::AddEdge { edge: edge(id, from, to) }).collect()).unwrap();
        let order = downstream_order(&doc, "a");
        let ids: Vec<&str> = order.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"], "board order among ready steps, d after both of its feeders");
        assert!(order[0].1.is_empty());
        let mut d_after = order[3].1.clone();
        d_after.sort();
        assert_eq!(d_after, ["b", "c"], "d waits for the steps of this run only");
        assert_eq!(downstream_order(&doc, "d").len(), 1);
    }
}
