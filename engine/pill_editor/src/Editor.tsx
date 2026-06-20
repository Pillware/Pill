import { useState, useRef, useEffect, useCallback, useContext, createContext } from "react";
import type { ReactElement, MutableRefObject } from "react";
import { Layout, Model, TabNode, IJsonModel, Actions, DockLocation } from "flexlayout-react";
import { invoke } from "@tauri-apps/api/core";
import "flexlayout-react/style/dark.css";
import "./editor-styles.css";

// ---------------------------------------------------------------------------
// Unity-like layout model
//
// Top row:   Hierarchy (20%) | Scene+Game tabs (60%) | Inspector (20%)
// Bottom row: Project+Console tabs (100%)
// ---------------------------------------------------------------------------
const layoutJson: IJsonModel = {
  global: {
    tabSetMinWidth: 100,
    tabSetMinHeight: 80,
    borderMinSize: 100,
    // Allow popout on all tabs by default; scene/game override to false below.
    tabEnablePopout: true,
    tabEnablePopoutIcon: true,
  },
  borders: [],
  layout: {
    type: "row",
    weight: 100,
    children: [
      // ── Main work area (top ~70%) ──────────────────────────────────────
      {
        type: "row",
        weight: 70,
        children: [
          // Left: Hierarchy
          {
            type: "tabset",
            weight: 20,
            children: [
              { type: "tab", name: "Hierarchy", component: "hierarchy" },
            ],
          },
          // Centre: Scene / Game views
          {
            type: "tabset",
            id: "scene-tabset",
            weight: 60,
            selected: 0,
            children: [
              { type: "tab", name: "Scene", component: "scene", config: { vpId: "scene-0" }, enablePopout: false },
              { type: "tab", name: "Game", component: "game", config: { vpId: "game-0" }, enablePopout: false },
            ],
          },
          // Right: Inspector
          {
            type: "tabset",
            weight: 20,
            children: [
              { type: "tab", name: "Inspector", component: "inspector" },
            ],
          },
        ],
      },
      // ── Bottom strip (~30%) ────────────────────────────────────────────
      {
        type: "tabset",
        weight: 30,
        children: [
          { type: "tab", name: "Project", component: "project" },
          { type: "tab", name: "Console", component: "console" },
        ],
      },
    ],
  },
};

// ---------------------------------------------------------------------------
// Viewport sync — bridges React tab bounds to native Wayland subsurfaces
// ---------------------------------------------------------------------------

/**
 * A Set of "measure" callbacks, one per mounted Scene/Game tab.
 * The Editor calls all of them from onModelChange so position changes
 * (caused by other panels resizing) are detected in addition to size changes.
 */
const ViewportSyncContext = createContext<MutableRefObject<Set<() => void>> | null>(null);

/**
 * Attach to a Scene or Game panel div.  On mount it calls `create_viewport`;
 * on subsequent size/position changes it calls `viewport_resize`.
 * A rAF throttle ensures at most one IPC round-trip per animation frame.
 */
function useViewportSync(id: string) {
  const elRef = useRef<HTMLDivElement>(null);
  const created = useRef(false);
  const rafId = useRef<number | null>(null);
  const ctx = useContext(ViewportSyncContext);

  const measure = useCallback(() => {
    if (rafId.current !== null) return; // already scheduled this frame
    rafId.current = requestAnimationFrame(() => {
      rafId.current = null;
      const el = elRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const args = {
        id,
        x: Math.round(r.x),
        y: Math.round(r.y),
        width: Math.round(r.width),
        height: Math.round(r.height),
      };
      if (!created.current) {
        created.current = true;
        invoke("create_viewport", args).catch(console.error);
      } else {
        invoke("viewport_resize", args).catch(console.error);
      }
    });
  }, [id]);

  // Register with the Editor so layout-driven position changes trigger us too.
  useEffect(() => {
    const set = ctx?.current;
    if (!set) return;
    set.add(measure);
    return () => { set.delete(measure); };
  }, [ctx, measure]);

  // ResizeObserver handles size changes within the tab.
  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    measure(); // initial measurement
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (rafId.current !== null) { cancelAnimationFrame(rafId.current); rafId.current = null; }
    };
  }, [measure]);

  return elRef;
}

// ---------------------------------------------------------------------------
// Panel components
// ---------------------------------------------------------------------------

function Hierarchy() {
  const items = ["Main Camera", "Directional Light", "Cube", "Sphere", "Plane", "Player", "Enemy"];
  return (
    <div className="panel panel-hierarchy">
      {items.map((item) => (
        <div key={item} className="hierarchy-item">{item}</div>
      ))}
    </div>
  );
}

function SceneView({ id }: { id: string }) {
  const ref = useViewportSync(id);
  return (
    <div ref={ref} className="panel panel-scene" style={{ width: "100%", height: "100%" }}>
      <div className="scene-label">Scene View</div>
      <div className="scene-grid" />
      <div className="scene-gizmo">
        <span>X</span><span>Y</span><span>Z</span>
      </div>
    </div>
  );
}

function GameView({ id }: { id: string }) {
  const ref = useViewportSync(id);
  return (
    <div ref={ref} className="panel panel-game" style={{ width: "100%", height: "100%" }}>
      <div className="scene-label">Game View — 16:9</div>
    </div>
  );
}

function Inspector() {
  const [pos, setPos] = useState({ x: "0", y: "1", z: "0" });
  return (
    <div className="panel panel-inspector">
      <div className="inspector-header">Transform</div>
      {(["x", "y", "z"] as const).map((axis) => (
        <div key={axis} className="inspector-row">
          <label>{axis.toUpperCase()}</label>
          <input
            value={pos[axis]}
            onChange={(e) => setPos((p) => ({ ...p, [axis]: e.target.value }))}
          />
        </div>
      ))}
      <div className="inspector-header" style={{ marginTop: 12 }}>Mesh Renderer</div>
      <div className="inspector-row"><label>Cast Shadows</label><input type="checkbox" defaultChecked /></div>
      <div className="inspector-row"><label>Material</label><span className="inspector-ref">Default-Material</span></div>
    </div>
  );
}

function ProjectBrowser() {
  const files = ["Assets/", "Scenes/", "Scripts/", "Materials/", "Prefabs/", "Textures/"];
  return (
    <div className="panel panel-project">
      {files.map((f) => (
        <div key={f} className="project-item">{f}</div>
      ))}
    </div>
  );
}

function Console() {
  const logs = [
    { level: "log", msg: "[Info]  Scene loaded successfully" },
    { level: "warn", msg: "[Warn]  Missing reference on Player" },
    { level: "error", msg: "[Error] NullReferenceException: Object not set" },
    { level: "log", msg: "[Info]  Compiled 3 scripts in 0.42s" },
  ];
  return (
    <div className="panel panel-console">
      {logs.map((l, i) => (
        <div key={i} className={`console-line console-${l.level}`}>{l.msg}</div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Component registry — maps tab `component` string → React element
// ---------------------------------------------------------------------------
const PANELS: Record<string, () => ReactElement> = {
  hierarchy: Hierarchy,
  inspector: Inspector,
  project: ProjectBrowser,
  console: Console,
};

// ---------------------------------------------------------------------------
// Editor root
// ---------------------------------------------------------------------------
const model = Model.fromJson(layoutJson);

export default function Editor() {
  const measureCallbacks = useRef(new Set<() => void>());
  const sceneCount = useRef(0);

  // ── Track popout layouts and close GTK windows when they vanish
  // ── from the model (flexlayout removes them silently on last-tab drag-out).
  const activePopouts = useRef(new Set<string>());
  useEffect(() => {
    const id = setInterval(() => {
      const current = new Set<string>();
      for (const [lid, lay] of model.getLayouts()) {
        if (lay.getLayoutId() !== Model.MAIN_LAYOUT_ID) {
          current.add(lid);
        }
      }
      for (const lid of activePopouts.current) {
        if (!current.has(lid)) {
          console.log("[auto-close] popout vanished:", lid);
          invoke("close_popup_window", { layoutId: lid }).catch(console.error);
        }
      }
      activePopouts.current = current;
    }, 300);
    return () => clearInterval(id);
  }, []);

  function addSceneTab() {
    sceneCount.current += 1;
    const vpId = `scene-${sceneCount.current}`;
    model.doAction(
      Actions.addNode(
        { type: "tab", name: `scene_${sceneCount.current}`, component: "scene", config: { vpId }, enablePopout: false },
        "scene-tabset",
        DockLocation.CENTER,
        -1,
      )
    );
  }

  function renderTab(node: TabNode): ReactElement {
    const component = node.getComponent() ?? "";
    const vpId: string =
      (node.getConfig() as { vpId?: string } | undefined)?.vpId
      ?? `${component}-${node.getId()}`;

    if (component === "scene") return <SceneView id={vpId} />;
    if (component === "game") return <GameView id={vpId} />;

    const Panel = PANELS[component];
    return Panel
      ? <Panel />
      : <div className="panel">Unknown panel: {component}</div>;
  }

  return (
    <ViewportSyncContext.Provider value={measureCallbacks}>
      <div className="editor-root">
        {/* ── Toolbar ─────────────────────────────────────────────────── */}
        <div className="editor-toolbar">
          <div className="toolbar-group">
            <button className="toolbar-btn">Hand</button>
            <button className="toolbar-btn">Move</button>
            <button className="toolbar-btn">Rotate</button>
            <button className="toolbar-btn">Scale</button>
          </div>
          <div className="toolbar-center">
            <button className="toolbar-btn play">▶ Play</button>
            <button className="toolbar-btn">⏸ Pause</button>
            <button className="toolbar-btn">⏭ Step</button>
          </div>
          <div className="toolbar-group">
            <button className="toolbar-btn">Layers</button>
            <button className="toolbar-btn">Layout</button>
            <button className="toolbar-btn" onClick={addSceneTab}>+ Scene</button>
          </div>
        </div>

        {/* ── FlexLayout canvas ───────────────────────────────────────── */}
        <div className="editor-layout">
          <Layout
            model={model}
            factory={renderTab}
            supportsPopout={true}
            popoutURL="popout.html"
            onModelChange={() => {
              measureCallbacks.current.forEach(fn => fn());
            }}

            onAction={(action) => {
              if (action.type === Actions.DELETE_TAB) {
                const node = model.getNodeById(action.data.node);
                if (node instanceof TabNode) {
                  const comp = node.getComponent();
                  if (comp === "scene" || comp === "game") {
                    const vpId =
                      (node.getConfig() as { vpId?: string } | undefined)?.vpId
                      ?? `${comp}-${node.getId()}`;
                    invoke("delete_viewport", { id: vpId }).catch(console.error);
                  }
                }
              }
              if (action.type === Actions.CLOSE_POPOUT) {
                invoke("close_popup_window", { layoutId: action.data.layoutId }).catch(console.error);
              }
              return action;
            }}
          />
        </div>
      </div>
    </ViewportSyncContext.Provider>
  );
}
