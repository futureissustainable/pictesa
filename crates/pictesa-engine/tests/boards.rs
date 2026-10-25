//! Boards on the free mock provider: steps run through the real queue and land their results,
//! concurrent edits merge or are refused, and only members can see or change a board.

use std::time::Duration;

use pictesa_core::InputRole;
use pictesa_engine::{
    boards::{BoardRole, Edge, Node, NodeKind, Op, RunStatus, StepRun},
    Config, Engine, EngineError, Mode, LOCAL_USER,
};
use serde_json::{json, Map};

async fn engine(name: &str, mode: Mode) -> Engine {
    std::env::set_var("PICTESA_MOCK", "1");
    let dir = std::env::temp_dir().join(format!("pictesa-boards-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::local(dir);
    cfg.mode = mode;
    let e = Engine::open(cfg).await.expect("engine opens");
    e.start();
    e
}

fn png(shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbImage::from_pixel(16, 8, image::Rgb([shade, 105, 61]));
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn node(id: &str, kind: NodeKind) -> Node {
    serde_json::from_value(json!({ "id": id, "kind": kind })).unwrap()
}

fn asset_node(id: &str, asset: &str) -> Op {
    Op::AddNode { node: Node { asset_id: Some(asset.into()), ..node(id, NodeKind::Asset) } }
}

fn step_node(id: &str, model: &str, prompt: &str) -> Op {
    Op::AddNode { node: Node { model: Some(model.into()), prompt: Some(prompt.into()), x: 400.0, ..node(id, NodeKind::Step) } }
}

fn link(id: &str, from: &str, to: &str, role: Option<InputRole>) -> Op {
    Op::AddEdge { edge: Edge { id: id.into(), from: from.into(), to: to.into(), role, rev: 0 } }
}

fn note_text(id: &str, base: i64, text: &str) -> Op {
    Op::UpdateNode { id: id.into(), base, w: None, text: Some(text.into()), model: None, prompt: None, negative_prompt: None, params: None, count: None }
}

async fn apply(e: &Engine, user: &str, board: &str, ops: Vec<Op>) -> pictesa_engine::boards::Changes {
    e.apply_board_ops(user, board, ops).await.expect("allowed").expect("not stale")
}

/// Waits until the step's run is over and returns it.
async fn finished(e: &Engine, user: &str, board: &str, step: &str) -> StepRun {
    for _ in 0..240 {
        let b = e.board(user, board).await.unwrap();
        if let Some(run) = b.runs.get(step).filter(|r| !r.status.is_active()) {
            return run.clone();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("step {step} did not finish");
}

#[tokio::test(flavor = "multi_thread")]
async fn steps_run_through_the_queue_and_land_results_linked_to_the_step() {
    let e = engine("run", Mode::Local).await;
    let photo = e.upload(LOCAL_USER, &png(200), Some("photo.png"), None).await.unwrap();
    let board = e.create_board(LOCAL_USER, "Lighthouse").await.unwrap();
    apply(&e, LOCAL_USER, &board.id, vec![
        asset_node("photo", &photo.id),
        step_node("paint", "mock:image", "a lighthouse at dusk"),
        link("in", "photo", "paint", Some(InputRole::Reference)),
    ])
    .await;

    let started = e.run_board_step(LOCAL_USER, &board.id, "paint", false).await.unwrap();
    assert_eq!(started.run.status, RunStatus::Running);
    let job = e.job(LOCAL_USER, &started.run.jobs[0]).await.unwrap();
    assert_eq!(job.inputs.len(), 1);
    assert_eq!((job.inputs[0].asset_id.as_str(), job.inputs[0].role), (photo.id.as_str(), InputRole::Reference), "the edge feeds the photo as a reference");
    assert_eq!(job.prompt, "a lighthouse at dusk");

    let run = finished(&e, LOCAL_USER, &board.id, "paint").await;
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.error);
    assert_eq!(run.outputs.len(), 1);
    let b = e.board(LOCAL_USER, &board.id).await.unwrap();
    let result = b.nodes.iter().find(|n| n.asset_id.as_ref() == Some(&run.outputs[0])).expect("the result is on the board");
    assert!(b.edges.iter().any(|edge| edge.from == "paint" && edge.to == result.id), "linked to its step");
    assert!(result.x > b.nodes.iter().find(|n| n.id == "paint").unwrap().x, "placed right of the step");
    assert_eq!(b.assets[&run.outputs[0]].job_id.as_deref(), Some(job.id.as_str()));

    // Run downstream: the second step takes the first one's new result, then runs itself.
    let base = b.nodes.iter().find(|n| n.id == "paint").unwrap().rev;
    apply(&e, LOCAL_USER, &board.id, vec![
        Op::UpdateNode { id: "paint".into(), base, w: None, text: None, model: None, prompt: Some("a lighthouse in fog".into()), negative_prompt: None, params: Some(Map::from_iter([("aspect_ratio".into(), json!("16:9"))])), count: Some(2) },
        step_node("cut", "mock:remove-background", ""),
        link("chain", "paint", "cut", None),
    ])
    .await;
    let started = e.run_board_step(LOCAL_USER, &board.id, "paint", true).await.unwrap();
    assert_eq!(started.steps, ["paint", "cut"]);
    let first = finished(&e, LOCAL_USER, &board.id, "paint").await;
    let second = finished(&e, LOCAL_USER, &board.id, "cut").await;
    assert_eq!((first.status, second.status), (RunStatus::Succeeded, RunStatus::Succeeded), "{:?} {:?}", first.error, second.error);
    assert_eq!(first.outputs.len(), 2);
    let cut = e.job(LOCAL_USER, &second.jobs[0]).await.unwrap();
    assert_eq!(cut.inputs.len(), 1, "the remover takes one image, so the extra result is left out");
    assert_eq!(cut.inputs[0].asset_id, first.outputs[0]);
    assert_eq!(e.board(LOCAL_USER, &board.id).await.unwrap().nodes.len(), 2 + 1 + 2 + 1 + 1);

    // A step without the input its model needs fails at once and says why; later steps settle.
    apply(&e, LOCAL_USER, &board.id, vec![step_node("lonely", "mock:remove-background", ""), step_node("after", "mock:image", "x"), link("l2", "lonely", "after", None)]).await;
    let refused = e.run_board_step(LOCAL_USER, &board.id, "lonely", true).await;
    assert!(matches!(&refused, Err(EngineError::Validation { .. })), "{refused:?}");
    let b = e.board(LOCAL_USER, &board.id).await.unwrap();
    assert_eq!(b.runs["lonely"].status, RunStatus::Failed);
    assert_eq!(b.runs["after"].status, RunStatus::Failed);
    assert!(e.share_board(LOCAL_USER, &board.id, "someone@example.com", BoardRole::Editor).await.is_err(), "local mode has one user");
}

#[tokio::test(flavor = "multi_thread")]
async fn members_edit_live_viewers_only_look_and_strangers_see_nothing() {
    let e = engine("members", Mode::Hosted).await;
    let owner = e.signup("owner@example.com", "correct horse battery", Some("Ana")).await.unwrap();
    let editor = e.signup("editor@example.com", "correct horse battery", Some("Bo")).await.unwrap();
    let viewer = e.signup("viewer@example.com", "correct horse battery", None).await.unwrap();
    let stranger = e.signup("stranger@example.com", "correct horse battery", None).await.unwrap();
    let photo = e.upload(&owner.id, &png(10), Some("mine.png"), None).await.unwrap();
    let board = e.create_board(&owner.id, "Shared").await.unwrap();
    apply(&e, &owner.id, &board.id, vec![asset_node("photo", &photo.id), Op::AddNode { node: node("a", NodeKind::Note) }, Op::AddNode { node: node("b", NodeKind::Note) }]).await;

    // Sharing is the owner's.
    assert!(matches!(e.share_board(&editor.id, &board.id, "viewer@example.com", BoardRole::Viewer).await, Err(EngineError::NotFound(_))));
    e.share_board(&owner.id, &board.id, "EDITOR@example.com ", BoardRole::Editor).await.unwrap();
    let members = e.share_board(&owner.id, &board.id, "viewer@example.com", BoardRole::Viewer).await.unwrap();
    assert_eq!(members.iter().map(|m| m.role).collect::<Vec<_>>(), [BoardRole::Editor, BoardRole::Viewer]);
    assert!(matches!(e.share_board(&editor.id, &board.id, "stranger@example.com", BoardRole::Viewer).await, Err(EngineError::Forbidden(_))));
    assert!(e.share_board(&owner.id, &board.id, "nobody@example.com", BoardRole::Viewer).await.is_err());

    // Strangers: no board, no list entry, no file.
    assert!(matches!(e.board(&stranger.id, &board.id).await, Err(EngineError::NotFound(_))));
    assert!(e.boards(&stranger.id).await.unwrap().is_empty());
    assert!(matches!(e.apply_board_ops(&stranger.id, &board.id, vec![Op::AddNode { node: node("x", NodeKind::Note) }]).await, Err(EngineError::NotFound(_))));
    assert!(e.asset_file(Some(&stranger.id), &photo.id).await.is_err());
    assert!(e.asset_thumb(Some(&stranger.id), &photo.id).await.is_err());

    // Viewers see the board and its files but change nothing.
    let seen = e.board(&viewer.id, &board.id).await.unwrap();
    assert_eq!(seen.role, BoardRole::Viewer);
    assert!(seen.assets.contains_key(&photo.id));
    assert!(e.asset_file(Some(&viewer.id), &photo.id).await.is_ok());
    assert!(matches!(e.apply_board_ops(&viewer.id, &board.id, vec![Op::AddNode { node: node("x", NodeKind::Note) }]).await, Err(EngineError::Forbidden(_))));
    assert!(matches!(e.run_board_step(&viewer.id, &board.id, "a", false).await, Err(EngineError::Forbidden(_))));
    assert!(matches!(e.delete_board(&viewer.id, &board.id).await, Err(EngineError::Forbidden(_))));
    assert_eq!(e.boards(&viewer.id).await.unwrap()[0].role, BoardRole::Viewer);

    // Every change reaches every member at once.
    let mut events = e.subscribe();
    let rev = seen.nodes.iter().find(|n| n.id == "a").unwrap().rev;
    apply(&e, &editor.id, &board.id, vec![note_text("a", rev, "from Bo")]).await;
    let mut told = Vec::new();
    while let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
        if ev.kind == "board" {
            assert_eq!(ev.data["nodes"][0]["text"], "from Bo");
            assert_eq!(ev.data["by"]["name"], "Bo");
            told.push(ev.user_id);
        }
    }
    told.sort();
    let mut everyone = vec![owner.id.clone(), editor.id.clone(), viewer.id.clone()];
    everyone.sort();
    assert_eq!(told, everyone);

    // The owner edits the other note from the same old view: both edits survive. The owner's
    // stale write to the note the editor changed is refused and returns the editor's version.
    let rev_b = seen.nodes.iter().find(|n| n.id == "b").unwrap().rev;
    apply(&e, &owner.id, &board.id, vec![note_text("b", rev_b, "from Ana")]).await;
    let stale = e.apply_board_ops(&owner.id, &board.id, vec![note_text("a", rev, "overwrite")]).await.unwrap().unwrap_err();
    assert_eq!(stale.current.nodes[0].text.as_deref(), Some("from Bo"));
    let now = e.board(&owner.id, &board.id).await.unwrap();
    let texts: Vec<Option<&str>> = ["a", "b"].iter().map(|id| now.nodes.iter().find(|n| n.id == *id).unwrap().text.as_deref()).collect();
    assert_eq!(texts, [Some("from Bo"), Some("from Ana")]);

    // Editors add their own files, never someone else's.
    let strangers_file = e.upload(&stranger.id, &png(99), None, None).await.unwrap();
    assert!(matches!(e.apply_board_ops(&editor.id, &board.id, vec![asset_node("s", &strangers_file.id)]).await, Err(EngineError::Validation { .. })));

    // An editor runs a step on the owner's photo: the run is the editor's, on a copy of the file
    // in the editor's library, and its result is visible to the owner through the board.
    apply(&e, &editor.id, &board.id, vec![step_node("paint", "mock:image", "a red door"), link("in", "photo", "paint", None)]).await;
    let started = e.run_board_step(&editor.id, &board.id, "paint", false).await.unwrap();
    let job = e.job(&editor.id, &started.run.jobs[0]).await.unwrap();
    assert_ne!(job.inputs[0].asset_id, photo.id, "the editor's own copy is the input");
    let (copy, _) = e.asset(&editor.id, &job.inputs[0].asset_id).await.unwrap();
    assert_eq!(copy.origin.as_deref(), Some("board"));
    assert!(e.job(&owner.id, &job.id).await.is_err(), "the run is the editor's");
    let run = finished(&e, &owner.id, &board.id, "paint").await;
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.error);
    assert!(e.asset_file(Some(&owner.id), &run.outputs[0]).await.is_ok());
    assert!(e.asset_file(Some(&stranger.id), &run.outputs[0]).await.is_err());

    // The owner may also take the editor's result into their own library.
    let copies = e.copy_board_assets(&owner.id, &board.id, &run.outputs).await.unwrap();
    assert_ne!(copies[0].id, run.outputs[0]);
    assert!(e.copy_board_assets(&stranger.id, &board.id, &run.outputs).await.is_err());

    // Leaving, removing and deleting.
    e.remove_board_member(&viewer.id, &board.id, &viewer.id).await.unwrap();
    assert!(e.board(&viewer.id, &board.id).await.is_err());
    assert!(e.asset_file(Some(&viewer.id), &photo.id).await.is_err(), "files go with the board");
    assert!(matches!(e.remove_board_member(&editor.id, &board.id, &owner.id).await, Err(EngineError::Forbidden(_))));
    assert!(matches!(e.delete_board(&editor.id, &board.id).await, Err(EngineError::Forbidden(_))));
    e.delete_board(&owner.id, &board.id).await.unwrap();
    assert!(e.board(&editor.id, &board.id).await.is_err());
}
