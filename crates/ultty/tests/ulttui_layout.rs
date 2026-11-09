//! `ulttui` widgets as the terminal lays them out: the messages a view
//! sends (`render_view_messages`) applied to a scene and run through the
//! layout engine. Catches layouts that differ from the widget's ANSI
//! rendering, such as children drawn on a border.

use ulttui::widgets::{render_view_messages, List, Modal, ScrollView, Table, Text, View};
use ultty::scene::layout::LayoutEngine;
use ultty::scene::{Region, SceneTree};
use ultty_protocol::{AppMessage, ComputedRegionLayout, Content, RegionId};

/// Lay `view` out in a `cols` x `rows` viewport.
fn layout(view: &dyn View, cols: f32, rows: f32) -> (SceneTree, Vec<ComputedRegionLayout>) {
    let mut scene = SceneTree::new();
    for message in render_view_messages(view).unwrap() {
        match message {
            AppMessage::CreateRegion {
                id,
                parent,
                constraints,
                style,
                ..
            } => {
                let mut region = Region::new(id);
                region.parent = Some(parent);
                region.constraints = constraints;
                region.style = style;
                scene.insert(region);
            }
            AppMessage::SetContent { id, content } => {
                if let Some(region) = scene.get_mut(id) {
                    region.content = content;
                }
            }
            _ => {}
        }
    }
    let regions = LayoutEngine::new()
        .compute_scene_layout(&mut scene, cols, rows)
        .unwrap();
    (scene, regions)
}

/// The layout of the region whose text contains `needle`.
fn region_with_text(
    scene: &SceneTree,
    regions: &[ComputedRegionLayout],
    needle: &str,
) -> ComputedRegionLayout {
    regions
        .iter()
        .find(|r| {
            scene.get(r.id).is_some_and(
                |region| matches!(&region.content, Content::Text(t) if t.text.contains(needle)),
            )
        })
        .copied()
        .unwrap_or_else(|| panic!("no region with {needle:?}"))
}

fn first_child_of_root(
    scene: &SceneTree,
    regions: &[ComputedRegionLayout],
) -> ComputedRegionLayout {
    let id = scene.get(RegionId::ROOT).unwrap().children[0];
    *regions.iter().find(|r| r.id == id).unwrap()
}

/// A native border takes no room, so a bordered scroll view pads its child
/// inside the border, where the ANSI `Block` puts it.
#[test]
fn a_bordered_scroll_view_keeps_its_child_inside_the_border() {
    let view = ScrollView::new().child(Text::new("hello"));
    let (scene, regions) = layout(&view, 40.0, 10.0);
    let outer = first_child_of_root(&scene, &regions);
    let text = region_with_text(&scene, &regions, "hello");
    assert_eq!(
        (text.x - outer.x, text.y - outer.y),
        (1.0, 1.0),
        "the child starts one cell in from the border"
    );
}

#[test]
fn a_bordered_list_keeps_its_items_inside_the_border() {
    let view = List::new().items(vec!["first", "second"]);
    let (scene, regions) = layout(&view, 40.0, 10.0);
    let outer = first_child_of_root(&scene, &regions);
    let item = region_with_text(&scene, &regions, "first");
    assert!(
        item.x - outer.x >= 1.0 && item.y - outer.y >= 1.0,
        "the first item is on the border: {item:?} in {outer:?}"
    );
}

/// The region holding `needle` sits at least one cell inside the view's own
/// bordered region.
fn assert_inside_border(view: &dyn View, needle: &str) {
    let (scene, regions) = layout(view, 40.0, 12.0);
    let outer = first_child_of_root(&scene, &regions);
    let inner = region_with_text(&scene, &regions, needle);
    assert!(
        inner.x - outer.x >= 1.0 && inner.y - outer.y >= 1.0,
        "{needle:?} is on the border: {inner:?} in {outer:?}"
    );
}

#[test]
fn a_bordered_table_keeps_its_header_inside_the_border() {
    let view = Table::new()
        .headers(vec!["Name", "Size"])
        .rows(vec![vec!["a.rs", "12"]]);
    assert_inside_border(&view, "Name");
}

#[test]
fn a_bordered_modal_keeps_its_body_inside_the_border() {
    let view = Modal::new()
        .title("Confirm")
        .body("Apply the change?")
        .buttons(vec!["Yes", "No"])
        .visible(true);
    assert_inside_border(&view, "Apply the change?");
}
