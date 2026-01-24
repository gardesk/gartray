//! Menu popup rendering for SNI context menus
//!
//! Renders DBusMenu items in an X11 popup window.

use anyhow::Result;
use cairo::{Context, Format, ImageSurface};
use gartk_x11::{Connection, Window, WindowConfig};
use x11rb::protocol::xproto::{self, ConnectionExt, EventMask};
use x11rb::protocol::Event;
use tracing::{debug, warn};

use super::menu::MenuItem;

/// Menu item dimensions
const ITEM_HEIGHT: i32 = 28;
const ITEM_PADDING_X: i32 = 12;
const SEPARATOR_HEIGHT: i32 = 8;
const MENU_PADDING: i32 = 4;
const CHECKBOX_SIZE: i32 = 14;
const SUBMENU_ARROW_SIZE: i32 = 8;

/// Colors
const BG_COLOR: (f64, f64, f64) = (0.12, 0.12, 0.12);
const HOVER_COLOR: (f64, f64, f64) = (0.22, 0.22, 0.22);
const TEXT_COLOR: (f64, f64, f64) = (0.9, 0.9, 0.9);
const DISABLED_COLOR: (f64, f64, f64) = (0.5, 0.5, 0.5);
const SEPARATOR_COLOR: (f64, f64, f64) = (0.3, 0.3, 0.3);
const CHECK_COLOR: (f64, f64, f64) = (0.3, 0.6, 1.0);

/// Result of menu interaction
#[derive(Debug, Clone)]
pub enum MenuAction {
    /// No action taken
    None,
    /// User clicked a menu item
    ItemClicked(i32),
    /// User opened a submenu
    SubmenuOpened(i32),
    /// Menu was dismissed
    Dismissed,
}

/// A popup menu window
pub struct MenuPopup {
    conn: Connection,
    window: Window,
    surface: Option<ImageSurface>,
    items: Vec<MenuItem>,
    item_rects: Vec<ItemRect>,
    hover_index: Option<usize>,
    width: u32,
    height: u32,
    visible: bool,
}

/// Rectangle for hit testing
#[derive(Debug, Clone)]
struct ItemRect {
    y: i32,
    height: i32,
    item_id: i32,
    has_submenu: bool,
    enabled: bool,
}

impl MenuPopup {
    /// Create a new menu popup (window created but hidden)
    pub fn new() -> Result<Self> {
        let conn = Connection::connect(None)?;

        // Create popup window (initially unmapped, will be positioned later)
        let window = Window::create(
            conn.clone(),
            WindowConfig::new()
                .title("gartray-menu")
                .class("gartray")
                .size(200, 100)
                .position(0, 0)
                .override_redirect(true)
                .transparent(true)
                .map_on_create(false),
        )?;

        // Subscribe to events
        conn.inner().change_window_attributes(
            window.id(),
            &xproto::ChangeWindowAttributesAux::new()
                .event_mask(
                    EventMask::EXPOSURE |
                    EventMask::BUTTON_PRESS |
                    EventMask::BUTTON_RELEASE |
                    EventMask::POINTER_MOTION |
                    EventMask::LEAVE_WINDOW |
                    EventMask::KEY_PRESS
                ),
        )?;

        Ok(Self {
            conn,
            window,
            surface: None,
            items: Vec::new(),
            item_rects: Vec::new(),
            hover_index: None,
            width: 200,
            height: 100,
            visible: false,
        })
    }

    /// Show the menu at position with given items
    pub fn show(&mut self, x: i32, y: i32, items: Vec<MenuItem>) -> Result<()> {
        self.items = items;
        self.hover_index = None;
        self.item_rects.clear();

        // Calculate menu size
        let (width, height) = self.calculate_size();
        self.width = width;
        self.height = height;

        // Adjust position to stay on screen
        let screen = self.conn.screen();
        let screen_width = screen.width_in_pixels as i32;
        let screen_height = screen.height_in_pixels as i32;

        let final_x = if x + width as i32 > screen_width {
            (screen_width - width as i32).max(0)
        } else {
            x
        };

        let final_y = if y + height as i32 > screen_height {
            (screen_height - height as i32).max(0)
        } else {
            y
        };

        // Resize and move window
        self.conn.inner().configure_window(
            self.window.id(),
            &xproto::ConfigureWindowAux::new()
                .x(final_x)
                .y(final_y)
                .width(width)
                .height(height)
                .stack_mode(xproto::StackMode::ABOVE),
        )?;

        // Create surface
        self.surface = Some(
            ImageSurface::create(Format::ARgb32, width as i32, height as i32)
                .map_err(|e| anyhow::anyhow!("Failed to create surface: {}", e))?
        );

        // Render and show
        self.render()?;

        self.conn.inner().map_window(self.window.id())?;
        self.conn.inner().set_input_focus(
            xproto::InputFocus::PARENT,
            self.window.id(),
            x11rb::CURRENT_TIME,
        )?;
        self.conn.flush()?;

        self.visible = true;
        debug!("Menu popup shown at ({}, {}) with {} items", final_x, final_y, self.items.len());

        Ok(())
    }

    /// Hide the menu
    pub fn hide(&mut self) -> Result<()> {
        if self.visible {
            self.conn.inner().unmap_window(self.window.id())?;
            self.conn.flush()?;
            self.visible = false;
            debug!("Menu popup hidden");
        }
        Ok(())
    }

    /// Check if menu is visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Process events, returns action if any
    pub fn process_events(&mut self) -> Result<MenuAction> {
        while let Some(event) = self.conn.poll_event()? {
            let action = self.handle_event(event)?;
            if !matches!(action, MenuAction::None) {
                return Ok(action);
            }
        }
        Ok(MenuAction::None)
    }

    /// Handle a single event
    fn handle_event(&mut self, event: Event) -> Result<MenuAction> {
        match event {
            Event::Expose(e) if e.window == self.window.id() => {
                self.render()?;
            }
            Event::ButtonPress(e) if e.event == self.window.id() => {
                if e.detail == 1 {
                    // Left click
                    if let Some(idx) = self.hit_test(e.event_y as i32) {
                        let rect = &self.item_rects[idx];
                        if rect.enabled {
                            if rect.has_submenu {
                                return Ok(MenuAction::SubmenuOpened(rect.item_id));
                            } else {
                                return Ok(MenuAction::ItemClicked(rect.item_id));
                            }
                        }
                    }
                } else if e.detail == 3 {
                    // Right click dismisses
                    return Ok(MenuAction::Dismissed);
                }
            }
            Event::MotionNotify(e) if e.event == self.window.id() => {
                let new_hover = self.hit_test(e.event_y as i32);
                if new_hover != self.hover_index {
                    self.hover_index = new_hover;
                    self.render()?;
                }
            }
            Event::LeaveNotify(e) if e.event == self.window.id() => {
                if self.hover_index.is_some() {
                    self.hover_index = None;
                    self.render()?;
                }
            }
            Event::KeyPress(e) if e.event == self.window.id() => {
                // ESC to dismiss
                if e.detail == 9 {
                    return Ok(MenuAction::Dismissed);
                }
                // Enter to activate
                if e.detail == 36 {
                    if let Some(idx) = self.hover_index {
                        let rect = &self.item_rects[idx];
                        if rect.enabled && !rect.has_submenu {
                            return Ok(MenuAction::ItemClicked(rect.item_id));
                        }
                    }
                }
                // Up arrow
                if e.detail == 111 {
                    self.move_hover(-1);
                    self.render()?;
                }
                // Down arrow
                if e.detail == 116 {
                    self.move_hover(1);
                    self.render()?;
                }
            }
            Event::FocusOut(e) if e.event == self.window.id() => {
                // Lost focus - dismiss
                return Ok(MenuAction::Dismissed);
            }
            _ => {}
        }
        Ok(MenuAction::None)
    }

    /// Move hover selection
    fn move_hover(&mut self, delta: i32) {
        if self.item_rects.is_empty() {
            return;
        }

        let current = self.hover_index.unwrap_or(0) as i32;
        let mut new_idx = current + delta;

        // Wrap around
        let len = self.item_rects.len() as i32;
        if new_idx < 0 {
            new_idx = len - 1;
        } else if new_idx >= len {
            new_idx = 0;
        }

        // Skip disabled items and separators
        let start_idx = new_idx;
        loop {
            if self.item_rects[new_idx as usize].enabled {
                break;
            }
            new_idx = (new_idx + delta.signum()).rem_euclid(len);
            if new_idx == start_idx {
                // All items disabled
                return;
            }
        }

        self.hover_index = Some(new_idx as usize);
    }

    /// Hit test - returns item index at y position
    fn hit_test(&self, y: i32) -> Option<usize> {
        for (idx, rect) in self.item_rects.iter().enumerate() {
            if y >= rect.y && y < rect.y + rect.height {
                return Some(idx);
            }
        }
        None
    }

    /// Calculate required size for menu
    fn calculate_size(&self) -> (u32, u32) {
        let mut width: u32 = 150;  // minimum width
        let mut height: u32 = MENU_PADDING as u32 * 2;

        for item in &self.items {
            if !item.visible {
                continue;
            }

            if item.item_type == "separator" {
                height += SEPARATOR_HEIGHT as u32;
            } else {
                height += ITEM_HEIGHT as u32;

                // Calculate text width
                if let Some(ref label) = item.label {
                    let text_width = (label.len() * 8) as u32 + ITEM_PADDING_X as u32 * 2;
                    width = width.max(text_width + 40);  // Extra space for checkbox/arrow
                }
            }
        }

        (width.max(100), height.max(20))
    }

    /// Render the menu
    fn render(&mut self) -> Result<()> {
        let surface = self.surface.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        let ctx = Context::new(surface)
            .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

        // Clear with background
        ctx.set_source_rgb(BG_COLOR.0, BG_COLOR.1, BG_COLOR.2);
        ctx.paint().ok();

        // Draw border
        ctx.set_source_rgb(0.3, 0.3, 0.3);
        ctx.set_line_width(1.0);
        ctx.rectangle(0.5, 0.5, self.width as f64 - 1.0, self.height as f64 - 1.0);
        ctx.stroke().ok();

        // Build item rects and render items
        self.item_rects.clear();
        let mut y = MENU_PADDING;

        for (idx, item) in self.items.iter().enumerate() {
            if !item.visible {
                continue;
            }

            if item.item_type == "separator" {
                // Draw separator
                let sep_y = y + SEPARATOR_HEIGHT / 2;
                ctx.set_source_rgb(SEPARATOR_COLOR.0, SEPARATOR_COLOR.1, SEPARATOR_COLOR.2);
                ctx.move_to(ITEM_PADDING_X as f64, sep_y as f64);
                ctx.line_to((self.width as i32 - ITEM_PADDING_X) as f64, sep_y as f64);
                ctx.stroke().ok();

                self.item_rects.push(ItemRect {
                    y,
                    height: SEPARATOR_HEIGHT,
                    item_id: item.id,
                    has_submenu: false,
                    enabled: false,
                });

                y += SEPARATOR_HEIGHT;
            } else {
                let is_hover = self.hover_index == Some(self.item_rects.len());
                let has_submenu = !item.children.is_empty();

                // Draw hover background
                if is_hover && item.enabled {
                    ctx.set_source_rgb(HOVER_COLOR.0, HOVER_COLOR.1, HOVER_COLOR.2);
                    ctx.rectangle(
                        1.0,
                        y as f64,
                        self.width as f64 - 2.0,
                        ITEM_HEIGHT as f64,
                    );
                    ctx.fill().ok();
                }

                // Draw checkbox/radio if toggle type
                let text_x = if item.toggle_type.is_some() {
                    let check_x = ITEM_PADDING_X;
                    let check_y = y + (ITEM_HEIGHT - CHECKBOX_SIZE) / 2;

                    if item.toggle_state == 1 {
                        ctx.set_source_rgb(CHECK_COLOR.0, CHECK_COLOR.1, CHECK_COLOR.2);
                    } else {
                        ctx.set_source_rgb(DISABLED_COLOR.0, DISABLED_COLOR.1, DISABLED_COLOR.2);
                    }

                    if item.toggle_type.as_deref() == Some("radio") {
                        // Draw radio circle
                        let cx = check_x as f64 + CHECKBOX_SIZE as f64 / 2.0;
                        let cy = check_y as f64 + CHECKBOX_SIZE as f64 / 2.0;
                        ctx.arc(cx, cy, CHECKBOX_SIZE as f64 / 2.0 - 2.0, 0.0, 2.0 * std::f64::consts::PI);
                        if item.toggle_state == 1 {
                            ctx.fill().ok();
                        } else {
                            ctx.stroke().ok();
                        }
                    } else {
                        // Draw checkbox
                        ctx.rectangle(
                            check_x as f64,
                            check_y as f64,
                            CHECKBOX_SIZE as f64,
                            CHECKBOX_SIZE as f64,
                        );
                        if item.toggle_state == 1 {
                            ctx.fill().ok();
                        } else {
                            ctx.stroke().ok();
                        }
                    }

                    ITEM_PADDING_X + CHECKBOX_SIZE + 8
                } else {
                    ITEM_PADDING_X
                };

                // Draw label
                if item.enabled {
                    ctx.set_source_rgb(TEXT_COLOR.0, TEXT_COLOR.1, TEXT_COLOR.2);
                } else {
                    ctx.set_source_rgb(DISABLED_COLOR.0, DISABLED_COLOR.1, DISABLED_COLOR.2);
                }

                ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
                ctx.set_font_size(13.0);

                let text = item.label.as_deref().unwrap_or("");
                let text_y = y + ITEM_HEIGHT / 2 + 4;
                ctx.move_to(text_x as f64, text_y as f64);
                ctx.show_text(text).ok();

                // Draw submenu arrow
                if has_submenu {
                    let arrow_x = self.width as i32 - ITEM_PADDING_X - SUBMENU_ARROW_SIZE;
                    let arrow_y = y + ITEM_HEIGHT / 2;

                    ctx.move_to(arrow_x as f64, (arrow_y - 4) as f64);
                    ctx.line_to((arrow_x + 6) as f64, arrow_y as f64);
                    ctx.line_to(arrow_x as f64, (arrow_y + 4) as f64);
                    ctx.close_path();
                    ctx.fill().ok();
                }

                self.item_rects.push(ItemRect {
                    y,
                    height: ITEM_HEIGHT,
                    item_id: item.id,
                    has_submenu,
                    enabled: item.enabled,
                });

                y += ITEM_HEIGHT;
            }
        }

        // Copy to window
        self.copy_to_window()?;

        Ok(())
    }

    /// Copy surface to window
    fn copy_to_window(&mut self) -> Result<()> {
        let surface = self.surface.as_mut()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        surface.flush();

        let data = {
            let data_ref = surface.data()
                .map_err(|e| anyhow::anyhow!("Failed to get surface data: {}", e))?;
            data_ref.to_vec()
        };

        let gc = self.conn.generate_id()?;
        self.conn.inner().create_gc(gc, self.window.id(), &Default::default())?;

        self.conn.inner().put_image(
            xproto::ImageFormat::Z_PIXMAP,
            self.window.id(),
            gc,
            self.width as u16,
            self.height as u16,
            0,
            0,
            0,
            self.window.depth(),
            &data,
        )?;

        self.conn.inner().free_gc(gc)?;
        self.conn.flush()?;

        Ok(())
    }
}
