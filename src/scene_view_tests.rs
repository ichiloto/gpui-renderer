use super::*;
use crate::{retained_state::RetainedSession, state::PaintItem};
use serde_json::json;

fn parse_event_line(line: &str, event: Event) -> Value {
    let mut production = Vec::new();
    write_event(&mut production, Version::V2, &event).unwrap();
    assert_eq!(line.as_bytes(), production);
    assert_eq!(line.bytes().filter(|byte| *byte == b'\n').count(), 1);
    assert!(line.ends_with('\n'));
    serde_json::from_str(line).unwrap()
}

#[test]
fn public_ready_uses_production_drawing_capabilities_without_input_subscriptions() {
    let root = assets();
    let session = SceneSession::new(root.path(), grid()).unwrap();
    let hello = Hello {
        title: "Synthetic production session".into(),
        asset_root: root.path().into(),
        grid: grid(),
        required_capabilities: Vec::new(),
        icon: None,
    };
    let capabilities = hello.get_enabled_capabilities(Version::V2);
    assert_eq!(session.get_capabilities(), capabilities);
    assert!(!capabilities.contains(&SceneCapability::WindowActivation));
    assert!(!capabilities.contains(&SceneCapability::KeyTransitions));
    assert!(capabilities.contains(&SceneCapability::TileShadows));
    let ready = parse_event_line(
        &session.encode_ready().unwrap(),
        Event::Ready {
            capabilities: capabilities.clone(),
        },
    );
    assert_eq!(ready["protocol"], 2);
    assert_eq!(ready["type"], "ready");
    assert_eq!(
        ready["capabilities"],
        serde_json::to_value(capabilities).unwrap()
    );
    assert_eq!(session.get_generation(), 0);
}

#[test]
fn public_acknowledgements_preserve_frame_generation_and_staged_visibility() {
    for (generation, frame, presented) in [(1, 0, false), ((1u64 << 53) + 17, 91, true)] {
        let ack = parse_event_line(
            &SceneSession::encode_acknowledgement(generation, frame, presented).unwrap(),
            Event::FrameAck {
                generation,
                frame,
                presented,
            },
        );
        assert_eq!(ack["protocol"], 2);
        assert_eq!(ack["type"], "frame_ack");
        assert_eq!(ack["generation"].as_u64(), Some(generation));
        assert_eq!(ack["frame"].as_u64(), Some(frame));
        assert_eq!(ack["presented"].as_bool(), Some(presented));
        assert!(ack.get("expectedGeneration").is_none());
    }
}

#[test]
fn public_rejection_reports_the_accepted_staging_cursor_and_unmodified_diagnostic() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let empty = parse_event_line(
        &session.encode_rejection(0, "invalid envelope").unwrap(),
        Event::FrameRejected {
            generation: 0,
            expected_generation: 0,
            message: "invalid envelope".into(),
            resync_required: true,
        },
    );
    assert_eq!(empty["expectedGeneration"], 0);
    let visible = session.apply_update(first()).unwrap().unwrap();
    let mut staged = update(1, 2, false, vec![]);
    staged["frame"] = json!(29);
    staged["present"] = json!(false);
    assert!(session.apply_update(staged).unwrap().is_none());
    let mut invalid = actor(2);
    invalid["value"]["asset"] = json!("../outside.png");
    let message = session
        .apply_update(update(2, 3, false, vec![invalid]))
        .unwrap_err();
    let rejected = parse_event_line(
        &session.encode_rejection(3, &message).unwrap(),
        Event::FrameRejected {
            generation: 3,
            expected_generation: 2,
            message: message.clone(),
            resync_required: true,
        },
    );
    assert_eq!(rejected["protocol"], 2);
    assert_eq!(rejected["type"], "frame_rejected");
    assert_eq!(rejected["generation"], 3);
    assert_eq!(rejected["expectedGeneration"], 2);
    assert_eq!(rejected["resyncRequired"], true);
    assert_eq!(rejected["message"], message);
    assert!(rejected.get("frame").is_none());
    assert_eq!(session.get_generation(), 2);
    assert!(Arc::ptr_eq(
        &visible.prepared,
        session.prepared.as_ref().unwrap()
    ));
    let escaped = "bad \"crop\"\\path\nnext \u{754c}";
    let parsed = parse_event_line(
        &session.encode_rejection(3, escaped).unwrap(),
        Event::FrameRejected {
            generation: 3,
            expected_generation: 2,
            message: escaped.into(),
            resync_required: true,
        },
    );
    assert_eq!(parsed["message"], escaped);
    assert!(session.apply_update(update(2, 3, false, vec![])).is_err());
    let mut reset = first();
    reset["generation"] = json!(3);
    reset["frame"] = json!(31);
    let recovered = session.apply_update(reset).unwrap().unwrap();
    let ack: Value = serde_json::from_str(
        &SceneSession::encode_acknowledgement(recovered.get_generation(), 31, true).unwrap(),
    )
    .unwrap();
    assert_eq!(ack["generation"], 3);
    assert_eq!(ack["frame"], 31);
    assert_eq!(ack["presented"], true);
}

// Emitted by Engine RendererPresentation/RetainedPresentation from typed synthetic
// PresentationWorld, FieldViewport, sprite and overlay inputs. assets() supplies mutable test PNGs.
const ENGINE_SYNTHETIC_UPDATE: &str = r#"{"frame":1,"baseGeneration":0,"generation":1,"reset":true,"present":true,"operations":[{"op":"put","kind":"world","id":"map","value":{"columns":4,"rows":2,"cellWidth":48,"cellHeight":48,"layers":[{"id":"map:terrain","layer":-100,"kind":"gameplay"}]}},{"op":"worldRows","id":"map","rows":[{"row":0,"cells":[{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"}]}]},{"op":"worldRows","id":"map","rows":[{"row":1,"cells":[{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"},{"glyph":".","foreground":null,"background":null,"ownerLayerId":"map:terrain"}]}]},{"op":"put","kind":"canvas","id":"canvas","value":{"id":"canvas","width":40,"height":40,"mode":"overlay","order":0}},{"op":"put","kind":"canvas_image","id":"effect","value":{"id":"effect","asset":"actor.png","destination":{"x":0,"y":0,"width":8,"height":12},"layer":500,"order":0}},{"op":"put","kind":"text","id":"hud","value":{"id":"hud","layer":1000,"order":0,"runs":[{"row":0,"column":0,"text":"HP","foreground":null,"background":null}]}},{"op":"put","kind":"sprite","id":"hero","value":{"id":"hero","asset":"actor.png","x":1,"y":1,"width":8,"height":12,"anchor":"bottom_center","layer":100,"sourceRect":{"x":1,"y":0,"width":2,"height":3},"motion":{"duration":0.25},"lift":4,"quarterTurns":1,"order":0}}],"viewport":{"scale":1,"origin":{"x":0,"y":0},"clipRect":{"x":0,"y":0,"width":40,"height":40},"textLayerIds":[],"spriteIds":["hero"],"worldId":"map","worldOrigin":{"column":0,"row":0},"follow":{"spriteId":"hero","textLayerIds":[]}}}"#;

#[test]
fn actual_engine_produced_field_payload_prepares_and_receives_canonical_feedback() {
    let root = assets();
    let source: Value = serde_json::from_str(ENGINE_SYNTHETIC_UPDATE).unwrap();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let mut staged = source.clone();
    staged["operations"] = json!([source["operations"][0]]);
    staged["present"] = json!(false);
    staged.as_object_mut().unwrap().remove("viewport");
    assert!(session.apply_update(staged).unwrap().is_none());
    let ack = parse_event_line(
        &SceneSession::encode_acknowledgement(
            session.get_generation(),
            source["frame"].as_u64().unwrap(),
            false,
        )
        .unwrap(),
        Event::FrameAck {
            generation: 1,
            frame: 1,
            presented: false,
        },
    );
    assert_eq!(ack["presented"], false);
    let mut completed = source.clone();
    completed["baseGeneration"] = json!(1);
    completed["generation"] = json!(2);
    completed["reset"] = json!(false);
    completed["operations"] = json!(&source["operations"].as_array().unwrap()[1..]);
    let frame = session.apply_update(completed).unwrap().unwrap();
    let mut runtime = RetainedSession::default();
    let accepted = runtime
        .apply(serde_json::from_value(source).unwrap(), grid())
        .unwrap();
    let prepared = PreparedScene::prepare(
        accepted.scene,
        1,
        grid(),
        &AssetRoot::new(root.path()).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(frame.prepared.screen.plan, prepared.screen.plan);
    assert_eq!(
        frame.prepared.screen.canvas.as_ref().unwrap().source,
        prepared.screen.canvas.as_ref().unwrap().source
    );
    assert_eq!(frame.prepared.source.get_sprite_lift("hero"), 4);
    assert_eq!(frame.prepared.source.get_sprite_quarter_turns("hero"), 1);
    assert_eq!(
        frame.viewport.as_ref().unwrap().world_id.as_deref(),
        Some("map")
    );
    assert_eq!(frame.prepared.worlds.len(), 1);
    assert!(frame.prepared.screen.canvas_overlay);
    let ack = parse_event_line(
        &SceneSession::encode_acknowledgement(frame.get_generation(), 1, true).unwrap(),
        Event::FrameAck {
            generation: 2,
            frame: 1,
            presented: true,
        },
    );
    assert_eq!(ack["generation"], 2);
    assert_eq!(ack["frame"], 1);
    assert_eq!(ack["presented"], true);
}

fn grid() -> SceneGrid {
    SceneGrid {
        columns: 4,
        rows: 2,
        cell_width: 10,
        cell_height: 20,
    }
}

fn assets() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    image::RgbaImage::from_fn(8, 12, |x, y| {
        image::Rgba([x as u8 * 20, y as u8 * 20, 90, 255])
    })
    .save(dir.path().join("actor.png"))
    .unwrap();
    dir
}

fn actor(x: i32) -> Value {
    json!({"op":"put","kind":"sprite","id":"hero","value":{
        "id":"hero","order":0,"asset":"actor.png","x":x,"y":1,
        "width":8,"height":12,"anchor":"bottom_center","layer":100,
        "lift":4,"quarterTurns":1,"motion":{"duration":0.25},
        "sourceRect":{"x":1,"y":0,"width":2,"height":3}}})
}

fn viewport() -> Value {
    json!({"scale":1,"origin":{"x":0,"y":0},"worldOrigin":{"column":0,"row":0},
        "clipRect":{"x":0,"y":0,"width":40,"height":40},"worldId":"map",
        "textLayerIds":[],"spriteIds":["hero"],"follow":{"spriteId":"hero"}})
}

fn update(base: u64, generation: u64, reset: bool, operations: Vec<Value>) -> Value {
    json!({"frame":generation,"baseGeneration":base,"generation":generation,
        "reset":reset,"operations":operations})
}

fn first() -> Value {
    let rows = (0..2)
        .map(|row| {
            json!({"row":row,"cells":(0..4).map(|_| json!({
        "glyph":".","foreground":null,"background":null,"ownerLayerId":"map:floor"}))
        .collect::<Vec<_>>()})
        })
        .collect::<Vec<_>>();
    let mut first = update(
        0,
        1,
        true,
        vec![
            json!({"op":"put","kind":"world","id":"map","value":{
            "columns":4,"rows":2,"cellWidth":48,"cellHeight":48,"layers":[
                {"id":"map:floor","layer":-100,"kind":"gameplay"},
                {"id":"map:tiles","layer":-50,"kind":"tiles","coversLayerId":"map:floor"}],
            "tileset":{"tileSize":2,"sheets":["actor.png"],"tiles":[{"frames":[[
                {"sheet":0,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0}]]}]}}}),
            json!({"op":"worldRows","id":"map","rows":rows}),
            json!({"op":"worldTiles","id":"map","layerId":"map:tiles","rows":[
            {"row":0,"cells":[{"column":0,"tile":0}]}]}),
            actor(1),
            json!({"op":"put","kind":"text","id":"hud","value":{
            "id":"hud","order":0,"layer":200,"runs":[
                {"row":0,"column":0,"text":"HP","foreground":null,"background":null}]}}),
            json!({"op":"put","kind":"canvas","id":"canvas","value":{
            "width":40,"height":40,"mode":"overlay"}}),
            json!({"op":"put","kind":"canvas_image","id":"effect","value":{
            "id":"effect","order":0,"asset":"actor.png","layer":500,
            "destination":{"x":0,"y":0,"width":8,"height":12}}}),
        ],
    );
    first["viewport"] = viewport();
    first
}

#[test]
fn canonical_shadows_match_runtime_depth_and_reuse_projection_during_camera_updates() {
    use crate::map_world::{MapAssets, MapLayerKind, MapWorld};
    use crate::retained_paint::{
        FieldPaint::{Plan, World},
        cell_bounds, merge_paint_order,
    };

    let root = assets();
    let mut payload = first();
    let definition = &mut payload["operations"][0]["value"];
    definition["layers"].as_array_mut().unwrap().extend([
        json!({"id":"map:shadows","layer":-50,"kind":"shadows"}),
        json!({"id":"map:canopy","layer":150,"kind":"tiles"}),
    ]);
    definition["tileset"]["tiles"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "width":1,"left":1,"top":-1,"frames":[[
                {"fill":[0,0,0,64],"width":1,"height":2,"left":0,"top":0}
            ]]
        }));
    payload["operations"].as_array_mut().unwrap().push(json!({
        "op":"worldTiles","id":"map","layerId":"map:shadows",
        "rows":[{"row":0,"cells":[{"column":1,"tile":1}]}]
    }));
    let operations = payload["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| {
            operation["kind"] == "world"
                || operation["op"] == "worldRows"
                || operation["op"] == "worldTiles"
        })
        .cloned()
        .collect();
    let editor =
        MapWorld::from_operations(operations, &MapAssets::new(root.path()).unwrap()).unwrap();
    assert_eq!(
        editor.get_layers().nth(2).unwrap().kind,
        MapLayerKind::Shadows
    );
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let first = session.apply_update(payload.clone()).unwrap().unwrap();
    let mut runtime = RetainedSession::default();
    let accepted = runtime
        .apply(serde_json::from_value(payload.clone()).unwrap(), grid())
        .unwrap();
    let production = PreparedScene::prepare(
        accepted.scene,
        1,
        grid(),
        &AssetRoot::new(root.path()).unwrap(),
        None,
    )
    .unwrap();
    let world = first.prepared.get_world("map").unwrap();
    let normal = production.get_world("map").unwrap();
    let depths: Vec<_> = world.layers.iter().map(|layer| layer.layer).collect();
    assert_eq!(
        world
            .layers
            .iter()
            .map(|layer| layer.id.as_str())
            .collect::<Vec<_>>(),
        ["map:floor", "map:tiles", "map:shadows", "map:canopy"]
    );
    assert_eq!(
        merge_paint_order(&depths, &[100, 200]),
        [World(0), World(1), World(2), Plan(0), World(3), Plan(1)]
    );
    for column in 0..4 {
        let covered = world.has_covering_tile(column, 0, "map:floor");
        assert_eq!(covered, column == 0);
        assert_eq!(covered, normal.has_covering_tile(column, 0, "map:floor"));
        assert_eq!(covered, editor.has_covering_tile(column, 0, "map:floor"));
    }
    assert_eq!(
        world.get_tile_image(1, 0).unwrap().as_bytes(0),
        normal.get_tile_image(1, 0).unwrap().as_bytes(0)
    );
    let shadow = world.get_tile_image(1, 0).unwrap().id;
    for (generation, column, scale, origin) in
        [(2, 1, 0.5, -5.0), (3, -1, 1.5, 2.0), (4, 0, 1.0, 0.0)]
    {
        let mut camera = update(generation - 1, generation, false, vec![]);
        camera["viewport"] = viewport();
        camera["viewport"]["worldOrigin"]["column"] = json!(column);
        camera["viewport"]["scale"] = json!(scale);
        camera["viewport"]["origin"]["x"] = json!(origin);
        camera["viewport"]["tileFrame"] = json!(if generation == 4 { 0 } else { 99 });
        let frame = session.apply_update(camera).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first.prepared, &frame.prepared));
        assert_eq!(
            frame
                .prepared
                .get_world("map")
                .unwrap()
                .get_tile_image(1, 99)
                .unwrap()
                .id,
            shadow
        );
        let view = frame.viewport.as_ref().unwrap();
        let mut seen = 0;
        world
            .source
            .project_visible_tiles("map:shadows", view, |cell, tile| {
                assert_eq!(tile, 1);
                let bounds = cell_bounds(
                    cell,
                    world.source.cell_size(),
                    (24.0, -24.0, 24.0),
                    ViewportTransform::fit(40.0, 40.0, 40.0, 40.0),
                    view,
                );
                assert_eq!(
                    bounds.left,
                    origin + ((1 - column) as f32 * 48.0 + 24.0) * scale
                );
                assert_eq!(bounds.top, -24.0 * scale);
                assert_eq!(bounds.width, 24.0 * scale);
                assert_eq!(bounds.height, 48.0 * scale);
                seen += 1;
            });
        assert_eq!(seen, 1);
    }
    let mut invalid = payload;
    invalid["baseGeneration"] = json!(4);
    invalid["generation"] = json!(5);
    invalid["reset"] = json!(false);
    invalid["operations"][0]["value"]["tileset"]["tiles"][1]["frames"][0][0]["fill"] =
        json!([0, 0, 0, 256]);
    assert!(session.apply_update(invalid).is_err());
    assert!(Arc::ptr_eq(
        &first.prepared,
        session.prepared.as_ref().unwrap()
    ));
    let mut cleanup = update(4, 6, true, vec![]);
    cleanup["viewport"] = Value::Null;
    let cleared = session.apply_update(cleanup).unwrap().unwrap();
    assert!(cleared.prepared.worlds.is_empty());
    assert!(
        !cleared
            .prepared
            .images
            .iter()
            .any(|image| image.id == shadow)
    );
    assert!(cleared.viewport.is_none());
}

#[test]
fn public_field_frame_matches_runtime_world_actors_overlay_and_order() {
    let root = assets();
    let mut view = SceneSession::new(root.path(), grid()).unwrap();
    let frame = view.apply_update(first()).unwrap().unwrap();
    let mut retained = RetainedSession::default();
    let accepted = retained
        .apply(serde_json::from_value(first()).unwrap(), grid())
        .unwrap();
    let runtime = PreparedScene::prepare(
        accepted.scene,
        1,
        grid(),
        &AssetRoot::new(root.path()).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(frame.get_size(), (40.0, 40.0));
    assert_eq!(frame.get_generation(), 1);
    assert_eq!(
        frame.prepared.screen.plan,
        [PaintItem::Sprite(0), PaintItem::Text(0)]
    );
    assert_eq!(frame.prepared.screen.plan, runtime.screen.plan);
    assert!(frame.prepared.screen.canvas_overlay);
    assert_eq!(
        frame.prepared.screen.canvas.as_ref().unwrap().source,
        runtime.screen.canvas.as_ref().unwrap().source
    );
    assert_eq!(frame.prepared.source.get_sprite_lift("hero"), 4);
    assert_eq!(frame.prepared.source.get_sprite_quarter_turns("hero"), 1);
    assert_eq!(
        frame
            .prepared
            .get_turned_sprite_image("hero")
            .unwrap()
            .as_bytes(0),
        runtime.get_turned_sprite_image("hero").unwrap().as_bytes(0)
    );
    let view = frame.viewport.as_ref().unwrap();
    let runtime_view = accepted.viewport.as_ref().unwrap();
    assert_eq!(view.world_id, runtime_view.world_id);
    assert_eq!(view.sprite_ids, runtime_view.sprite_ids);
    assert_eq!(view.origin, runtime_view.origin);
    assert_eq!(view.clip_rect, runtime_view.clip_rect);
    assert_eq!(view.follow, runtime_view.follow);
    assert!(frame.prepared.worlds["map"].has_covering_tile(0, 0, "map:floor"));
    assert!(!frame.prepared.worlds["map"].has_covering_tile(1, 0, "map:floor"));
    assert_eq!(
        frame.get_resources().decoded_bytes,
        runtime.screen.resources.decoded_bytes
    );
    let projected = crate::retained_paint::project_world(
        &frame.prepared.worlds["map"],
        frame.viewport.as_ref().unwrap(),
    );
    assert!(!projected.is_empty());
}

#[test]
fn camera_updates_reuse_the_complete_prepared_scene_and_sprite_updates_reuse_world() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let first = session.apply_update(first()).unwrap().unwrap();
    let mut camera = update(1, 2, false, vec![]);
    camera["viewport"] = viewport();
    camera["viewport"]["origin"]["x"] = json!(4.5);
    let moved = session.apply_update(camera).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first.prepared, &moved.prepared));
    assert_eq!(moved.viewport.as_ref().unwrap().origin.x, 4.5);
    let actor = session
        .apply_update(update(2, 3, false, vec![actor(2)]))
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        &first.prepared.worlds["map"],
        &actor.prepared.worlds["map"]
    ));
    assert!(Arc::ptr_eq(
        first.prepared.get_turned_sprite_image("hero").unwrap(),
        actor.prepared.get_turned_sprite_image("hero").unwrap()
    ));
}

#[test]
fn staged_world_uploads_do_not_replace_the_visible_frame_until_complete() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let source = first();
    let mut staged = update(0, 1, true, vec![source["operations"][0].clone()]);
    staged["present"] = json!(false);
    assert!(session.apply_update(staged).unwrap().is_none());
    assert_eq!(session.get_generation(), 1);
    let mut completed = update(
        1,
        2,
        false,
        source["operations"].as_array().unwrap()[1..].to_vec(),
    );
    completed["viewport"] = viewport();
    let frame = session.apply_update(completed).unwrap().unwrap();
    assert_eq!(frame.get_generation(), 2);
    assert_eq!(frame.prepared.worlds.len(), 1);
}

#[test]
fn invalid_candidate_preserves_visible_resources_and_requires_explicit_reset() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let accepted = session.apply_update(first()).unwrap().unwrap();
    let original = accepted.prepared.screen.sprites[0].image.clone();
    let mut bad = actor(2);
    bad["value"]["asset"] = json!("../outside.png");
    assert!(
        session
            .apply_update(update(1, 2, false, vec![bad]))
            .is_err()
    );
    assert_eq!(session.get_generation(), 1);
    assert!(Arc::ptr_eq(
        &accepted.prepared,
        session.prepared.as_ref().unwrap()
    ));
    assert!(
        session
            .apply_update(update(1, 2, false, vec![actor(2)]))
            .is_err()
    );
    let mut reset = first();
    reset["generation"] = json!(2);
    assert!(session.apply_update(reset).is_ok());
    assert!(Arc::ptr_eq(
        &original,
        &accepted.prepared.screen.sprites[0].image
    ));
}

#[test]
fn replacing_art_redecodes_without_identity_hashes_or_duplicated_dimensions() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let old = session.apply_update(first()).unwrap().unwrap();
    image::RgbaImage::from_pixel(16, 24, image::Rgba([180, 40, 10, 255]))
        .save(root.path().join("actor.png"))
        .unwrap();
    let replaced = session
        .apply_update(update(1, 2, false, vec![actor(1)]))
        .unwrap()
        .unwrap();
    assert_ne!(
        old.prepared.screen.sprites[0].image.id,
        replaced.prepared.screen.sprites[0].image.id
    );
    assert_eq!(replaced.prepared.screen.sprites[0].sprite.id, "hero");
    assert_eq!(replaced.prepared.screen.sprites[0].sprite.width, 8);
    assert_eq!(
        replaced.prepared.screen.sprites[0].image.size(0).width.0 as u32,
        16
    );
}

#[test]
fn preview_playhead_owns_slides_and_seek_or_new_session_clears_them() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let idle = session.apply_update(first()).unwrap().unwrap();
    let moved = session
        .apply_update(update(1, 2, false, vec![actor(2)]))
        .unwrap()
        .unwrap();
    let mut painter = ScenePainter::default();
    painter.observe_frame(&idle, 0.0);
    painter.observe_frame(&moved, 0.5);
    assert_eq!(painter.motion.get_sprite_shift("hero", 0.5), (-1.0, 0.0));
    painter.observe_frame(&moved, 0.625);
    assert_eq!(painter.motion.get_sprite_shift("hero", 0.625), (-0.5, 0.0));
    painter.observe_frame(&idle, 0.0);
    assert_eq!(painter.motion.get_sprite_shift("hero", 0.0), (0.0, 0.0));
    painter.observe_frame(&moved, 1.0);
    let mut other = SceneSession::new(root.path(), grid()).unwrap();
    let other_frame = other.apply_update(first()).unwrap().unwrap();
    painter.observe_frame(&other_frame, 1.0);
    assert!(!painter.motion.is_animating(1.0));
}

#[test]
fn closing_a_view_retires_previous_images_once() {
    let root = assets();
    let mut session = SceneSession::new(root.path(), grid()).unwrap();
    let frame = session.apply_update(first()).unwrap().unwrap();
    let mut painter = ScenePainter::default();
    assert!(
        painter
            .replace_cached_images(&frame.prepared.images)
            .is_empty()
    );
    assert!(
        painter
            .replace_cached_images(&frame.prepared.images)
            .is_empty()
    );
    assert_eq!(
        painter.replace_cached_images(&[]).len(),
        frame.prepared.images.len()
    );
    assert!(painter.replace_cached_images(&[]).is_empty());
}
