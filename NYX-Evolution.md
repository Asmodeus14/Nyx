Nyx Compositor Evolution Plan
=============================

This roadmap transforms your current implementation into a professional-grade windowing environment.

PhaseFocusKey Technical Objective**1**ArchitectureDecouple the compositor state from the main event loop.**2**InteractivityImplement Hit-Testing for Resizing and Window Controls.**3**ComponentizationCreate a Polymorphic Widget System (Buttons, Sliders).**4**Layout EngineImplement Automated Tiling and Animation/Tweening.

Phase 1: Architectural Decoupling
---------------------------------

Currently, nyx-user/src/main.rs is a monolithic loop. You must isolate the state to make adding features like resizing manageable.

1.  **Extract State:** Create a CompositorState struct.
    
    *   Move clients, dirty\_rects, mouse\_pos, and focused\_window\_idx into this struct.
        
2.  **Define the Interface:** Split your main loop into three clear stages:
    
    *   **Input/Event Handler:** Process IPC messages and mouse inputs, updating the CompositorState.
        
    *   **Update/Logic:** Handle window resizing calculations and focus state changes.
        
    *   **Render:** A dedicated function that iterates over CompositorState and draws to the buffer.
        

Phase 2: Resizing and Window Controls
-------------------------------------

To implement interactive resizing and buttons (Close, Min, Max), you must implement **Hit-Testing**.

### 2.1 Resizing Logic

Modify your main.rs mouse engine to detect the "resize zone."

1.  **Hit-Test Region:** Define a 10px margin at the bottom-right of every window.
    
2.  **State Flags:** Add a is\_resizing: bool and resizing\_win\_id: Option to your CompositorState.
    
3.  Rust// In mouse event processing:if left\_click && is\_near\_edge(mx, my, client) { state.resizing\_win\_id = Some(idx);}// If resizing, update window dimensions:if let Some(idx) = state.resizing\_win\_id { clients\[idx\].win.w = mx - clients\[idx\].win.x; clients\[idx\].win.h = my - clients\[idx\].win.y;}
    

### 2.2 Window Control Buttons

Instead of hardcoding, treat the title bar as a container for buttons.

1.  **Struct Update:** Update the Window struct in nyx-gui/src/ui.rs to include a is\_minimized and is\_maximized flag.
    
2.  **Hit-Testing:** In the title bar region (top 30px), define three distinct hit-zones:
    
    *   **Close (X):** \[x+10, y+10\]
        
    *   **Minimize (-):** \[x+25, y+10\]
        
    *   **Maximize (□):** \[x+40, y+10\]
        
3.  **Action Routing:** When a click is detected in these zones, send the corresponding IPC message to the owner PID (e.g., MSG\_WINDOW\_CLOSE or a new MSG\_WINDOW\_STATE\_CHANGE).
    

Phase 3: Modular Widget Toolkit
-------------------------------

To avoid cluttering the compositor with drawing logic for every new button, move to a trait-based system.

1.  Rustpub trait Widget { fn draw(&self, canvas: &mut Canvas); fn handle\_event(&mut self, mx: usize, my: usize, clicked: bool) -> bool;}
    
2.  **Registration:** Add a Vec\> to your WindowClient struct in nyx-user/src/main.rs.
    
3.  **Dynamic Rendering:** Your render loop now simply calls widget.draw() for all widgets assigned to that window.
    

Phase 4: Automated Tiling & Layout
----------------------------------

Finally, implement the "Wayland-style" tiling engine.

1.  **Reflow Trigger:** In your IPC handler, call recalculate\_layout() whenever MSG\_REQ\_WINDOW or MSG\_WINDOW\_CLOSE is processed.
    
2.  **Grid Engine:** \* Calculate cols = sqrt(count).
    
    *   Distribute win.w and win.h as screen\_width / cols and screen\_height / rows.
        
3.  **Animation:** Use the transition\_timer (from Phase 2 of our prior plan) to interpolate windows from their old (x, y) to their new (new\_x, new\_y). This prevents the "teleportation" effect when a window is added to the grid.
    

### Summary of Component Dependencies

FeaturePrimary File(s)Logic/Dependency**Resizing**nyx-user/src/main.rsHit-detection math + mouse move events.**Buttons**nyx-gui/src/ui.rsDrawing primitives + IPC message passing.**Widgets**nyx-gui/src/ui.rsPolymorphic trait definitions.**Layout**nyx-user/src/main.rsrecalculate\_layout logic + Tweening.