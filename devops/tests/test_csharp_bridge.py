"""
C# <-> Rust Bridge Integration Suite for Rust-Hybrid-ECS.

PROTECTS: C# hot reload: mirror layout, calls both ways, assembly swaps keeping state, layout migration.
TIER: host
SETS: hot-reload

REQUIREMENTS
  - Python 3.8+
  - Rust toolchain (cargo) on PATH
  - .NET SDK 8 on PATH (the host runs `dotnet build` for the managed project)
  - Run from the repository root or anywhere (paths are resolved from __file__)

DESCRIPTION
    End-to-end suite for the managed (C#) project backend and the extension
     -> C# component bridge. Launches the standalone host with
    `devops/tests/projects/managed` and the `pill_spline` module plus a component-less
    dummy module, then verifies from console output and on-disk artifacts:

      1. csharp_bridge_startup  - the host starts the C# backend, the host
          auto-generates the module's C# mirror file (exact namespace, size,
          alignment pad), a component-less dummy module still generates a
          mirror from its standalone value type and mirrored free function,
          the managed build is warning-free (no CS8019 unused-using), and the
          bridge probe proves both directions of the Rust <-> C# connection:
            * Rust -> C#: C# reads the spline the Rust module seeded
              (sees 1 spline, first P0.X=0, count=4);
            * C# -> Rust: C# creates its own spline through Commands and
              sees it in the SAME native column (sees 2 splines).
      2. csharp_hot_reload      - editing the probe file (a behavior-only change)
          is picked up by the watcher: dotnet rebuilds, the collectible
          assembly is reloaded (`[csharp_runtime] reloaded managed.dll`,
          `C# hot reload complete`), and the new assembly's probe runs with
          the expected spline count (module + pre-reload seed + post-reload
          seed = 3), proving the connection survives a managed hot reload.
      3. csharp_codegen_rebuild - after a clean restart with the mirror file
          deleted, the host regenerates it from the module's real layout
          before the project build (missing-file regeneration), the bridge
          still works, and the regenerated file is byte-identical to the
          committed bootstrap (drift in the tracked copy fails the suite).

    Every file the suite touches (project_settings.yaml, Components.cs, the
    generated mirror files) is backed up at startup and restored afterwards, and
    the probe file it writes (src/BridgeProbe.cs) is removed at the end, so a
    developer workspace is left exactly as it was.

USAGE
  python tests/test_csharp_bridge.py [--timeout-scale S] [--skip-build]

EXAMPLE USAGE
  python tests/test_csharp_bridge.py
  python tests/test_csharp_bridge.py --timeout-scale 1.5
  python tests/test_csharp_bridge.py --skip-build

--- SCRIPT ---
"""

import argparse
import os
import re
import sys
import time
from pathlib import Path
from typing import List, Sequence

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers. The
# host's log tokens live in one place.
# Standalone-runnable: put `devops/` on `sys.path` before reaching `core`, so
# the suite works from any working directory without a package import.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# Shared paths, tokens, print wrapper, OutputMonitor, process helpers.
from core.suite_common import *  # noqa: E402,F401,F403

# The shared scenario runner: one definition for every hot-reload suite.
# This suite adds only its C# tokens and scenario table.
from core.scenario import Scenario, ScenarioPhase, ScenarioRunner, run_scenario  # noqa: E402

# =============================================================================
# Session configuration
# =============================================================================

# `MANAGED_PROJECT_SYSTEMS_CS` and `MANAGED_PROJECT_COMPONENTS_CS` come from
# `core.projects` through the star import above.
SPLINE_LIB_RS = MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "src" / "pill_spline.rs"
SPLINE_GENERATED_FILE = (
    MODULES_ROOT / "extensions" / "pill_spline" / "modules" / "runtime" / "generated" / "pill_spline_Components.g.cs"
)
DUMMY_GENERATED_FILE = (
    MODULES_ROOT / "extensions" / "pill_dummy_color" / "modules" / "runtime" / "generated"
    / "pill_dummy_color_Components.g.cs"
)

# The tracked bootstrap's bytes, captured before the first host start so the
# codegen-rebuild check can compare the host's fresh output against the copy a
# standalone C# build would compile. Stays None when the file is absent, in
# which case the check only proves that regeneration happens.
COMMITTED_MIRROR_BYTES = None
# The suite's own probe file. Written before the host starts and removed in the
# suite's cleanup, so the example project carries no test-only code and this
# suite does not depend on demo text a project edit can silently change.
# It carries the schedule probes too: their counters are probe state, and the
# scenario that edits a declared order edits this file.
MANAGED_PROJECT_PROBE_CS = MANAGED_PROJECT_BRIDGE_PROBE_CS

# The C# project plus a component-less dummy module: the dummy exercises the
# codegen paths a module without components takes - a standalone
# `#[derive(PillMirror)]` value type and a `#[pill_mirror_fn]` free function -
# inside the same session that asserts the real module's component mirror.
CSHARP_SETTINGS = """\
name: "C# Bridge Test"
build_binary_name: "CSharpBridgeTest"
modules:
  - "pill_spline"
  - "pill_dummy_color"
"""

# The C# half of the bridge assertions.
#
# `Run` is the classic two-direction probe: it counts the splines C# can see in
# the module's native column (the module seeds one on load) and creates one of
# its own through Commands on the first frame of every assembly load. Seeding
# per assembly load is deliberate - statics start empty again after a swap, so
# each accepted reload adds exactly one spline on top of the survivors, which is
# what makes the probe counts move across reloads instead of standing still.
# The first report of a load therefore shows the module's spline alone
# (`P0.X=0, count=4`), because the entity this system creates is deferred to the
# end of the frame.
#
# `omo` drives the mirrored Rust method path: the OmoMO value is built in C#,
# the calls below resolve the module's exported trampolines, and the printed
# values (46 / 1212 / 1234) only exist if they really ran in Rust.
#
# `BridgeProbeStartup` exists so the startup-contract refusal has something real
# to change: startup method sets are fixed for the life of a host.
BRIDGE_PROBE_SOURCE = '''\
// Written by devops/tests/test_csharp_bridge.py while the suite runs; deleted
// afterwards. Not part of the project.
using System;
using System.Diagnostics;

namespace TracyLive;

/// <summary>Startup renamed by the suite to prove startup changes are refused.</summary>
public static class BridgeProbeStartup
{
    [Startup]
    public static void Start(Commands commands)
    {
        _ = commands;
    }
}

/// <summary>Reads the module's spline column and writes one of its own.</summary>
public static class ModuleSplineBridgeDemo
{
    // PILL0301 warns that mutable statics in a system-declaring type are what a
    // parallel batch races on and what a reload resets. Both statics here are
    // touched by this one system only, so nothing races, and the reset is the
    // behaviour the probe depends on (see the note above BRIDGE_PROBE_SOURCE).
    // Suppressed locally rather than project-wide: when managed systems can
    // reach engine resources, this state moves there and the pragma goes.
#pragma warning disable PILL0301
    private static bool _seeded;
    private static long _lastReport;
#pragma warning restore PILL0301

    [System]
    public static void Run(Query<Read<global::pill_spline.Spline>> query, Commands commands)
    {
        if (!_seeded)
        {
            _seeded = true;
            var spline = new global::pill_spline.Spline();
            spline.ControlPoints0 = new global::pill_spline.Vector3f { X = 10.0f, Y = 20.0f };
            spline.ControlPoints1 = new global::pill_spline.Vector3f { X = 50.0f, Y = 80.0f };
            spline.ControlPoints2 = new global::pill_spline.Vector3f { X = 90.0f, Y = 30.0f };
            spline.ControlPoints3 = new global::pill_spline.Vector3f { X = 130.0f, Y = 60.0f };
            spline.ControlPointCount = 4;
            spline.Elo = 30.0f;
            commands.CreateEntity().With(spline).Build();
            Console.WriteLine("[managed] cs spline bridge: seeded one spline via Commands");
        }

        long now = Stopwatch.GetTimestamp();
        if (Stopwatch.GetElapsedTime(_lastReport, now).TotalMilliseconds < 2000)
            return;
        _lastReport = now;

        int visibleSplines = 0;
        float firstPointX = float.NaN;
        uint firstPointCount = 0;
        foreach (var row in query.Rows())
        {
            var mirror = row.Spline;
            if (visibleSplines == 0)
            {
                firstPointX = mirror.ControlPoints0.X;
                firstPointCount = mirror.ControlPointCount;
            }
            visibleSplines++;
        }

        var omo = new global::pill_spline.OmoMO();
        omo.X = 12;
        omo.Y = 34;
        ulong omoSum = omo.GetSum();
        ulong omoA = omo.GetA();
        ulong omoB = omo.GetB();

        // The dummy module's mirrored free function: a plain static C# call
        // into Rust, resolved through the module's exported trampoline with
        // no component row involved.
        float alpha = global::pill_dummy_color.PillDummyColor.GetColorA();

        Console.WriteLine(
            $"[managed] cs spline bridge: sees {visibleSplines} spline(s), " +
            $"first P0.X={firstPointX}, count={firstPointCount}, " +
            $"omo=({omo.X},{omo.Y}) sum={omoSum} a={omoA} b={omoB} alpha={alpha}");
    }
}

/// <summary>One row the schedule probes create and count.</summary>
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct ScheduleMarker
{
    /// <summary>Assembly load that queued this row, so earlier loads are not counted.</summary>
    public long Pass;

    /// <summary>The frame that queued this row.</summary>
    public int Frame;
}

/// <summary>The declared orders, named so one edit moves the attribute and the log.</summary>
internal static class ScheduleProbeOrders
{
    /// <summary>The suite edits this value to prove a changed order re-registers.</summary>
    internal const int First = 480;

    internal const int Second = 520;

    internal const int PostProject = 900;
}

/// <summary>Counters shared by the schedule probes; a reload resets them with the assembly.</summary>
internal static class ScheduleProbeState
{
    /// <summary>Unique per assembly load, so rows from earlier loads are not counted.</summary>
    internal static readonly long Stamp = System.Diagnostics.Stopwatch.GetTimestamp();

    internal static int OrderedSteps;
    internal static int ProjectFrames;
    internal static int PostFrames;
}

/// <summary>Runs first by its declared order although its type name sorts last.</summary>
/// <remarks>
/// It declares a write on the marker column the second probe reads, so the two
/// land in successive batches instead of one parallel one; their type names
/// sort the opposite way to their Order values, so only the declaration can
/// produce the logged sequence.
/// </remarks>
public static class ZScheduleOrderFirstProbe
{
    [System(Name = "schedule_order_first", Order = ScheduleProbeOrders.First)]
    public static void Run(Query<Write<ScheduleMarker>> markers)
    {
        _ = markers;
        if (ScheduleProbeState.OrderedSteps >= 2)
            return;
        ScheduleProbeState.OrderedSteps++;
        Console.WriteLine(
            $"[probe] schedule step={ScheduleProbeState.OrderedSteps} order={ScheduleProbeOrders.First}");
    }
}

/// <summary>Runs second by its declared order although its type name sorts first.</summary>
public static class AScheduleOrderSecondProbe
{
    [System(Name = "schedule_order_second", Order = ScheduleProbeOrders.Second)]
    public static void Run(Query<Read<ScheduleMarker>> markers)
    {
        _ = markers;
        if (ScheduleProbeState.OrderedSteps >= 2)
            return;
        ScheduleProbeState.OrderedSteps++;
        Console.WriteLine(
            $"[probe] schedule step={ScheduleProbeState.OrderedSteps} order={ScheduleProbeOrders.Second}");
    }
}

/// <summary>Queues one marker row each frame, for the first two frames.</summary>
public static class ScheduleProjectProbe
{
    [System(Name = "schedule_project_probe", Order = 500)]
    public static void Run(Commands commands)
    {
        if (ScheduleProbeState.ProjectFrames >= 2)
            return;
        ScheduleProbeState.ProjectFrames++;
        commands.CreateEntity()
            .With(new ScheduleMarker
            {
                Pass = ScheduleProbeState.Stamp,
                Frame = ScheduleProbeState.ProjectFrames,
            })
            .Build();
        Console.WriteLine($"[probe] schedule project frame={ScheduleProbeState.ProjectFrames}");
    }
}

/// <summary>
/// Counts the marker rows after the project phase's flush; the count equals the
/// frame number only when this ran in the post-project phase.
/// </summary>
public static class SchedulePostProjectProbe
{
    [System(
        Name = "schedule_post_project_probe",
        Order = ScheduleProbeOrders.PostProject,
        RunPhase = RunPhase.PostProject)]
    public static void Run(Query<Read<ScheduleMarker>> markers)
    {
        if (ScheduleProbeState.PostFrames >= 2)
            return;
        ScheduleProbeState.PostFrames++;
        int count = 0;
        foreach (var row in markers.Rows())
        {
            // Only this load's row counts: the world keeps every earlier
            // generation's rows, and the probe must not read them as flushed
            // work of this frame.
            if (row.ScheduleMarker.Pass == ScheduleProbeState.Stamp &&
                row.ScheduleMarker.Frame == ScheduleProbeState.PostFrames)
                count++;
        }
        Console.WriteLine(
            $"[probe] schedule post_project frame={ScheduleProbeState.PostFrames} markers={count}");
    }
}
'''

# --- C# / bridge specific output tokens ---------------------------------------

# The host's collectible loader prints this after a successful assembly swap.
CSHARP_RELOADED_TOKEN = "[csharp_runtime] reloaded managed.dll"
# The same line carries the unload ledger: versions retired by a swap and
# versions a collection has confirmed dead. An accepted reload retires one more
# than it has collected, because the context it just unloaded has not been
# observed by a sweep yet.
CSHARP_RELOAD_LEDGER_PREFIX = "reloaded managed.dll (retired "
CSHARP_RETIRED_SURVIVOR_TOKEN = "are still loaded after"
CSHARP_RELOAD_LEDGER_PATTERN = re.compile(
    r"\[csharp_runtime\] reloaded managed\.dll \(retired (\d+), collected (\d+)\)"
)
# The host's reload poll accepts the swap only when system/signature/manifest
# identity is unchanged (behavior-only reload).
CSHARP_RELOAD_COMPLETE_TOKEN = "C# hot reload complete"
CSHARP_RELOAD_REJECTED_TOKEN = "C# reload rejected"

# The shared reload report: a managed swap records one event in it, so the same
# block a Rust reload prints carries the managed reload.
CSHARP_RELOAD_REPORT_TOKEN = "Hot reload finished"

# The managed loader prints the SPECIFIC reason to stderr before the Rust side
# prints its single generic line. Asserting on these is what tells a developer
# which of the three contracts they broke, and pins the wording that tells them.
CSHARP_LOAD_FAILED_TOKEN = "[csharp_runtime] reload failed:"
CSHARP_REJECT_COMPONENT_REASON = "C# component identities or layouts changed"
# The loader parks a version whose manifest changed and waits for the host's
# verdict; seeing this proves the handshake ran rather than the swap simply
# happening without one.
CSHARP_MANIFEST_PENDING_TOKEN = "awaiting the host's verdict"
CSHARP_REJECT_STARTUP_REASON = "C# startup methods changed"
# The bridge probe emitted by the suite's BridgeProbe.cs fixture (its
# `ModuleSplineBridgeDemo`).
BRIDGE_PROBE_PREFIX = "cs spline bridge: sees"
BRIDGE_PROBE_V2_PREFIX = "cs spline bridge v2: sees"
# The two probe lines this suite waits for, spelled out rather than written
# inline at each `wait_for` call: `test_log_contract.py` only sees a `*_TOKEN`
# constant, so an inline literal could be reworded with nothing to catch it.
# Both carry BRIDGE_PROBE_PREFIX and must move with it.
BRIDGE_PROBE_RUST_TO_CS_TOKEN = "omo=(12,34) sum=47 a=1212 b=1234"
BRIDGE_PROBE_CS_TO_RUST_TOKEN = "cs spline bridge: sees 2 spline(s)"
# dotnet build's summary line; a clean managed build reports exactly these.
DOTNET_CLEAN_WARNING_SUMMARY = "0 Warning(s)"
WARNING_CS_TOKEN = "warning CS"

# Host log line printed when a reloaded module changed the C# mirror surface
# and the host queued a C# project reload (live mirror regeneration).
CSHARP_MIRROR_REGEN_TOKEN = "module reload changed the C# mirror surface; queuing a C# project reload"

# Expected probe values for the canonical OmoMO demo (X=12, Y=34). The module's
# own arithmetic, not the suite's:
#   GetSum()  = x + y + 1  = 47
#   GetA()    = x + 1200   = 1212
#   GetB()    = y + 1200   = 1234
#   GetE()    = x + 9000   = 9012 (added live by the regeneration scenario)
#   alpha=1 is the dummy module's mirrored free function `get_color_a`,
#   reached from C# as `pill_dummy_color.PillDummyColor.GetColorA()`.
OMO_PROBE_SUM_A_B = "omo=(12,34) sum=47 a=1212 b=1234 alpha=1"
OMO_PROBE_WITH_E = "omo=(12,34) sum=47 a=1212 b=1234 e=9012 alpha=1"

# The scenario model and runner are shared: `core/scenario.py`.

# =============================================================================
# Bridge-specific startup assertions
# =============================================================================


def verify_generated_mirror(
    generated_file: Path,
    expected_fragments: Sequence[str],
    should_exist: bool,
    description: str,
) -> bool:
    """Asserts a module's generated mirror file exists (or not) and matches."""
    if not should_exist:
        if generated_file.exists():
            print(
                f"  [FAIL] {description}: expected no {generated_file.name}, "
                "but a stale mirror file exists"
            )
            return False
        print(f"  [OK] {description}: no {generated_file.name} (empty exposure).")
        return True

    if not generated_file.exists():
        print(f"  [FAIL] {description}: {generated_file.name} was not generated")
        return False
    content = generated_file.read_text(encoding="utf-8")
    for fragment in expected_fragments:
        if fragment not in content:
            print(
                f"  [FAIL] {description}: generated file misses {fragment!r}:\n"
                f"{content[:600]}"
            )
            return False
    print(f"  [OK] {description}: {generated_file.name} has the expected content.")
    return True


def verify_startup(
    monitor: OutputMonitor,
    startup_output: str,
) -> bool:
    """Validates host startup plus the on-disk codegen and clean C# build."""
    for token in (ANALYTICS_REPORT_TOKEN,):
        if token not in startup_output:
            print(f"  [FAIL] Missing startup token: {token!r}")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False
    if has_crash_signals(startup_output):
        print("  [FAIL] Crash signals during startup.")
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False

    # The module mirror must be regenerated from the module's real layout:
    # typed structs with exact sizes and field offsets for PillMirror-able
    # types (`Vector3f` declares its fields, so it lands as X/Y/Z at fixed
    # offsets rather than as an opaque blob).
    # Mirrored `#[pill_mirror_method]` Rust methods appear as typed C# instance
    # methods that call the module's trampoline through MirrorCall.
    if not verify_generated_mirror(
        SPLINE_GENERATED_FILE,
        [
            "namespace pill_spline {",
            "public partial struct Spline",
            "Size = 588",
            "[FieldOffset(192)] public uint ControlPointCount;",
            "[FieldOffset(196)] public float Elo;",
            # The stored tangents and the mode that reads them sit after the
            # control points, so an older scene document still lands on the
            # same first fields. The mode is a byte, which is what keeps the
            # whole row blittable across the managed boundary.
            "[FieldOffset(200)] public Vector3f InTangents0;",
            "[FieldOffset(392)] public Vector3f OutTangents0;",
            "[FieldOffset(584)] public byte TangentMode;",
            "[StructLayout(LayoutKind.Explicit, Size = 16)]\npublic partial struct OmoMO",
            "[FieldOffset(0)] public ulong X;",
            "[StructLayout(LayoutKind.Explicit, Size = 12)]\npublic partial struct Vector3f",
            "[FieldOffset(0)] public float X;",
            "[FieldOffset(4)] public float Y;",
            "[FieldOffset(8)] public float Z;",
            "public ulong GetSum()",
            "var __call = global::TracyLive.MirrorCall.Begin(\"pill_spline::OmoMO\", \"get_sum\");",
            "__call.PushAddress(ref global::System.Runtime.CompilerServices.Unsafe.AsRef(in this));",
            "public ulong GetA()",
            "MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref Unsafe.AsRef(in this), 1))",
        ],
        should_exist=True,
        description="pill_spline mirror",
    ):
        return False

    # The component-less dummy module still gets a mirror: with no component
    # to anchor it, the file carries the module's standalone value type and
    # its mirrored free function - the surface C# calls statically.
    if not verify_generated_mirror(
        DUMMY_GENERATED_FILE,
        [
            "namespace pill_dummy_color {",
            "public partial struct Tint",
            "[FieldOffset(0)] public float R;",
            "[FieldOffset(8)] public float B;",
            "public partial struct TestStruct",
            "public ulong Aaa()",
            "public static class PillDummyColor",
            "public static float GetColorA()",
            "var __call = global::TracyLive.MirrorCall.Begin(\"pill_dummy_color\", \"get_color_a\");",
        ],
        should_exist=True,
        description="pill_dummy_color mirror",
    ):
        return False

    # The managed build must be clean: dotnet reports "0 Warning(s)" and the
    # compiler emits no `warning CS` lines (e.g. CS8019 unused usings from a
    # header-only generated file).
    if DOTNET_CLEAN_WARNING_SUMMARY not in startup_output:
        print(
            "  [FAIL] Managed build did not report "
            f"{DOTNET_CLEAN_WARNING_SUMMARY!r} at startup."
        )
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False
    if WARNING_CS_TOKEN in startup_output:
        print(f"  [FAIL] Managed build produced {WARNING_CS_TOKEN!r} warnings.")
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False
    print(f"  [OK] Managed build is warning-free ({DOTNET_CLEAN_WARNING_SUMMARY}).")

    # Slice G: each managed component's reflected field layout must reach the
    # engine's field-layout table (what makes C# components inspectable in the
    # editor). The host logs every descriptor component's field layout
    # deterministically as `name@offset:size:type_tag` entries, so assert the
    # well-known fields of `TracyLive.PhysicsState` (six floats, then a byte)
    # and `TracyLive.SplineSample`.
    if "managed component field layout registered" not in startup_output:
        print("  [FAIL] No descriptor component field layout was registered at startup.")
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False
    if "Active@24:1:u8" not in startup_output:
        print("  [FAIL] TracyLive.PhysicsState layout is missing its trailing byte field.")
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False
    if "T@0:4:f32" not in startup_output:
        print("  [FAIL] TracyLive.SplineSample layout is missing its float field.")
        print(f"  Output tail:\n{startup_output[-1600:]}")
        return False
    print("  [OK] Managed component field layouts match the manifest.")

    # Bridge probes (both directions of the Rust <-> C# connection). The
    # probe is throttled to one line every 2s, so both waits are bounded by
    # PROBE_TIMEOUT. start_index 0 covers the whole run because the first
    # probe can fire before verify_startup finishes its file checks.
    #   Rust -> C#: C# reads the spline the module seeded with the correct
    #   layout (first P0.X=0, count=4).
    if not monitor.wait_for(
        BRIDGE_PROBE_RUST_TO_CS_TOKEN,
        PROBE_TIMEOUT,
        0,
    ):
        print("  [FAIL] Rust -> C# bridge probe missing (C# did not read the module spline).")
        print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
        return False
    print("  [OK] Rust -> C#: C# reads the module-seeded spline with the right layout.")
    #   C# -> Rust: C# created its own spline via Commands and sees it in the
    #   same native column (2 total). Only the count is asserted here: the
    #   project's own path system rewrites the live curve every frame, so the
    #   first control point and the point count move from the second report on.
    if not monitor.wait_for(
        BRIDGE_PROBE_CS_TO_RUST_TOKEN,
        PROBE_TIMEOUT,
        0,
    ):
        print("  [FAIL] C# -> Rust bridge probe missing (C# spline not in the native column).")
        print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
        return False
    print("  [OK] C# -> Rust: C#-created spline is visible in the module's native column.")
    #   Mirrored Rust method: C# calls the module's OmoMO::get_sum trampoline
    #   through the typed mirror (X=12, Y=34 -> 46). This only succeeds when the
    #   host resolved the exported trampoline and handed the address to C#.
    if not monitor.wait_for(OMO_PROBE_SUM_A_B, PROBE_TIMEOUT, 0):
        print("  [FAIL] Mirrored OmoMO method probe missing (GetSum did not run).")
        print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
        return False
    print("  [OK] Rust method mirroring: C# GetSum/GetA/GetB call the module's OmoMO methods.")
    return True

# =============================================================================
# Scenarios
# =============================================================================

# Behavior-only edit to the bridge probe: changes the log prefix but not the
# system name, its signature, the component manifest, or the startups, so the
# collectible reload is accepted (C# reload is behavior-only by design).
# Matches inside the interpolated string `$"[managed] cs spline bridge:
# sees {visibleSplines} spline(s), "`.
BRIDGE_PROBE_PREFIX_EDIT = (
    "cs spline bridge: sees {visibleSplines} spline(s), ",
    "cs spline bridge v2: sees {visibleSplines} spline(s), ",
)

# Each of the three contracts the managed loader refuses to let a reload break.
# All three edits are valid C# that compiles cleanly - the build must succeed and
# the LOADER must refuse, which is the whole point. A compile error would prove
# nothing about the loader.

# Adds a field to a project-owned component, changing its layout and therefore
# the manifest the native component registry was built from. `SplineSample` is
# the smallest component the project itself declares; the match starts at the
# field's last decorator so the attributes stay with the field they describe.
COMPONENT_LAYOUT_EDIT = (
    "    [Range(0f, 1f)]\n    public float T;\n}",
    "    [Range(0f, 1f)]\n    public float T;\n\n    /// <summary>Added by the suite; changes the component's layout.</summary>\n    public float Extra;\n}",
)

# The same shape of change, with a declared non-zero default for the added
# field: the host must migrate the column and fill the new field from the
# default, not zero. The sample system below prints the field once after the
# swap, which is what turns "filled from the default" into an assertion.
COMPONENT_DEFAULT_FIELD_EDIT = (
    "    [Range(0f, 1f)]\n    public float T;\n}",
    "    [Range(0f, 1f)]\n    public float T;\n\n    /// <summary>Added by the suite; its field default must survive the migration.</summary>\n    [EcsFieldDefault(7.5f)]\n    public float Extra;\n}",
)

# The print witness for COMPONENT_DEFAULT_FIELD_EDIT. A latch static and a
# body-only print line: neither touches a system's query signature, so the
# swap stays legal while the manifest change rides the parked path.
SYSTEM_EXTRA_LATCH_EDIT = (
    "public static class SplinePathSystem\n{\n    [System]",
    "public static class SplinePathSystem\n{\n    // Written by devops/tests/test_csharp_bridge.py while the suite runs:\n    // reports the migrated field default once per assembly.\n#pragma warning disable PILL0301\n    private static bool extraLogged;\n#pragma warning restore PILL0301\n\n    [System]",
)
SYSTEM_EXTRA_PRINT_EDIT = (
    "                ref var sample = ref sampleRow.SplineSample;",
    "                ref var sample = ref sampleRow.SplineSample;\n                if (!extraLogged)\n                {\n                    extraLogged = true;\n                    Console.WriteLine($\"[managed] sample extra={sample.Extra}\");\n                }",
)

# `7.5f` through `"R"` formatting; anything else means the added field was
# reset to zero instead of filled from its declared default.
SAMPLE_EXTRA_TOKEN = "[managed] sample extra=7.5"

# The retirement probes: a resource and a component declared only by the probe
# file and referenced by no query signature, so adding and removing them is a
# manifest-only change that rides the parked path exactly as a layout change
# does. The component is seeded with three rows, which is what makes its
# retirement visible as data going away rather than a declaration blink.
PROBE_RETIREMENT_RESOURCE_ADD = (
    "public static class ModuleSplineBridgeDemo\n{",
    "/// <summary>Declared, then retired, by devops/tests/test_csharp_bridge.py.</summary>\n"
    "[EcsResource]\n"
    "public struct ProbeSettings\n"
    "{\n"
    "    public float Gain;\n"
    "}\n\n"
    "public static class ModuleSplineBridgeDemo\n{",
)
PROBE_RETIREMENT_MARKER_ADD = (
    "public static class ModuleSplineBridgeDemo\n{",
    "/// <summary>Seeded, then retired, by devops/tests/test_csharp_bridge.py.</summary>\n"
    "public struct ProbeMarker\n"
    "{\n"
    "    public float Value;\n"
    "}\n\n"
    "public static class ModuleSplineBridgeDemo\n{",
)
PROBE_RETIREMENT_MARKER_SEED = (
    "        if (!_seeded)\n        {\n            _seeded = true;",
    "        if (!_seeded)\n        {\n            _seeded = true;\n"
    "            for (int index = 0; index < 3; index++)\n"
    "                commands.CreateEntity().With(new ProbeMarker { Value = index }).Build();\n"
    '            Console.WriteLine("[managed] probe retirement markers seeded");',
)

MARKERS_SEEDED_TOKEN = "probe retirement markers seeded"
# The retirement logs: three rows go with the component, and the unclaimed
# resource is dropped outright. Both lines come from the shared manifest
# pipeline and carry structured `kind`/`entries`/`touched` fields.
COMPONENT_RETIREMENT_TOKEN = 'kind="component" entries=1 touched=3'
RESOURCE_RETIREMENT_TOKEN = 'kind="resource" entries=1 touched=1'

# Renames an [System] method of the probe file. Rust builds its execution
# graph once at startup from these names, so the set has to stay stable until a
# restart.
SYSTEM_NAME_EDIT = (
    "public static void Run(Query<Read<global::pill_spline.Spline>> query, Commands commands)",
    "public static void Renamed(Query<Read<global::pill_spline.Spline>> query, Commands commands)",
)

# The host's log line for a re-registration, which is the only thing that tells
# a rebuilt scheduler apart from a behaviour-only swap in the output.
CSHARP_SYSTEMS_REREGISTERED_TOKEN = (
    "re-registered the project's managed systems after a signature change"
)

# Renames an [Startup] method. Startups are not re-run on reload, so a change
# to the set is refused rather than silently ignored.
STARTUP_NAME_EDIT = (
    "public static void Start(Commands commands)",
    "public static void StartRenamed(Commands commands)",
)

# Behavior-only edits chained so each phase is a distinct change to the same
# line. Used to reload repeatedly in one session.
BRIDGE_PROBE_V2_TO_V3_EDIT = (
    "cs spline bridge v2: sees {visibleSplines} spline(s), ",
    "cs spline bridge v3: sees {visibleSplines} spline(s), ",
)
BRIDGE_PROBE_V3_TO_V4_EDIT = (
    "cs spline bridge v3: sees {visibleSplines} spline(s), ",
    "cs spline bridge v4: sees {visibleSplines} spline(s), ",
)

BRIDGE_PROBE_V3_PREFIX = "cs spline bridge v3: sees"
BRIDGE_PROBE_V4_PREFIX = "cs spline bridge v4: sees"

# The schedule probes (ScheduleProbes.cs). The ordered pair shares a step
# counter and their type names sort the opposite way to their Order values, so
# the logged sequence can only come from the declared order; the post-project
# probe counts the marker rows the project-phase probe queued in the same
# frame, which only the second flush and the declared phase make equal to the
# frame number.
CSHARP_SCHEDULE_ORDER_TOKEN = "[probe] schedule step=1 order=480"
CSHARP_SCHEDULE_ORDER_SECOND_TOKEN = "[probe] schedule step=2 order=520"
CSHARP_SCHEDULE_POST_PROJECT_TOKEN = "[probe] schedule post_project frame=2 markers=1"
# The same ordered pair after the suite moves the first system's order past the
# second's, which also proves an edited order reaches the scheduler through a
# re-registration rather than being ignored by the reload signature.
CSHARP_SCHEDULE_REORDERED_TOKEN = "[probe] schedule step=2 order=999"

# A behaviour-only change to the probe file, so the scenario gets a fresh
# generation that logs the schedule probes with their declared order before the
# order is edited.
SCHEDULE_PROBE_NOOP_EDIT = (
    "/// <summary>Runs first by its declared order although its type name sorts last.</summary>",
    "/// <summary>Runs first by its declared order, although its type name sorts last.</summary>",
)

# Moves the first system's declared order past the second one's. The constant
# feeds both the attribute and the probe's log text, so one edit changes the
# declaration and what the output says about it.
SCHEDULE_ORDER_EDIT = (
    "internal const int First = 480;",
    "internal const int First = 999;",
)


def verify_managed_reload_report(scenario_name: str, output: str) -> bool:
    """Asserts a managed swap printed the shared reload report block.

    The block has to name the project and carry the managed phase mapping:
    `kind=reload` (it is a reload of the project subject) with zero `init` and
    `migrate`, which are the two phases the loader has no equivalent of. A
    block that only appears for a module reload does not prove anything.
    """
    lines = [line for line in output.splitlines() if CSHARP_RELOAD_REPORT_TOKEN in line]
    if not lines:
        print(f"  [FAIL] {scenario_name}: no {CSHARP_RELOAD_REPORT_TOKEN!r} line")
        return False
    matching = [
        line
        for line in lines
        if "kind=reload" in line and "init=0.0" in line and "migrate=0.0" in line
    ]
    if not matching:
        print(
            f"  [FAIL] {scenario_name}: the reload report does not describe the "
            "managed swap (expected kind=reload with init=0.0 and migrate=0.0):"
        )
        for line in lines:
            print(f"    {line}")
        return False
    print(f"  [OK] {scenario_name}: the managed swap is in the shared reload report.")
    return True


# The engine formatter's line prefix: the local time in brackets, to the
# millisecond. A bare console write carries no such prefix.
ENGINE_LINE_PREFIX = re.compile(r"^\[\d{2}:\d{2}:\d{2}:\d{3}\]")


def verify_loader_lines_are_engine_lines(scenario_name: str, output: str) -> bool:
    """Asserts the loader's reload lines came through the engine formatter.

    The text is the same either way; the prefix is not. A line without the
    engine's timestamp came straight from `Console`, which is what put the
    loader's reload reporting outside the one log the host writes.
    """
    lines = [line for line in output.splitlines() if CSHARP_RELOADED_TOKEN in line]
    if not lines:
        print(f"  [FAIL] {scenario_name}: no {CSHARP_RELOADED_TOKEN!r} line")
        return False
    bare = [line for line in lines if not ENGINE_LINE_PREFIX.match(line)]
    if bare:
        print(f"  [FAIL] {scenario_name}: loader lines written outside the engine log:")
        for line in bare:
            print(f"    {line}")
        return False
    print(f"  [OK] {scenario_name}: the loader's reload lines carry the engine prefix.")
    return True


def rejection_scenario(name, path, edit, reason, still_running_prefix):
    """One scenario asserting the loader refuses a contract-breaking change.

    Three things have to hold, and each has been wrong at some point in a system
    like this: the reload is refused, the refusal names the contract that was
    broken, and the assembly that was already running keeps running. The third
    is what `alive_tokens` exists for - the probe line was streaming before the
    edit, so only an occurrence AFTER the refusal proves anything.
    """
    return Scenario(
        name=name,
        phases=[
            ScenarioPhase(
                edits=[(path, [edit])],
                wait_token=CSHARP_RELOAD_REJECTED_TOKEN,
                required_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    CSHARP_LOAD_FAILED_TOKEN,
                    reason,
                ],
                forbidden_tokens=[
                    # The swap must NOT have happened.
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                alive_tokens=[(still_running_prefix, PROBE_TIMEOUT)],
            )
        ],
        restore_after=[path],
    )


SESSION_SCENARIOS = [
    Scenario(
        name="csharp_hot_reload",
        phases=[
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [BRIDGE_PROBE_PREFIX_EDIT])],
                wait_token=CSHARP_RELOADED_TOKEN,
                required_tokens=[
                    CSHARP_RELOADED_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                    # The swap is recorded in the shared report, in the same
                    # block and grammar a Rust reload prints.
                    CSHARP_RELOAD_REPORT_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    # The private timing line the shared report replaced. It is
                    # written inline rather than as a `*_TOKEN`: the log
                    # contract requires every declared token to exist in Rust,
                    # and this one must not exist anywhere.
                    "C# hot reload timing",
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                # After the swap the new assembly seeds one more spline (its
                # static re-initializes) on top of the module's and the
                # pre-reload C# spline, which both survive: 1 + 1 + 1 = 3.
                wait_after=[
                    (f"{BRIDGE_PROBE_V2_PREFIX} 3 spline(s)", PROBE_TIMEOUT),
                ],
            )
        ],
        validators=[verify_managed_reload_report, verify_loader_lines_are_engine_lines],
        restore_after=[MANAGED_PROJECT_PROBE_CS],
    ),
    # Reloading repeatedly in one session. Each phase is a distinct edit to the
    # same line, so every swap is real work rather than a no-op rebuild.
    #
    # What this is for: every accepted reload calls `oldContext.Unload()` on a
    # collectible AssemblyLoadContext. If a context were ever retained - by a
    # cached delegate, a static, or a live reference into the old assembly - the
    # unload would not complete and versions would accumulate silently. A swap
    # that still works on the third consecutive reload, with the world's entity
    # count intact, is the observable end of that.
    Scenario(
        name="csharp_repeated_reload_stability",
        phases=[
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [BRIDGE_PROBE_PREFIX_EDIT])],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[CSHARP_RELOADED_TOKEN, CSHARP_RELOAD_LEDGER_PREFIX],
                forbidden_tokens=[CSHARP_RELOAD_REJECTED_TOKEN, PANIC_TOKEN,
                                  ACCESS_VIOLATION_TOKEN, CSHARP_RETIRED_SURVIVOR_TOKEN],
                alive_tokens=[(BRIDGE_PROBE_V2_PREFIX, PROBE_TIMEOUT)],
            ),
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [BRIDGE_PROBE_V2_TO_V3_EDIT])],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[CSHARP_RELOADED_TOKEN, CSHARP_RELOAD_LEDGER_PREFIX],
                forbidden_tokens=[CSHARP_RELOAD_REJECTED_TOKEN, PANIC_TOKEN,
                                  ACCESS_VIOLATION_TOKEN, CSHARP_RETIRED_SURVIVOR_TOKEN],
                alive_tokens=[(BRIDGE_PROBE_V3_PREFIX, PROBE_TIMEOUT)],
            ),
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [BRIDGE_PROBE_V3_TO_V4_EDIT])],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[CSHARP_RELOADED_TOKEN, CSHARP_RELOAD_LEDGER_PREFIX],
                forbidden_tokens=[CSHARP_RELOAD_REJECTED_TOKEN, PANIC_TOKEN,
                                  ACCESS_VIOLATION_TOKEN, CSHARP_RETIRED_SURVIVOR_TOKEN],
                alive_tokens=[(BRIDGE_PROBE_V4_PREFIX, PROBE_TIMEOUT)],
            ),
        ],
        restore_after=[MANAGED_PROJECT_PROBE_CS],
    ),
    # The three refusals. C# hot reload is behavior-only by design, and these
    # pin what "behavior-only" actually means at the boundary: each rejection
    # token must appear when the change it refuses arrives, not only stay
    # absent otherwise.
    # Adding a field to a C# component no longer needs a host restart: the
    # managed loader parks the change while the host migrates the descriptor
    # column's rows, applies the manifest to the running world, and only then
    # swaps the assembly - so the assertion is a completed reload rather than a
    # refusal.
    Scenario(
        name="csharp_migrates_component_layout_change",
        phases=[
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_COMPONENTS_CS, [COMPONENT_LAYOUT_EDIT])],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_MANIFEST_PENDING_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                ],
                forbidden_tokens=[
                    # The old refusal must be gone, and nothing may fault while
                    # the world takes the new layout.
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    CSHARP_REJECT_COMPONENT_REASON,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                # The probe keeps streaming afterwards, which is what proves the
                # migrated world is still the one the systems are running on.
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            )
        ],
        restore_after=[MANAGED_PROJECT_COMPONENTS_CS],
    ),
    # A field added with a declared default starts from that value instead of
    # zero, and the next generation sees it: the sample system prints the
    # migrated field once after the swap. The two Systems.cs edits are a latch
    # static and a body-only print, so the parked manifest stays the only
    # contract change the swap carries.
    Scenario(
        name="csharp_migrates_field_default",
        phases=[
            ScenarioPhase(
                edits=[
                    (MANAGED_PROJECT_COMPONENTS_CS, [COMPONENT_DEFAULT_FIELD_EDIT]),
                    (
                        MANAGED_PROJECT_SYSTEMS_CS,
                        [SYSTEM_EXTRA_LATCH_EDIT, SYSTEM_EXTRA_PRINT_EDIT],
                    ),
                ],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_MANIFEST_PENDING_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    CSHARP_REJECT_COMPONENT_REASON,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                # The value witness: 7.5 can only appear if the added field was
                # filled from its declared default; zero would print 0.
                wait_after=[(SAMPLE_EXTRA_TOKEN, PROBE_TIMEOUT)],
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            )
        ],
        restore_after=[MANAGED_PROJECT_COMPONENTS_CS, MANAGED_PROJECT_SYSTEMS_CS],
    ),
    # Retirement: a declaration the manifest stops naming is not a refusal
    # anymore. A deliberately unreferenced component (seeded with three rows)
    # and a deliberately unreferenced resource are declared, then removed, and
    # the host's own retirement lines say what went: three rows with the
    # component, the resource dropped outright. Both probes are referenced by
    # no query signature, so every phase here is a manifest-only change.
    Scenario(
        name="csharp_retires_vanished_declarations",
        phases=[
            ScenarioPhase(
                edits=[
                    (
                        MANAGED_PROJECT_PROBE_CS,
                        [
                            PROBE_RETIREMENT_RESOURCE_ADD,
                            PROBE_RETIREMENT_MARKER_ADD,
                            PROBE_RETIREMENT_MARKER_SEED,
                        ],
                    ),
                ],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_MANIFEST_PENDING_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                # The new assembly's first frame seeds the rows the next phase
                # retires; seeing this proves the registration landed.
                wait_after=[(MARKERS_SEEDED_TOKEN, PROBE_TIMEOUT)],
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            ),
            ScenarioPhase(
                edits=[
                    (
                        MANAGED_PROJECT_PROBE_CS,
                        [
                            (
                                PROBE_RETIREMENT_MARKER_SEED[1],
                                PROBE_RETIREMENT_MARKER_SEED[0],
                            ),
                            (
                                PROBE_RETIREMENT_MARKER_ADD[1],
                                PROBE_RETIREMENT_MARKER_ADD[0],
                            ),
                        ],
                    ),
                ],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_MANIFEST_PENDING_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                    COMPONENT_RETIREMENT_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    CSHARP_REJECT_COMPONENT_REASON,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            ),
            ScenarioPhase(
                edits=[
                    (
                        MANAGED_PROJECT_PROBE_CS,
                        [
                            (
                                PROBE_RETIREMENT_RESOURCE_ADD[1],
                                PROBE_RETIREMENT_RESOURCE_ADD[0],
                            ),
                        ],
                    ),
                ],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_MANIFEST_PENDING_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                    RESOURCE_RETIREMENT_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    CSHARP_REJECT_COMPONENT_REASON,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            ),
        ],
        restore_after=[MANAGED_PROJECT_PROBE_CS],
    ),
    # Renaming a system changes the metadata the scheduler was built from, and
    # the host rebuilds it: it clears the project's systems and re-registers
    # them from the arriving assembly. Three things have to hold, and the third
    # is the one a rebuild can plausibly get wrong - the reload completes, the
    # host says it re-registered, and the renamed system is still running
    # afterwards.
    Scenario(
        name="csharp_reregisters_on_system_signature_change",
        phases=[
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [SYSTEM_NAME_EDIT])],
                wait_token=CSHARP_SYSTEMS_REREGISTERED_TOKEN,
                required_tokens=[
                    CSHARP_SYSTEMS_REREGISTERED_TOKEN,
                    CSHARP_RELOAD_COMPLETE_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                # The renamed system keeps printing, which is what proves the
                # re-registration produced a running system and not just a
                # cleared one.
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            )
        ],
        restore_after=[MANAGED_PROJECT_PROBE_CS],
    ),
    rejection_scenario(
        "csharp_rejects_startup_change",
        MANAGED_PROJECT_PROBE_CS,
        STARTUP_NAME_EDIT,
        CSHARP_REJECT_STARTUP_REASON,
        BRIDGE_PROBE_PREFIX,
    ),
    # The [System] schedule values reach the engine: the declared order
    # sequences the conflicting probe pair (their type names sort the opposite
    # way), the post-project probe sees the marker queued in the same frame
    # (only the second flush and the declared phase give it that), and an
    # edited order re-registers the systems and moves the pair.
    Scenario(
        name="csharp_schedule_values",
        phases=[
            # A behaviour-only reload gets a fresh generation, which logs the
            # declared schedule: the conflicting pair's sequence and the
            # post-project count the project phase's flush.
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [SCHEDULE_PROBE_NOOP_EDIT])],
                wait_token=CSHARP_RELOAD_COMPLETE_TOKEN,
                required_tokens=[
                    CSHARP_SCHEDULE_ORDER_TOKEN,
                    CSHARP_SCHEDULE_ORDER_SECOND_TOKEN,
                    CSHARP_SCHEDULE_POST_PROJECT_TOKEN,
                ],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
            ),
            # A changed order is a changed schedule, so the reload re-registers
            # the systems and the pair moves.
            ScenarioPhase(
                edits=[(MANAGED_PROJECT_PROBE_CS, [SCHEDULE_ORDER_EDIT])],
                wait_token=CSHARP_SYSTEMS_REREGISTERED_TOKEN,
                required_tokens=[CSHARP_SYSTEMS_REREGISTERED_TOKEN],
                wait_after=[(CSHARP_SCHEDULE_REORDERED_TOKEN, PROBE_TIMEOUT)],
                forbidden_tokens=[
                    CSHARP_RELOAD_REJECTED_TOKEN,
                    PANIC_TOKEN,
                    ACCESS_VIOLATION_TOKEN,
                ],
                alive_tokens=[(BRIDGE_PROBE_PREFIX, PROBE_TIMEOUT)],
            ),
        ],
        restore_after=[MANAGED_PROJECT_PROBE_CS],
    ),
    # Adding a mirrored method to a module WHILE the host runs must reach C#
    # without a restart: the module reload regenerates the mirror file, queues
    # a C# project rebuild, and the swapped assembly resolves the new
    # trampoline. Phase 1 adds get_e to pill_spline and waits for the cascade;
    # Phase 2 calls GetE() from the probe file and waits for its value to print -
    # which only compiles because Phase 1's mirror regeneration put GetE() in
    # the file the C# build compiles.
    Scenario(
        name="csharp_mirror_live_regeneration",
        phases=[
            ScenarioPhase(
                edits=[(
                    SPLINE_LIB_RS,
                    [
                        (
                            """    #[pill_mirror_method]
    pub fn get_d(&self, alpha: i32, beta: i32) -> i32 {
        alpha + beta
    }
}""",
                            """    #[pill_mirror_method]
    pub fn get_d(&self, alpha: i32, beta: i32) -> i32 {
        alpha + beta
    }

    /// Live-added while the host ran; the mirror must regenerate without a restart.
    #[pill_mirror_method]
    pub fn get_e(&self) -> u64 {
        self.x + 9000
    }
}""",
                        )
                    ],
                )],
                wait_token=CSHARP_RELOADED_TOKEN,
                required_tokens=[
                    CSHARP_RELOADED_TOKEN,
                    CSHARP_MIRROR_REGEN_TOKEN,
                ],
                forbidden_tokens=[CSHARP_RELOAD_REJECTED_TOKEN, PANIC_TOKEN,
                                  ACCESS_VIOLATION_TOKEN],
            ),
            ScenarioPhase(
                edits=[(
                    MANAGED_PROJECT_PROBE_CS,
                    [
                        (
                            "ulong omoB = omo.GetB();",
                            "ulong omoB = omo.GetB();\n        ulong omoE = omo.GetE();",
                        ),
                        (
                            'b={omoB} alpha={alpha}");',
                            'b={omoB} e={omoE} alpha={alpha}");',
                        ),
                    ],
                )],
                wait_token=OMO_PROBE_WITH_E,
                required_tokens=[
                    OMO_PROBE_WITH_E,
                    CSHARP_RELOADED_TOKEN,
                ],
                forbidden_tokens=[CSHARP_RELOAD_REJECTED_TOKEN, PANIC_TOKEN,
                                  ACCESS_VIOLATION_TOKEN],
            ),
        ],
        # Restore the probe first so no build references GetE after the module
        # reverts (a transient reference would fail the intermediate build).
        restore_after=[MANAGED_PROJECT_PROBE_CS, SPLINE_LIB_RS],
    ),
]

# =============================================================================
# Host session
# =============================================================================


# This suite exercises the C# assembly-swap reload, so it pins the reload-only
# posture to keep the Rust patch fast path out of the host it drives.
#
# `rendering` is on because `managed` queries the renderer's own components:
# the managed manifest marks every type the runtime assembly declares as
# shared, and a shared component with no native binding is refused at startup.
# A headless host has no binding for `TracyLive.MeshRendererComponent` and dies
# there, which is what this suite used to do before the feature was pinned.
HOST_POSTURE = HOST_DEV_RENDERING


def write_bridge_probe() -> None:
    """Writes the suite's probe file into the managed project."""
    atomic_write(MANAGED_PROJECT_PROBE_CS, BRIDGE_PROBE_SOURCE)
    print(f"  [PREP] Wrote {MANAGED_PROJECT_PROBE_CS.relative_to(WORKSPACE_ROOT)}.")


def launch_host():
    """Launches the standalone host exe with PROJECT_PATH pinned to the C# project."""
    environment = os.environ.copy()
    environment["PROJECT_PATH"] = MANAGED_PROJECT_PATH
    return launch_process([str(HOST_EXE)], MODULES_ROOT, environment)


def verify_unload_ledger(session_output: str) -> bool:
    """Asserts every accepted reload reported a healthy unload ledger.

    A collectible context unloads only once nothing roots it, and
    `AssemblyLoadContext.Unload` cannot await that: a static holding a
    delegate, an event handler, a cached Type, a thread-pool callback or a
    thread-static in the old assembly keeps the whole version alive with no
    symptom but memory growth. The reload line therefore carries
    `retired N, collected M` from the weak-reference sweep that ran just
    before it. At the moment of a reload the context that reload just
    unloaded has not been observed by a sweep yet, so a healthy ledger is
    `M <= N <= M + 1` and never goes backwards; a rooted version prints its
    own stderr survivor warning, which is forbidden outright.
    """
    matches = CSHARP_RELOAD_LEDGER_PATTERN.findall(session_output)
    if not matches:
        print("  [FAIL] No reload line carried the unload ledger; the check never ran.")
        return False
    previous_retired = 0
    for retired_text, collected_text in matches:
        retired, collected = int(retired_text), int(collected_text)
        if not collected <= retired <= collected + 1:
            print(
                f"  [FAIL] Unload ledger out of step: retired {retired}, "
                f"collected {collected}; retired assemblies are accumulating."
            )
            return False
        if retired < previous_retired:
            print(f"  [FAIL] Unload ledger went backwards: retired {retired} "
                  f"after {previous_retired}.")
            return False
        previous_retired = retired
    if CSHARP_RETIRED_SURVIVOR_TOKEN in session_output:
        print("  [FAIL] A retired project assembly is still loaded; see the "
              "survivor warning in the host output.")
        return False
    print(f"  [OK] Unload ledger healthy across {len(matches)} reload line(s).")
    return True


def csharp_runner() -> ScenarioRunner:
    """The runner settings for this suite.

    Built per session so it picks up the timeouts `main()` scaled from
    `--timeout-scale`. A C# restore swaps an assembly and settles with the
    runtime's own reload line, whichever file was put back.
    """
    return ScenarioRunner(
        reload_timeout=RELOAD_TIMEOUT,
        settle_timeout=SETTLE_TIMEOUT,
        stability_sleep=STABILITY_SLEEP,
        restore_settle_token=lambda _path: CSHARP_RELOADED_TOKEN,
    )


def run_csharp_session() -> bool:
    """Writes the C# project's settings, launches the host, and runs the scenarios."""
    print("\n  [TEST] Launching standalone host with managed...")
    write_project_settings(MANAGED_PROJECT_ROOT, CSHARP_SETTINGS)
    write_bridge_probe()
    process, monitor = launch_host()
    session_passed = True

    try:
        if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
            print("  [FAIL] Host did not reach the project loop in time.")
            print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
            return False
        print("  [OK] Host started and entered the project loop.")

        startup_output = monitor.output_since(0)
        if not verify_startup(monitor, startup_output):
            return False

        for scenario in SESSION_SCENARIOS:
            if not run_scenario(scenario, monitor, csharp_runner()):
                session_passed = False
                break

        if session_passed and not verify_unload_ledger(monitor.output_since(0)):
            session_passed = False

        if session_passed:
            print("\n  [PASS] C# session completed.")
    finally:
        terminate_process(process, monitor)

    return session_passed


def verify_codegen_rebuild() -> bool:
    """Restart the host with the mirror deleted and verify regeneration.

    After the session every artifact is current. Deleting the generated mirror
    and restarting must make the host regenerate it from the module's real
    layout (missing-file path) before the managed project builds, and the
    bridge must still work afterwards. The regenerated copy is also compared
    byte-for-byte with the tracked one, because the mirror is committed for
    standalone C# builds (see devops/tests/README.md, "Generated mirrors").
    """
    print("\n  [TEST] Codegen rebuild (mirror deleted, host restarts)...")
    if SPLINE_GENERATED_FILE.exists():
        SPLINE_GENERATED_FILE.unlink()
        print(f"  [PREP] Deleted {SPLINE_GENERATED_FILE.name}.")

    process, monitor = launch_host()
    try:
        if not monitor.wait_for(STARTUP_TOKEN, STARTUP_TIMEOUT):
            print("  [FAIL] Codegen-rebuild restart did not reach the project loop.")
            print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
            return False
        startup_output = monitor.output_since(0)
        if has_crash_signals(startup_output):
            print("  [FAIL] Crash signals during codegen-rebuild restart.")
            print(f"  Output tail:\n{startup_output[-1600:]}")
            return False
        # The mirror must exist again with the exact expected content.
        if not verify_generated_mirror(
            SPLINE_GENERATED_FILE,
            [
                "namespace pill_spline {",
                "public partial struct Spline",
                "Size = 588",
                "[FieldOffset(192)] public uint ControlPointCount;",
                "[FieldOffset(196)] public float Elo;",
                "[FieldOffset(200)] public Vector3f InTangents0;",
                "[FieldOffset(392)] public Vector3f OutTangents0;",
                "[FieldOffset(584)] public byte TangentMode;",
                "[StructLayout(LayoutKind.Explicit, Size = 16)]\npublic partial struct OmoMO",
                "public ulong GetSum()",
                "var __call = global::TracyLive.MirrorCall.Begin(\"pill_spline::OmoMO\", \"get_sum\");",
            ],
            should_exist=True,
            description="regenerated pill_spline mirror",
        ):
            return False
        # Byte-for-byte against the tracked bootstrap: the mirror is committed
        # so standalone C# builds compile without a host run, and a tracked copy
        # the host would no longer generate is a file no build ever checks.
        if COMMITTED_MIRROR_BYTES is not None and (
            SPLINE_GENERATED_FILE.read_bytes() != COMMITTED_MIRROR_BYTES
        ):
            print(
                "  [FAIL] regenerated mirror differs from the committed bootstrap; "
                "run the host once and commit "
                f"{SPLINE_GENERATED_FILE.relative_to(WORKSPACE_ROOT)}"
            )
            return False
        print("  [OK] Regenerated mirror matches the committed bootstrap.")
        # And the bridge must be live again (Rust -> C# direction).
        if not monitor.wait_for(
            BRIDGE_PROBE_RUST_TO_CS_TOKEN,
            PROBE_TIMEOUT,
        ):
            print("  [FAIL] Bridge probe missing after codegen rebuild.")
            print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
            return False
        # The mirrored Rust method must actually run: GetSum() returns 46 for
        # the C#-seeded OmoMO (X=12, Y=34), plus GetA/GetB.
        if not monitor.wait_for(OMO_PROBE_SUM_A_B, PROBE_TIMEOUT):
            print("  [FAIL] Mirrored OmoMO probe missing after codegen rebuild.")
            print(f"  Output tail:\n{monitor.output_since(0)[-1600:]}")
            return False
        print("  [OK] Codegen rebuild: mirror regenerated and bridge live.")
        return True
    finally:
        terminate_process(process, monitor)


# =============================================================================
# Main
# =============================================================================


def main() -> None:
    """Parses arguments, runs the C# session and the codegen-rebuild check."""
    parser = argparse.ArgumentParser(
        description="C# <-> Rust bridge integration suite for Rust-Hybrid-ECS"
    )
    add_common_suite_arguments(parser)
    args = parser.parse_args()

    if args.timeout_scale <= 0:
        print("ERROR: --timeout-scale must be > 0")
        sys.exit(1)

    global STARTUP_TIMEOUT, RELOAD_TIMEOUT, PROBE_TIMEOUT, SETTLE_TIMEOUT
    global STABILITY_SLEEP, BUILD_TIMEOUT

    scale = args.timeout_scale
    STARTUP_TIMEOUT = int(STARTUP_TIMEOUT * scale)
    RELOAD_TIMEOUT = int(RELOAD_TIMEOUT * scale)
    PROBE_TIMEOUT = int(PROBE_TIMEOUT * scale)
    SETTLE_TIMEOUT = int(SETTLE_TIMEOUT * scale)
    STABILITY_SLEEP = max(1, int(STABILITY_SLEEP * scale))
    BUILD_TIMEOUT = int(BUILD_TIMEOUT * scale)

    ensure_host_lock()
    kill_stale_hosts()

    # Capture originals for every file the suite may touch, including the
    # committed bootstrap mirror that the codegen-rebuild check deletes. The
    # probe file is not captured here: it does not exist yet, and the scenarios
    # that edit it capture it themselves.
    for path in (
        project_settings_yaml(MANAGED_PROJECT_ROOT),
        MANAGED_PROJECT_COMPONENTS_CS,
        SPLINE_GENERATED_FILE,
        DUMMY_GENERATED_FILE,
    ):
        BACKUP.capture(path)

    # Record the tracked mirror's bytes before any host run can normalize it;
    # the codegen-rebuild check compares the host's fresh output against this
    # copy instead of re-reading the file later, when the first session has
    # already rewritten it.
    global COMMITTED_MIRROR_BYTES
    if SPLINE_GENERATED_FILE.exists():
        COMMITTED_MIRROR_BYTES = SPLINE_GENERATED_FILE.read_bytes()

    passed = True
    try:
        if not prepare_host(HOST_POSTURE, args, BUILD_TIMEOUT):
            sys.exit(1)
        if not run_csharp_session():
            passed = False
        if passed:
            if not verify_codegen_rebuild():
                passed = False
    finally:
        # Always restore the developer's files, even on failure.
        BACKUP.restore_all()
        # Restore the C# project's settings file last.
        csharp_settings_path = project_settings_yaml(MANAGED_PROJECT_ROOT)
        if BACKUP.captured(csharp_settings_path):
            BACKUP.restore_one(csharp_settings_path)
        # The probe file belongs to the suite, so it is removed rather than
        # restored: `restore_all` put its captured content back a moment ago.
        if MANAGED_PROJECT_PROBE_CS.exists():
            MANAGED_PROJECT_PROBE_CS.unlink()
            print(
                "  [CLEANUP] removed "
                f"{MANAGED_PROJECT_PROBE_CS.relative_to(WORKSPACE_ROOT)}"
            )

    print(f"\n{'=' * 64}")
    print("  SUMMARY")
    print(f"{'=' * 64}")
    if passed:
        print("  [PASS] C# <-> Rust bridge suite passed.")
        sys.exit(0)
    print("  [FAIL] C# <-> Rust bridge suite failed.")
    sys.exit(1)


if __name__ == "__main__":
    run_suite_with_timing(main)
