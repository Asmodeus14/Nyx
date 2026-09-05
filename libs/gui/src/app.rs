use alloc::string::String;
use alloc::vec::Vec;
use nyx_api::*;
use crate::canvas::Canvas;

pub trait NyxApp {
    fn title(&self) -> &str;
    fn initial_width(&self) -> usize { 640 }
    fn initial_height(&self) -> usize { 400 }
    
    /// Absolute path to this app's icon PNG, normally the `icon.png` in its own `.nyx` bundle. The
    /// compositor draws it on the window's taskbar button. Default: none, and the button falls back
    /// to the app's initial letter.
    fn icon_path(&self) -> &str { "" }

    fn init(&mut self) {}

    fn update(&mut self) -> bool { false }
    fn draw(&mut self, canvas: &mut Canvas);
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }

    /// The pointer moved over this window with no button held. Window-local pixels. Return true to
    /// redraw.
    ///
    /// Default `false`, which is what lets every existing app compile untouched: an app that does
    /// not implement this simply never learns about hover, exactly as before.
    ///
    /// Already coalesced — at most one call per tick, carrying the newest position. So it is safe to
    /// do real work here (a hit test, a layout query); what is not safe anywhere is returning `true`
    /// unconditionally, which repaints the window on every pointer movement.
    fn on_mouse_move(&mut self, _mx: usize, _my: usize) -> bool { false }

    /// The pointer left this window, or a shell surface took it. Clear any hover state and return
    /// true to redraw.
    ///
    /// An app that implements [`on_mouse_move`](Self::on_mouse_move) and not this one has a
    /// highlight that never switches off.
    fn on_mouse_leave(&mut self) -> bool { false }

    fn on_key(&mut self, _key: char) -> bool { false }

    /// Scrollbar (Boot B): opt-in. Return the app's FULL scrollable content height in px; `0` (default)
    /// means not scrollable → the compositor draws no scrollbar. Reported into the WindowHeader each frame.
    fn content_height(&self) -> usize { 0 }
    /// Scrollbar: the app's current top scroll offset in px (reported into the header for the thumb).
    fn scroll_offset(&self) -> usize { 0 }
    /// Scrollbar: the compositor moved the bar to `new_offset` (px, already clamped to the track). Apply
    /// it to the app's scroll state and return true to trigger a redraw. Default: ignore.
    fn on_scroll(&mut self, _new_offset: usize) -> bool { false }
}

pub fn run<T: NyxApp>(mut app: T) -> ! {
    const COMPOSITOR_PID: u64 = 4;
    
    let mut width = app.initial_width();
    let mut height = app.initial_height();
    
    let mut total_size = core::mem::size_of::<WindowHeader>() + (width * height * 4);
    let mut shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); }

    let mut buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let mut header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
    
    header.magic = WIN_MAGIC;
    header.requested_x = -1;
    header.requested_y = -1;
    header.width = width as u32;
    header.height = height as u32;
    header.flags = WIN_FLAG_NONE;
    
    let title_bytes = app.title().as_bytes();
    header.title.fill(0);
    let len = title_bytes.len().min(64);
    header.title[..len].copy_from_slice(&title_bytes[..len]);

    // The app declares its own icon: it knows its bundle path, the compositor doesn't (there is no
    // argv, and a pid says nothing about which binary it came from).
    let icon_bytes = app.icon_path().as_bytes();
    header.icon.fill(0);
    let ilen = icon_bytes.len().min(96);
    header.icon[..ilen].copy_from_slice(&icon_bytes[..ilen]);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { 
        if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } 
    }

    let mut pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    
    app.init();

    let mut needs_redraw = true;
    
    // 🚨 FIX: Track if we have a pending memory swap waiting for paint
    let mut pending_shm_swap: Option<u64> = None;

    // R1: buffers we've resized away from, awaiting the compositor's MSG_SHM_RELEASED before we free
    // them. Each entry is (shm_id, mapping_base_vaddr). We must NOT free a buffer while the compositor
    // still maps it (use-after-free of RAM it may reuse), so we hold it here until the ACK arrives.
    let mut pending_retire: Vec<(u64, u64)> = Vec::new();

    loop {
        let mut event_redraw = false;

        // U6: DRAIN the whole IPC queue each tick, coalescing resizes. A fast resize drag emits
        // MSG_WINDOW_RESIZED far faster than this 16 ms loop, so processing one message per tick made
        // the app trail the drag (allocating+painting through a stale backlog). Instead we consume every
        // queued message now and keep only the LATEST resize size, then reallocate ONCE below — the app
        // jumps straight to the current drag size in a single repaint.
        let mut pending_resize: Option<(usize, usize)> = None;
        // Hover, coalesced the same way and for the same reason. One slot rather than two, so that a
        // move followed by a leave stays a leave and a leave followed by a re-entry stays a move —
        // separate `Option`s would lose the ordering and strand the highlight on.
        //   Some(Some(pos)) = the pointer is here      Some(None) = it left      None = no news
        let mut pending_hover: Option<Option<(usize, usize)>> = None;
        while sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_WINDOW_CLOSE => sys_exit(0),
                MSG_WINDOW_RESIZED => {
                    // Coalesce: remember the newest size; don't allocate per message.
                    pending_resize = Some((msg.data1 as usize, msg.data2 as usize));
                },
                MSG_MOUSE_EVENT => {
                    event_redraw |= app.on_mouse(msg.data1 as usize, msg.data2 as usize, true);
                },
                MSG_MOUSE_MOVE => {
                    pending_hover = Some(Some((msg.data1 as usize, msg.data2 as usize)));
                },
                MSG_MOUSE_LEAVE => {
                    pending_hover = Some(None);
                },
                MSG_KEY_EVENT => {
                    if let Some(key) = core::char::from_u32(msg.data1 as u32) {
                        event_redraw |= app.on_key(key);
                    }
                },
                MSG_SCROLL => {
                    // Compositor moved our scrollbar → apply the new offset (data1) to scroll state.
                    event_redraw |= app.on_scroll(msg.data1 as usize);
                },
                MSG_SHM_RELEASED => {
                    // R1: the compositor released its mapping of an old resized-away buffer (data1 =
                    // its shm id). It's now single-owner, so destroy it — frees the physical RAM back
                    // to the kernel instead of leaking one buffer per resize step.
                    let rid = msg.data1;
                    if let Some(pos) = pending_retire.iter().position(|&(id, _)| id == rid) {
                        let (_, base) = pending_retire.swap_remove(pos);
                        sys_destroy_shm(rid, base);
                    }
                },
                _ => {}
            }
        }

        // Deliver the coalesced hover before the resize, so a move is reported against the geometry
        // the pointer was actually over rather than against a window that has since changed size.
        match pending_hover {
            Some(Some((mx, my))) => event_redraw |= app.on_mouse_move(mx, my),
            Some(None) => event_redraw |= app.on_mouse_leave(),
            None => {}
        }

        // Apply the coalesced resize ONCE (allocate the new SHM at the final drag size). We still defer
        // notifying the compositor until AFTER the buffer is painted (below) so it never samples a
        // half-painted surface.
        if let Some((w, h)) = pending_resize {
            // R1: remember the OUTGOING buffer (id + its mapping base) so we can destroy it once the
            // compositor ACKs that it has released it. Capture BEFORE we overwrite buffer_ptr/shm_id.
            let old_id = shm_id;
            let old_base = buffer_ptr as u64;

            width = w;
            height = h;
            total_size = core::mem::size_of::<WindowHeader>() + (width * height * 4);
            let new_shm_id = sys_create_shm(total_size);
            buffer_ptr = sys_map_shm(new_shm_id) as *mut u8;
            header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
            header.magic = WIN_MAGIC;
            header.width = width as u32;
            header.height = height as u32;
            pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
            shm_id = new_shm_id;                        // this is now the live buffer
            pending_shm_swap = Some(new_shm_id);
            pending_retire.push((old_id, old_base));    // retire the old one on the compositor's ACK
            needs_redraw = true;
        }

        let update_redraw = app.update();
        
        if needs_redraw || event_redraw || update_redraw {
            let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, width * height) };
            let mut canvas = Canvas::new(screen, width, height);
            
            // 1. Fully paint the buffer
            app.draw(&mut canvas);

            // 1b. Report scroll state into the header so the compositor can draw/drive the scrollbar.
            header.content_h = app.content_height() as u32;
            header.scroll_off = app.scroll_offset() as u32;

            // 2. NOW safely tell the Compositor the buffer is ready
            if let Some(swap_id) = pending_shm_swap {
                sys_ipc_send(COMPOSITOR_PID, MSG_WINDOW_UPDATE_SHM, swap_id, 0);
                pending_shm_swap = None;
            } else {
                // If it wasn't a resize event, just flush a normal frame update
                sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            }
            
            needs_redraw = false;
        }
        
        sys_sleep_ms(16);
    }
}