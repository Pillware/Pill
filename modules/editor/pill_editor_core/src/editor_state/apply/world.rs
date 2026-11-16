//! The world family's applier.
//!
//! This module applies the world commands: component fields, array elements,
//! entity and component lifecycle, resource fields, and the element transforms
//! a tool writes back.
//!
//! # Responsibilities
//!
//! - Apply the world edits and answer each with the inverse that undoes it,
//!   when one exists.
//! - Refuse a write the component registry cannot resolve, naming the
//!   component or field.
//! - Refuse an element move when no active tool writes element transforms.
//!
//! # Design
//!
//! One `match` over the family's own variants, called from
//! [`EditorCommand::apply_one`](super::EditorCommand). Every successful arm
//! returns a [`CommandOutcome`]: the inverse the history records, or
//! `plain()` for a command that cannot be undone.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::ChangeOrigin;
use pill_engine::Engine;

// Current crate
use super::CommandOutcome;
use super::*;

// =============================================================================
// Free Functions
// =============================================================================

/// The origin an editor change reports.
///
/// An enclosing scope wins: an undo or a redo replays a batch inside a
/// `HistoryRestore` scope, and the arms it delegates to must not relabel that
/// as an authored change. With no enclosing scope the editor's own value is
/// used, which is what a change a person made reports.
fn editor_origin(engine: &Engine, editor_default: ChangeOrigin) -> ChangeOrigin {
    match engine.world().change_origin() {
        ChangeOrigin::Simulation => editor_default,
        enclosing => enclosing,
    }
}

/// Applies one world command.
///
/// # Errors
///
/// Returns a message naming the command and the reason when a component name
/// is unknown or ambiguous, a component has no field layout, a field is
/// missing or its value is refused, the entity or the entity reference is
/// gone, the resource or its field is not editable, an element move arrives
/// with no tool to write it, or the queued world commands cannot be applied.
///
/// # Panics
///
/// Panics when `command` belongs to another family; `EditorCommand::apply_one`
/// routes only world commands here.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::CreateEntity { components } => {
            // Step 1: Build and check every component image before touching
            // the world, so one bad seed leaves the world unchanged.
            let mut native_components: Vec<Box<dyn pill_engine::commands::ComponentAdder>> =
                Vec::new();
            let mut descriptor_components = Vec::new();
            for seed in components {
                // Building the image checks that the component has a
                // registered, non-empty field layout.
                let component_image = engine
                    .world()
                    .build_component_image(&seed.type_name, &seed.fields)
                    .map_err(|error| format!("{command:?}: {error}"))?;
                // Two registrations claiming the same name is an error,
                // not a guess, so the editor never writes into the wrong
                // component's storage.
                let component_id = engine
                    .world()
                    .resolve_component_id_by_name_any(&seed.type_name)
                    .map_err(|error| format!("{command:?}: {error}"))?
                    .ok_or_else(|| {
                        format!("{command:?}: `{}` is not registered", seed.type_name)
                    })?;
                if component_id.is_native_storage() {
                    native_components.push(Box::new(
                        pill_engine::commands::ByteComponentAdder::new(
                            component_id,
                            component_image,
                        ),
                    ));
                } else {
                    descriptor_components.push((component_id, component_image));
                }
            }

            // Step 2: Reserve the handle, then queue the creation. `Entity`
            // is `Copy`, and `queue_deferred_commands` runs its closure
            // immediately, so the handle travels into the closure by value.
            // The inverse can then name it directly: no shared cell, no
            // lock, and no lookup that could fail.
            let assigned = engine.world_mut().reserve_entity();
            let origin = editor_origin(engine, ChangeOrigin::Authoring);
            engine
                .with_change_origin(origin, move |engine| {
                    engine.queue_deferred_commands(move |_world, queue| {
                        queue.create_mixed_entity(
                            assigned,
                            native_components,
                            descriptor_components,
                        );
                    });
                    engine.flush_deferred_commands()
                })
                .map_err(|errors| format!("{command:?}: {errors:?}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::DestroyEntity {
                entity: assigned,
            }))
        }

        EditorCommand::DestroyEntity { entity } => {
            // Capture the component values before the destroy: the restore
            // re-creates the entity from them.
            let seeds = super::EditorCommand::capture_seeds(engine, *entity)?;
            // The destroy goes through the command queue, so the scripts on the
            // entity hear that it stops existing before it goes. A capture
            // above already refused an entity that was gone.
            let destroyed_entity = *entity;
            let origin = editor_origin(engine, ChangeOrigin::Authoring);
            engine
                .with_change_origin(origin, move |engine| {
                    engine.queue_deferred_commands(move |_world, queue| {
                        queue.destroy_entity(destroyed_entity);
                    });
                    engine.flush_deferred_commands()
                })
                .map_err(|errors| format!("{command:?}: {errors:?}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::RestoreEntity {
                previous: destroyed_entity,
                components: seeds,
            }))
        }

        EditorCommand::AddComponent { entity, component } => {
            // Adding needs a registered, non-empty field layout, because the
            // new component starts zero-initialised and without a layout
            // there is no size to fill. The entity must not carry the
            // component yet: attaching it twice is an error, not a no-op.
            if engine
                .world()
                .resolve_entity_component_id(*entity, component)
                .is_some()
            {
                return Err(format!(
                    "{command:?}: `{component}` is already on the entity"
                ));
            }
            let component_image = engine
                .world()
                .build_component_image(component, &[])
                .map_err(|error| format!("{command:?}: {error}"))?;
            let component_id = engine
                .world()
                .resolve_component_id_by_name_any(component)
                .map_err(|error| format!("{command:?}: {error}"))?
                .ok_or_else(|| format!("{command:?}: `{component}` is not registered"))?;
            let origin = editor_origin(engine, ChangeOrigin::Authoring);
            engine
                .with_change_origin(origin, move |engine| {
                    engine.queue_deferred_commands(move |_world, queue| {
                        if component_id.is_native_storage() {
                            queue.add_component_adder_to_entity(
                                *entity,
                                Box::new(pill_engine::commands::ByteComponentAdder::new(
                                    component_id,
                                    component_image,
                                )),
                            );
                        } else {
                            queue.add_descriptor_component_to_entity(
                                *entity,
                                component_id,
                                component_image,
                            );
                        }
                    });
                    engine.flush_deferred_commands()
                })
                .map_err(|errors| format!("{command:?}: {errors:?}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::RemoveComponent {
                entity: *entity,
                component: component.clone(),
            }))
        }

        EditorCommand::RemoveComponent { entity, component } => {
            let component_id = engine
                .world()
                .resolve_entity_component_id(*entity, component)
                .ok_or_else(|| format!("{command:?}: `{component}` is not on the entity"))?;
            // Capture the field values first: the inverse re-attaches the
            // component with them. A component without a field layout has no
            // values to capture, so removing it cannot be undone.
            let field_names: Vec<String> = engine
                .world()
                .component_field_layout(component_id)
                .map(|layout| {
                    layout
                        .iter()
                        .map(|descriptor| descriptor.name.to_string())
                        .collect()
                })
                .unwrap_or_default();
            let captured_fields = if field_names.is_empty() {
                None
            } else {
                let field_values = engine
                    .world()
                    .read_component_fields(*entity, component)
                    .map_err(|error| format!("{command:?}: {error}"))?;
                Some(
                    field_names
                        .into_iter()
                        .zip(field_values)
                        .collect::<Vec<(String, FieldValue)>>(),
                )
            };
            let removing_entity = *entity;
            let origin = editor_origin(engine, ChangeOrigin::Authoring);
            engine
                .with_change_origin(origin, move |engine| {
                    engine.queue_deferred_commands(move |_world, queue| {
                        queue.remove_component_by_id(removing_entity, component_id);
                    });
                    engine.flush_deferred_commands()
                })
                .map_err(|errors| format!("{command:?}: {errors:?}"))?;
            Ok(match captured_fields {
                Some(fields) => CommandOutcome::inverse(EditorCommand::RestoreComponent {
                    entity: *entity,
                    component: component.clone(),
                    fields,
                }),
                None => CommandOutcome::plain(),
            })
        }

        EditorCommand::RestoreEntity {
            previous,
            components,
        } => {
            // A restore goes through the creation path so that the fresh
            // handle can be observed. The destroy inverse that path produces
            // is dropped on purpose: a restore replays a history step, it
            // never becomes one.
            let create_command = EditorCommand::CreateEntity {
                components: components.clone(),
            };
            let create_outcome = engine
                .with_change_origin(ChangeOrigin::HistoryRestore, |engine| {
                    super::EditorCommand::apply_one(engine, &create_command)
                })?;
            let Some(EditorCommand::DestroyEntity { entity: assigned }) = create_outcome.inverse
            else {
                return Err(format!("{command:?}: the restore reserved no handle"));
            };
            Ok(CommandOutcome {
                inverse: None,
                remap: Some((*previous, assigned)),
            })
        }

        EditorCommand::RestoreComponent {
            entity,
            component,
            fields,
        } => {
            // Attach the component zero-initialised first, then write the
            // captured values, so a restore is validated exactly like the
            // add and field edits the UI queues.
            let add_component_command = EditorCommand::AddComponent {
                entity: *entity,
                component: component.clone(),
            };
            engine.with_change_origin(ChangeOrigin::HistoryRestore, |engine| {
                super::EditorCommand::apply_one(engine, &add_component_command)
            })?;
            for (field, value) in fields {
                let set_field_command = EditorCommand::SetField {
                    entity: *entity,
                    component: component.clone(),
                    field: field.clone(),
                    value: value.clone(),
                };
                super::EditorCommand::apply_one(engine, &set_field_command)?;
            }
            Ok(CommandOutcome::plain())
        }
        EditorCommand::SetField {
            entity,
            component,
            field,
            value,
        } => {
            // Read the old value first: the inverse restores it. A field
            // that cannot be read cannot be written either, because both
            // resolve it the same way.
            let previous = engine
                .world()
                .read_component_field(*entity, component, field)
                .map_err(|error| format!("{command:?}: {error}"))?;
            engine
                .world_mut()
                .write_component_field(*entity, component, field, value.clone())
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::SetField {
                entity: *entity,
                component: component.clone(),
                field: field.clone(),
                value: previous,
            }))
        }

        EditorCommand::SetFieldAsset {
            entity,
            component,
            field,
            reference,
        } => {
            let previous = engine
                .world()
                .read_component_field(*entity, component, field)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let FieldValue::Asset { asset_type, .. } = &previous else {
                return Err(format!("{command:?}: `{field}` is not an asset reference"));
            };
            let value = match reference {
                None => FieldValue::Asset {
                    asset_type: asset_type.clone(),
                    index: u32::MAX,
                    generation: u32::MAX,
                },
                Some(reference) => resolve_asset_reference(engine, reference)
                    .map_err(|error| format!("{command:?}: {error}"))?,
            };
            engine
                .world_mut()
                .write_component_field(*entity, component, field, value)
                .map_err(|error| format!("{command:?}: {error}"))?;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                component = component.as_str(),
                field = field.as_str(),
                asset = reference.as_deref().unwrap_or("none"),
                "[editor] asset reference set"
            );
            Ok(CommandOutcome::inverse(EditorCommand::SetField {
                entity: *entity,
                component: component.clone(),
                field: field.clone(),
                value: previous,
            }))
        }

        EditorCommand::SetFieldEntity {
            entity,
            component,
            field,
            reference,
        } => {
            let previous = engine
                .world()
                .read_component_field(*entity, component, field)
                .map_err(|error| format!("{command:?}: {error}"))?;
            if !matches!(previous, FieldValue::Entity { .. }) {
                return Err(format!("{command:?}: `{field}` is not an entity reference"));
            }
            let value = match reference {
                None => FieldValue::Entity {
                    id: Entity::NULL.id(),
                    generation: Entity::NULL.generation(),
                },
                Some(target) => {
                    // A reference to an entity that has gone since the row
                    // was drawn is refused rather than written as a dangling
                    // handle. The component would otherwise keep pointing at
                    // an entity the world no longer has, and the row would
                    // show it missing.
                    if !engine.world().is_entity_valid(*target) {
                        return Err(format!(
                            "{command:?}: no live entity carries the reference {}",
                            EntityListEntry::display_of(*target)
                        ));
                    }
                    FieldValue::Entity {
                        id: target.id(),
                        generation: target.generation(),
                    }
                }
            };
            engine
                .world_mut()
                .write_component_field(*entity, component, field, value)
                .map_err(|error| format!("{command:?}: {error}"))?;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                component = component.as_str(),
                field = field.as_str(),
                reference = reference
                    .map(EntityListEntry::display_of)
                    .unwrap_or_else(|| "none".to_owned()),
                "[editor] entity reference set"
            );
            Ok(CommandOutcome::inverse(EditorCommand::SetField {
                entity: *entity,
                component: component.clone(),
                field: field.clone(),
                value: previous,
            }))
        }

        EditorCommand::SetArrayElement {
            entity,
            component,
            field,
            index,
            value,
        } => {
            // The field reads back as the whole array, so take the element
            // at `index` as the previous value; the inverse writes it back.
            let current_value = engine
                .world()
                .read_component_field(*entity, component, field)
                .map_err(|error| format!("{command:?}: {error}"))?;
            let FieldValue::Array(array_elements) = current_value else {
                return Err(format!("{command:?}: `{field}` is not an array field"));
            };
            let previous = array_elements
                .get(*index)
                .cloned()
                .ok_or_else(|| format!("{command:?}: element {index} is outside the array"))?;
            engine
                .world_mut()
                .write_component_array_element(*entity, component, field, *index, value.clone())
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::SetArrayElement {
                entity: *entity,
                component: component.clone(),
                field: field.clone(),
                index: *index,
                value: previous,
            }))
        }

        EditorCommand::SetResourceField {
            resource,
            field,
            value,
        } => {
            let world = engine.world_mut();
            if world.get_resource::<crate::gizmo::GizmoState>().is_none() {
                world.insert_resource(crate::gizmo::GizmoState::default());
            }
            let Some(gizmo_state) = world.get_resource_mut::<crate::gizmo::GizmoState>() else {
                return Err(format!("{command:?}: the gizmo state could not be claimed"));
            };
            let previous =
                crate::gizmo::set_resource_field(&mut gizmo_state.settings, resource, field, value)
                    .map_err(|message| format!("{command:?}: {message}"))?;
            Ok(CommandOutcome::inverse(EditorCommand::SetResourceField {
                resource: resource.clone(),
                field: field.clone(),
                value: previous,
            }))
        }

        EditorCommand::SetElementTransforms {
            entity,
            kind,
            moves,
        } => {
            let world = engine.world_mut();
            // A copy of the registry, so the report and the write can read it
            // while the tool sees the world mutably.
            let registry = crate::tools::tool_registry_mut(world).clone();
            // Step 1: Read what the tool reports for the moved elements now.
            // That is what this command replaces, so it is also its inverse -
            // and it is read before the write, so a tool that answers the
            // report from its own state sees the older value.
            let moved: Vec<u64> = moves.iter().map(|movement| movement.element).collect();
            let previous = registry
                .active_elements(*entity, world)
                .into_iter()
                .filter(|element| moved.contains(&element.id))
                .map(|element| crate::tools::ElementMove {
                    element: element.id,
                    transform: element.transform,
                })
                .collect::<Vec<_>>();
            // Step 2: Hand the moves to the tool, which owns the component
            // they live in. The editor never guesses that component, so a tool
            // that stopped taking moves refuses the step instead of writing
            // somewhere nobody asked.
            if !registry.active_set_transforms(*entity, moves, world) {
                return Err(format!(
                    "{command:?}: no active tool writes element transforms"
                ));
            }
            pill_core::info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                kind = kind.as_str(),
                elements = moves.len(),
                "[editor] element transforms written"
            );
            Ok(CommandOutcome::inverse(
                EditorCommand::SetElementTransforms {
                    entity: *entity,
                    kind: kind.clone(),
                    moves: previous,
                },
            ))
        }

        other => unreachable!("{:?} is not a world command", other.family()),
    }
}
