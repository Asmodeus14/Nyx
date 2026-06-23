#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};
// Import the new widgets!
use nyx_gui::ui::{Widget, Button, CheckBox, Menu, TextBox, Label};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[derive(PartialEq, Clone, Copy)]
enum SettingsTab { Display, Personalization, System, Security }

struct SettingsApp {
    active_tab: SettingsTab,
    
    // Sidebar Navigation Widgets
    btn_display: Button,
    btn_personalization: Button,
    btn_system: Button,
    btn_security: Button,

    // --- Personalization Tab Widgets ---
    chk_animations: CheckBox,
    chk_dark_mode: CheckBox,

    // --- Display Tab Widgets ---
    menu_scale: Menu,
    txt_resolution: TextBox,
}

impl SettingsApp {
    fn new() -> Self {
        Self {
            active_tab: SettingsTab::Display,
            
            // Sidebar Buttons
            btn_display: Button { x: 10, y: 65, w: 160, h: 30, text: String::from("Display"), is_hovered: false, is_pressed: false },
            btn_personalization: Button { x: 10, y: 105, w: 160, h: 30, text: String::from("Personalization"), is_hovered: false, is_pressed: false },
            btn_system: Button { x: 10, y: 145, w: 160, h: 30, text: String::from("System Info"), is_hovered: false, is_pressed: false },
            btn_security: Button { x: 10, y: 185, w: 160, h: 30, text: String::from("Security"), is_hovered: false, is_pressed: false },

            // Personalization Widgets
            chk_animations: CheckBox { x: 210, y: 80, text: String::from("Enable Window Animations"), is_checked: true },
            chk_dark_mode: CheckBox { x: 210, y: 120, text: String::from("Force Dark Mode UI"), is_checked: false },

            // Display Widgets
            menu_scale: Menu { x: 210, y: 120, w: 150, items: vec![String::from("100%"), String::from("125%"), String::from("150%")], is_open: false, selected_idx: 0 },
            txt_resolution: TextBox { x: 210, y: 80, w: 150, h: 25, text: String::from("1920x1080"), is_focused: false },
        }
    }
}

impl NyxApp for SettingsApp {
    fn title(&self) -> &str { "System Settings" }
    fn initial_width(&self) -> usize { 680 }
    fn initial_height(&self) -> usize { 450 }

    fn draw(&mut self, canvas: &mut Canvas) {
        let width = canvas.width;
        let height = canvas.height;

        canvas.fill_rect(0, 0, width, height, Color::WARM_BG);

        // Sidebar Background
        canvas.fill_rect(0, 0, 180, height, Color::WARM_SURFACE);
        canvas.fill_rect(180, 0, 1, height, Color::WARM_BORDER);
        canvas.print_str(15, 20, "SETTINGS", Color::TEXT_DARK, 2);

        // 1. Draw Sidebar Widgets
        self.btn_display.draw(canvas);
        self.btn_personalization.draw(canvas);
        self.btn_system.draw(canvas);
        self.btn_security.draw(canvas);

        // 2. Draw Active Tab Content
        let cx = 210;
        match self.active_tab {
            SettingsTab::Display => {
                canvas.print_str(cx, 30, "Display Settings", Color::TEXT_DARK, 2);
                canvas.fill_rect(cx, 60, width - cx - 30, 1, Color::WARM_BORDER);
                
                canvas.print_str(cx, 160, "Global Scale Factor", Color::TEXT_DARK, 1);
                
                // Draw display widgets
                self.txt_resolution.draw(canvas);
                self.menu_scale.draw(canvas); // Draw menu last so it overlaps everything else
            },
            SettingsTab::Personalization => {
                canvas.print_str(cx, 30, "Personalization", Color::TEXT_DARK, 2);
                canvas.fill_rect(cx, 60, width - cx - 30, 1, Color::WARM_BORDER);

                // Draw personalization widgets
                self.chk_animations.draw(canvas);
                self.chk_dark_mode.draw(canvas);
            },
            SettingsTab::System => {
                canvas.print_str(cx, 30, "System Specifications", Color::TEXT_DARK, 2);
                canvas.fill_rect(cx, 60, width - cx - 30, 1, Color::WARM_BORDER);
                canvas.print_str(cx, 80, "OS: NyxOS v0.1 (Lethe Build)", Color::TEXT_DARK, 1);
                canvas.print_str(cx, 110, "Architecture: x86_64", Color::TEXT_DARK, 1);
                canvas.print_str(cx, 140, "Window Server: Nyx Compositor Phase 3", Color::TEXT_DARK, 1);
            },
            _ => {
                canvas.print_str(cx, 30, "Module Pending", Color::TEXT_DARK, 2);
            }
        }
    }

    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut needs_redraw = false;

        // 1. Pass events to Sidebar Buttons
        needs_redraw |= self.btn_display.on_mouse(mx, my, clicked);
        needs_redraw |= self.btn_personalization.on_mouse(mx, my, clicked);
        needs_redraw |= self.btn_system.on_mouse(mx, my, clicked);
        needs_redraw |= self.btn_security.on_mouse(mx, my, clicked);

        // Map button presses to state changes
        if clicked {
            if self.btn_display.is_pressed { self.active_tab = SettingsTab::Display; }
            if self.btn_personalization.is_pressed { self.active_tab = SettingsTab::Personalization; }
            if self.btn_system.is_pressed { self.active_tab = SettingsTab::System; }
            if self.btn_security.is_pressed { self.active_tab = SettingsTab::Security; }
        }

        // 2. Pass events to active tab widgets
        if self.active_tab == SettingsTab::Personalization {
            needs_redraw |= self.chk_animations.on_mouse(mx, my, clicked);
            needs_redraw |= self.chk_dark_mode.on_mouse(mx, my, clicked);
        } else if self.active_tab == SettingsTab::Display {
            // Priority: Pass to menu first, because if it's open, it swallows clicks!
            needs_redraw |= self.menu_scale.on_mouse(mx, my, clicked);
            if !self.menu_scale.is_open {
                needs_redraw |= self.txt_resolution.on_mouse(mx, my, clicked);
            }
        }

        needs_redraw
    }

    fn on_key(&mut self, key: char) -> bool {
        let mut needs_redraw = false;
        
        // Pass keyboard events to the focused active tab widgets
        if self.active_tab == SettingsTab::Display {
            needs_redraw |= self.txt_resolution.on_key(key);
        }
        
        needs_redraw
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    nyx_gui::app::run(SettingsApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }