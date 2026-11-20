//! System parameter infrastructure: how a plain function becomes a [`System`].
//!
//! # Responsibilities
//!
//! - Defines the [`SystemParam`] trait that lets a function's parameters take
//!   themselves out of the [`World`] (`Query<T>`, `Commands`, `Res<T>`).
//! - Provides [`IntoSystem`] to turn a plain function into a boxed [`System`].
//! - Implements [`System`] for the closure each adapted function is boxed
//!   into, so the engine can call every system the same way.
//!
//! # Design
//!
//! Four traits work together. [`SystemParam`] takes a system's arguments out of
//! the [`World`] and the [`CommandQueue`], and reports what they access to the
//! scheduler. [`SystemParamFunction`] adapts a `FnMut` that takes those
//! arguments into something callable. [`IntoSystem`] boxes that function into a
//! [`System`] the engine can store and call. [`SystemOutput`] turns the
//! function's return value (`()` or `Result<(), SystemError>`) into one shared
//! result type.
//!
//! # Safety
//!
//! [`SystemParam::fetch`] returns a value the compiler treats as `'static`,
//! even though it borrows from the world and the command queue. A system's
//! parameter type is fixed when the system is registered, so it cannot name the
//! lifetime of the borrow it gets when it is called. The transmutation that
//! hides this is deliberate, and it is sound only while the parameter does not
//! outlive the system call:
//!
//! 1. A system parameter must not be stored: no static or global state, and
//!    nothing the system call outlives.
//! 2. It must not escape through a closure that outlives the system: a
//!    background thread, an async task, a callback registered for later.
//! 3. It must not be returned.
//!
//! The engine's runner (`IntoSystem::into_system` / `into_hot_system`) is the
//! one audited caller, and the signature refuses a safe call from anywhere
//! else. Breaking these rules is undefined behavior.
//!
//! ```no_run
//! # use pill_engine::*;
//! # #[derive(Debug, Clone)] struct Position { x: f32, y: f32 }
//! # impl Component for Position {}
//! # #[derive(Debug, Clone)] struct Velocity { x: f32, y: f32 }
//! # impl Component for Velocity {}
//! fn movement_system(mut query: Query<(&mut Position, &Velocity)>) {
//!     for (mut position, velocity) in query.iter_mut() {
//!         position.x += velocity.x;  // used within the system
//!     }
//! }  // query dropped here - the borrow ends with the call
//! ```

// Current crate
use crate::commands::{CommandQueue, Commands};
use crate::error::SystemError;
use crate::query::{Query, QueryFilter, QueryTarget, Res, ResMut};
use crate::resource::{Resource, ResourceId};
use crate::scheduler::SystemAccess;
use crate::world::World;

// =============================================================================
// System
// =============================================================================

/// A function the engine can run as a system.
///
/// A system works on `World` data. It runs every frame and can read and write
/// components, create entities, and more.
///
/// A system reports a failure through [`Result::Err`]. The engine records the
/// failure and carries on with the other systems.
///
/// Systems must be `Send`, so the scheduler can run them on several threads.
pub trait System: Send {
    /// Runs the system once.
    ///
    /// # Errors
    ///
    /// Returns the [`SystemError`] that ended the call: the one the system
    /// returned, or the one the engine built from a panic in a patched body.
    fn run(&mut self, world: &mut World, queue: &mut CommandQueue) -> Result<(), SystemError>;
}

/// Implements [`System`] for any `FnMut` closure with the right signature.
///
/// This is the form a registered system takes: the engine's registration
/// entry stores the boxed closure and calls it like any other [`System`].
impl<F, Output> System for F
where
    F: FnMut(&mut World, &mut CommandQueue) -> Output + Send,
    Output: SystemOutput,
{
    fn run(&mut self, world: &mut World, queue: &mut CommandQueue) -> Result<(), SystemError> {
        self(world, queue).into_system_result()
    }
}

// =============================================================================
// SystemOutput
// =============================================================================

/// A value a system function can return.
///
/// A system returns `()` when it cannot fail, or `Result<(), SystemError>` when
/// it can. The engine turns either one into the same result type.
pub trait SystemOutput: 'static {
    /// Turns this return value into the result type the engine runs on.
    fn into_system_result(self) -> Result<(), SystemError>;
}

impl SystemOutput for () {
    fn into_system_result(self) -> Result<(), SystemError> {
        Ok(())
    }
}

impl SystemOutput for Result<(), SystemError> {
    fn into_system_result(self) -> Result<(), SystemError> {
        self
    }
}

// =============================================================================
// SystemParam
// =============================================================================

/// A type that can be used as a parameter of a system function.
///
/// This trait is what makes system signatures flexible: any type that
/// implements it can appear in the parameter list of a system.
///
/// # Safety
///
/// Implementations change lifetimes with a transmutation. The module-level
/// docs list the rules that keep this sound.
///
/// A parameter must not escape the system function that received it.
pub trait SystemParam: Sized {
    /// Takes this parameter out of the world and the command queue.
    ///
    /// # Safety
    ///
    /// The returned value is marked `'static`, but it really borrows from
    /// `world` and `queue`. The caller must make sure that:
    ///
    /// 1. the value is dropped before the system function returns,
    /// 2. it is never stored in static or global state, and
    /// 3. it is never moved onto a background thread or into an async task.
    ///
    /// Breaking these rules is undefined behavior. The engine's runner
    /// (`IntoSystem::into_system` / `into_hot_system`) is the one caller that
    /// upholds them, and the signature refuses a safe call from anywhere else:
    ///
    /// ```compile_fail
    /// use pill_engine::system::SystemParam;
    ///
    /// fn fetch_safely<P: SystemParam>(
    ///     world: &mut pill_engine::world::World,
    ///     queue: &mut pill_engine::commands::CommandQueue,
    /// ) {
    ///     let _ = P::fetch(world, queue);
    /// }
    /// ```
    unsafe fn fetch(world: &mut World, queue: &mut CommandQueue) -> Self;

    /// Records which components and resources this parameter reads and writes.
    ///
    /// The engine calls this while registering a system, to build the graph
    /// that decides which systems may run at the same time. The default
    /// implementation records nothing, which suits a parameter that touches
    /// neither components nor resources.
    fn report_access(_access: &mut SystemAccess) {
        // The default records no access.
    }
}

/// Lets a system queue entity changes that apply after it returns.
impl SystemParam for Commands<'static> {
    unsafe fn fetch(world: &mut World, queue: &mut CommandQueue) -> Self {
        // SAFETY: The lifetime of the borrow is transmuted to `'static`.
        //
        // This is sound only if the caller keeps the SystemParam contract:
        // - the `Commands<'static>` must not escape the system function,
        // - it must not be stored in global or static state, and
        // - it must be dropped before the system returns.
        //
        // The engine's system runner calls systems as opaque functions that
        // cannot return their parameters, so it keeps these rules.
        //
        // It is undefined behavior if `Commands` escapes: stored in a static,
        // moved to another thread, or captured by a closure that outlives the
        // call.
        unsafe { std::mem::transmute(Commands::new(queue, world)) }
    }

    fn report_access(access: &mut SystemAccess) {
        // Commands can create and destroy entities and add or remove
        // components, so the system needs exclusive access to the world.
        access.set_uses_commands(true);
    }
}

/// Lets a system take any query as a parameter.
///
/// One implementation covers every query pattern, so a new query type needs no
/// separate implementation of this trait.
impl<Q: QueryTarget + 'static, F: QueryFilter + 'static> SystemParam for Query<'static, Q, F> {
    unsafe fn fetch(world: &mut World, _queue: &mut CommandQueue) -> Self {
        // SAFETY: The lifetime of the borrow is transmuted to `'static`.
        //
        // This is sound only if the caller keeps the SystemParam contract:
        // - the `Query<'static, Q, F>` must not escape the system function,
        // - it must not be stored in global or static state, and
        // - it must be dropped before the system returns.
        //
        // The engine's system runner calls systems as opaque functions that
        // cannot return their parameters, so it keeps these rules.
        //
        // It is undefined behavior if `Query` escapes: stored in a static,
        // moved to another thread, or captured by a closure that outlives the
        // call.
        unsafe {
            let query: Query<Q, F> = Query::new(world);
            std::mem::transmute(query)
        }
    }

    fn report_access(access: &mut SystemAccess) {
        let (reads, writes) = Q::report_component_access();
        for component_id in reads {
            access.add_read(component_id);
        }
        for component_id in writes {
            access.add_write(component_id);
        }
        // A filter can require a component to be present or to have changed
        // (`With<T>`, `Changed<T>`). Such a filter needs read access to that
        // component, or the scheduler could run a writer of it in parallel.
        for component_id in F::included_component_ids() {
            access.add_read(component_id);
        }
    }
}

/// Gives a system shared access to a resource.
///
/// The scheduler records this as a resource read, so several systems may read
/// the same resource at the same time.
impl<T: Resource> SystemParam for Res<'static, T> {
    unsafe fn fetch(world: &mut World, _queue: &mut CommandQueue) -> Self {
        // SAFETY: The lifetime of the borrow is transmuted to `'static`.
        //
        // This is sound only if the caller keeps the SystemParam contract:
        // - the `Res<'static, T>` must not escape the system function,
        // - it must not be stored in global or static state, and
        // - it must be dropped before the system returns.
        //
        // It is undefined behavior if `Res` escapes.
        unsafe {
            let resource: Res<T> = Res::new(&*world);
            std::mem::transmute(resource)
        }
    }

    fn report_access(access: &mut SystemAccess) {
        access.add_resource_read(ResourceId::of::<T>());
    }
}

/// Gives a system exclusive access to a resource.
///
/// The scheduler records this as a resource write, so no other system touches
/// the same resource at the same time.
impl<T: Resource> SystemParam for ResMut<'static, T> {
    unsafe fn fetch(world: &mut World, _queue: &mut CommandQueue) -> Self {
        // SAFETY: The lifetime of the borrow is transmuted to `'static`.
        //
        // This is sound only if the caller keeps the SystemParam contract:
        // - the `ResMut<'static, T>` must not escape the system function,
        // - it must not be stored in global or static state, and
        // - it must be dropped before the system returns.
        //
        // It is undefined behavior if `ResMut` escapes.
        unsafe {
            let resource: ResMut<T> = ResMut::new(world);
            std::mem::transmute(resource)
        }
    }

    fn report_access(access: &mut SystemAccess) {
        access.add_resource_write(ResourceId::of::<T>());
    }
}

// =============================================================================
// SystemParam Tuple Implementations
// =============================================================================

/// Implements [`SystemParam`] for tuples, so a system can take more than one
/// parameter.
///
/// Each element is fetched on its own and put into the tuple.
macro_rules! impl_system_param_tuple {
    ($($T:ident),*) => {
        #[allow(non_snake_case)]
        impl<$($T: SystemParam),*> SystemParam for ($($T,)*) {
            unsafe fn fetch(world: &mut World, queue: &mut CommandQueue) -> Self {
                // SAFETY: Each element is fetched and moved straight into the
                // tuple this function returns. The caller's promise to drop the
                // value before it returns therefore covers every element, just
                // as it covers a single parameter.
                unsafe { ($($T::fetch(world, queue),)*) }
            }

            fn report_access(access: &mut SystemAccess) {
                $($T::report_access(access);)*
            }
        }
    };
}

// Tuples take 0 to 7 parameters. The empty tuple is written out here; the
// macro below covers the rest.
impl SystemParam for () {
    unsafe fn fetch(_world: &mut World, _queue: &mut CommandQueue) -> Self {}

    fn report_access(_access: &mut SystemAccess) {
        // An empty parameter list reports no access.
    }
}

impl_system_param_tuple!(A);
impl_system_param_tuple!(A, B);
impl_system_param_tuple!(A, B, C);
impl_system_param_tuple!(A, B, C, D);
impl_system_param_tuple!(A, B, C, D, E);
impl_system_param_tuple!(A, B, C, D, E, F);
impl_system_param_tuple!(A, B, C, D, E, F, G);

// =============================================================================
// ReadOnlySystemParam
// =============================================================================

/// A system parameter that only reads, so a
/// [`run_if`](crate::SystemOptions::run_if) condition may take it.
///
/// Read-only is decided by the parameter type, which is why this is a marker
/// trait rather than a runtime check: `Res<T>` reads a resource, a query of
/// shared references reads components, and a tuple reads whatever its members
/// do. `ResMut<T>`, `Commands` and a query containing `&mut T` change the
/// world, so none of them implements it, and a condition that asks for one
/// does not compile.
#[diagnostic::on_unimplemented(
    message = "a `run_if` condition may only read, and `{Self}` can change the world",
    label = "this parameter can write",
    note = "take `Res<T>` or a read-only query; leave `ResMut<T>`, `Commands` and mutable queries to the system the condition gates"
)]
pub trait ReadOnlySystemParam: SystemParam {}

impl<T: Resource> ReadOnlySystemParam for Res<'static, T> {}

impl<Q: crate::query::ReadOnlyQueryTarget + 'static, F: QueryFilter + 'static> ReadOnlySystemParam
    for Query<'static, Q, F>
{
}

impl ReadOnlySystemParam for () {}

/// Implements [`ReadOnlySystemParam`] for tuples whose members all read, so a
/// condition can take several read-only parameters.
macro_rules! impl_read_only_system_param_tuple {
    ($($T:ident),*) => {
        impl<$($T: ReadOnlySystemParam),*> ReadOnlySystemParam for ($($T,)*) {}
    };
}

impl_read_only_system_param_tuple!(A);
impl_read_only_system_param_tuple!(A, B);
impl_read_only_system_param_tuple!(A, B, C);
impl_read_only_system_param_tuple!(A, B, C, D);
impl_read_only_system_param_tuple!(A, B, C, D, E);
impl_read_only_system_param_tuple!(A, B, C, D, E, F);
impl_read_only_system_param_tuple!(A, B, C, D, E, F, G);

// =============================================================================
// SystemParamFunction
// =============================================================================

/// A function that can become a system.
///
/// The trait is implemented for functions with one to seven [`SystemParam`]
/// parameters, plus the case with none. It is the bridge between a function a
/// project writes and the [`System`] the engine runs. The associated
/// [`Output`] is the function's return type, `()` or
/// `Result<(), SystemError>`.
///
/// [`Output`]: SystemParamFunction::Output
pub trait SystemParamFunction<Input: SystemParam>: 'static {
    /// The function's return value, `()` or `Result<(), SystemError>`.
    type Output: SystemOutput;

    /// Calls the wrapped function with the parameters it was given.
    ///
    /// `input` is the tuple fetched from the [`World`] and the
    /// [`CommandQueue`]. The return value goes through [`SystemOutput`] to
    /// become a result.
    fn run(&mut self, input: Input) -> Self::Output;
}

/// Implements [`SystemParamFunction`] for functions with one to seven
/// parameters.
macro_rules! impl_system_param_function {
    ($($T:ident),*) => {
        #[allow(non_snake_case)]
        impl<Function, Output, $($T: SystemParam),*> SystemParamFunction<($($T,)*)> for Function
        where
            Function: FnMut($($T),*) -> Output + Send + 'static,
            Output: SystemOutput,
        {
            type Output = Output;

            fn run(&mut self, input: ($($T,)*)) -> Self::Output {
                let ($($T,)*) = input;
                self($($T),*)
            }
        }
    };
}

// Implement for functions with 0 parameters
impl<F, Output> SystemParamFunction<()> for F
where
    F: FnMut() -> Output + Send + 'static,
    Output: SystemOutput,
{
    type Output = Output;

    fn run(&mut self, _input: ()) -> Self::Output {
        self()
    }
}

// Implement for functions with 1-7 parameters
impl_system_param_function!(A);
impl_system_param_function!(A, B);
impl_system_param_function!(A, B, C);
impl_system_param_function!(A, B, C, D);
impl_system_param_function!(A, B, C, D, E);
impl_system_param_function!(A, B, C, D, E, F);
impl_system_param_function!(A, B, C, D, E, F, G);

// =============================================================================
// IntoSystem
// =============================================================================

/// Converts a function into a [`System`] the engine can run.
///
/// It uses the [`SystemParam`] machinery to fetch the function's parameters.
/// `engine.register_system(name, function)` relies on this trait to turn a
/// plain function into a boxed `System`.
pub trait IntoSystem<Input: SystemParam> {
    /// Boxes this function into a [`System`] trait object the engine can store
    /// and invoke generically.
    fn into_system(self) -> Box<dyn System>;

    /// Boxes this function so its body can be replaced while the game runs.
    ///
    /// It works like [`Self::into_system`], except that the call to the user's
    /// function goes through `slot`. Replacing the body is then a single atomic
    /// store: the returned box, its vtable, the scheduler graph and the
    /// system's metadata stay where they are, and the `World` is left alone.
    ///
    /// The slot is set up here because this is the only place that knows the
    /// concrete `F` and `Input`. `name` is the identity a panicking patched body
    /// is reported under, which lets the host roll that function back.
    #[cfg(feature = "development")]
    fn into_hot_system(
        self,
        slot: std::sync::Arc<crate::hot_patch::HotSlot>,
        name: String,
    ) -> Box<dyn System>;
}

/// Implements [`IntoSystem`] for any function that implements
/// [`SystemParamFunction`].
///
/// This is where the pieces meet:
/// 1. the function's parameters implement [`SystemParam`],
/// 2. [`SystemParam::fetch`] takes those parameters out of the world,
/// 3. the function is called with them, and
/// 4. the whole thing is boxed as a [`System`] for the engine to store.
impl<F, Input> IntoSystem<Input> for F
where
    F: SystemParamFunction<Input> + Send + 'static,
    Input: SystemParam,
{
    fn into_system(mut self) -> Box<dyn System> {
        Box::new(move |world: &mut World, queue: &mut CommandQueue| {
            // Step 1: Fetch the system's parameters from the world and queue.
            //
            // SAFETY: `input` is dropped when this closure call ends, before
            // `world` and `queue` - borrowed from the caller's frame - go out
            // of scope, and the closure returns only the system's own result,
            // so the parameter cannot escape. That is exactly the
            // drop-before-return rule the `'static` marker on `fetch` assumes.
            let input = unsafe { Input::fetch(world, queue) };
            // Step 2: Call the wrapped function and turn its return value into
            // a system result.
            self.run(input).into_system_result()
        })
    }

    #[cfg(feature = "development")]
    fn into_hot_system(
        mut self,
        slot: std::sync::Arc<crate::hot_patch::HotSlot>,
        name: String,
    ) -> Box<dyn System> {
        // Record the baseline implementation and the signature a replacement
        // has to match. This is the only place that knows the concrete `F` and
        // `Input`.
        slot.initialize(
            <F as SystemParamFunction<Input>>::run as *const () as usize,
            crate::hot_patch::signature_hash::<F, Input>(),
        );

        Box::new(move |world: &mut World, queue: &mut CommandQueue| {
            // Step 1: Fetch the parameters exactly as the plain path does, so
            // a hot system and a plain one behave the same.
            //
            // SAFETY: As in `into_system`, `input` is dropped before this
            // closure returns, so the `'static` marker never outlives the
            // borrows it stands for. A panicking callee unwinds through the
            // call and drops `input` on the way out, before the `Err` arm
            // below is reached.
            let input = unsafe { Input::fetch(world, queue) };

            // Step 2: Read the implementation to call. This costs one acquire
            // load and an indirect call; measured at under 0.2 ns/call against
            // a direct call, which is below the noise floor of ABI and
            // register-allocation accidents.
            let address = slot.current();

            // SAFETY: `address` is one of two things. It is the baseline
            // written by `initialize` above -
            // `<F as SystemParamFunction<Input>>::run`, exactly this function
            // type. Or it is a replacement that `HotSlot::install` accepted,
            // which refuses any address whose signature hash differs from this
            // monomorphization's. A replacement lives in a patch library the
            // host never unloads, so the address stays executable for the whole
            // process. The acquire load pairs with the release store in
            // `install`, so the replacement's code is visible before the call.
            let run: fn(&mut F, Input) -> F::Output =
                unsafe { std::mem::transmute::<usize, fn(&mut F, Input) -> F::Output>(address) };

            // Step 3: Call the implementation and catch a panic from a patched
            // body. Here the failing function still has a name, so the panic
            // becomes the system failure the engine already records and
            // survives. `panic_into_failure` also counts the streak; when it
            // reaches three panics in a row, the host rolls the patch back.
            //
            // `AssertUnwindSafe` is fine: the world and the function are only
            // lent to the call, and nothing inconsistent survives it. The
            // engine treats the run as failed and carries on at the next frame
            // boundary, just as it does for a system that returned an error.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut self, input))) {
                Ok(output) => {
                    crate::hot_patch::record_success(&name);
                    output.into_system_result()
                }
                Err(panic_payload) => {
                    Err(crate::hot_patch::panic_into_failure(&name, panic_payload))
                }
            }
        })
    }
}
