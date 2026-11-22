//! Creates the camera and the three rotating models.
//!
//! # Responsibilities
//!
//! - Add the camera when the world has none.
//! - Spawn one model per showcase slot, each rotating around its own axis and
//!   tagged so a reload re-runs the system without stacking duplicates.
//!
//! # Design
//!
//! Every model draws the same mesh at the same scale. The bundled model is
//! authored roughly six world units across and standing on `y = 0`, so each
//! instance is scaled down and lowered by half its drawn height: the models
//! straddle the axis the camera looks down, which is what a click at the middle
//! of the viewport lands on. The middle model sits on that axis, so a pick at
//! the window's centre answers with it on every run.

// External crates
use pill_engine::{Entity, Query, World};
use pill_rendering::{CameraComponent, MeshRendererComponent, TransformComponent};

// Current crate
use crate::{
    asset_loading::SceneAssets, LifecycleProbe, ShowcaseComponent, ShowcasePoint,
    ShowcaseReferenceComponent, TagAlphaComponent,
};

/// Scale every model draws at: the bundled mesh spans six world units, and the
/// scene is sized so its models stay clickable in the editor's default view,
/// which looks at the origin from sixteen units away.
const MODEL_SCALE: f32 = 0.6;

/// Where a model's middle sits relative to its origin: the mesh is authored
/// standing on `y = 0`, so a model is lowered by half its drawn height to stand
/// astride the camera's axis.
const MODEL_CENTER_OFFSET: f32 = -3.15 * MODEL_SCALE * 0.5;

/// Where the models stand, one per showcase variant: a row across the camera's
/// view, spaced so no two silhouettes touch.
const MODEL_TRANSLATIONS: [[f32; 3]; 3] = [
    [-4.4, MODEL_CENTER_OFFSET, 0.0],
    [0.0, MODEL_CENTER_OFFSET, 0.0],
    [4.4, MODEL_CENTER_OFFSET, 0.0],
];

/// How far back the game camera stands: far enough that the whole row fits the
/// frame, which is what keeps the scene's screen size the same in the game view
/// as it was when the models were smaller.
const CAMERA_DISTANCE: f32 = 12.0;

/// Creates the camera and the models in `world`.
///
/// # Errors
///
/// Returns the engine's entity-creation error as text.
pub(crate) fn create(world: &mut World, assets: SceneAssets) -> Result<(), String> {
    if Query::<&CameraComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        // The camera carries the lifecycle probe the editor suites watch: it is
        // the one entity with no mesh, so a script on it changes neither the
        // viewport's picks nor any model's Inspector sections.
        world
            .create_entity()
            .with(CameraComponent::default())
            .with(TransformComponent {
                translation: [0.0, 0.0, CAMERA_DISTANCE],
                ..Default::default()
            })
            .with(LifecycleProbe::default())
            .build()
            .map_err(|error| error.to_string())?;
    }

    if Query::<&TagAlphaComponent>::new(world)
        .iter_mut()
        .next()
        .is_some()
    {
        return Ok(());
    }

    // The entity every showcase reference is pointed at: one of the models, so
    // the row names something the Entities tab lists.
    let mut follow_target = None;
    for (index, translation) in MODEL_TRANSLATIONS.into_iter().enumerate() {
        let entity = world
            .create_entity()
            .with(TransformComponent {
                translation,
                scale: [MODEL_SCALE; 3],
                ..Default::default()
            })
            .with(
                MeshRendererComponent::builder()
                    .mesh(&assets.mesh)
                    .material(&assets.material)
                    .build(),
            )
            // The Inspector showcase: each model carries one, with distinct
            // values so the sliders and drag values open on different numbers.
            .with(ShowcaseComponent {
                internal_id: index as u32 + 1,
                frame: 0.0,
                lean: index as f32 * 0.5 - 0.5,
                scale: 1.0 + index as f32 * 0.5,
                // A distinct pair per model, so the collection rows open on
                // different numbers too.
                points: [
                    ShowcasePoint {
                        x: index as f32,
                        y: index as f32 + 0.5,
                    },
                    ShowcasePoint {
                        x: index as f32 + 1.0,
                        y: index as f32 + 1.5,
                    },
                ],
            })
            .with(TagAlphaComponent)
            .build()
            .map_err(|error| error.to_string())?;
        if index == 0 {
            follow_target = Some(entity);
        }
    }

    // Every model but the first points at the first one, set from code so the
    // Inspector's reference row names a target the moment the project loads.
    // The first model carries none: the reference section is drawn above the
    // others, and putting it on that model too would push its own mesh rows out
    // of the Inspector's body. Two carriers rather than one because the row's
    // steps need a model the editor has not selected yet, and which model a
    // scene pick answers depends on the view.
    if let Some(target) = follow_target {
        let tagged: Vec<Entity> = {
            let mut query = Query::<(&TagAlphaComponent, Entity)>::new(world);
            query.iter_mut().map(|(_, entity)| entity).collect()
        };
        for follower in tagged.iter().copied().filter(|entity| *entity != target) {
            if world
                .get_component::<ShowcaseReferenceComponent>(follower)
                .is_none()
            {
                world
                    .add_component(
                        follower,
                        ShowcaseReferenceComponent {
                            follow: target,
                            weight: 0.5,
                        },
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}
