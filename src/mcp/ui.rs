//! MCP in the editor UI: a status chip in the top bar and the "Connect your agent" panel
//! (on/off, port, copyable setup for Claude Code, Claude Desktop and other clients, recent calls).

use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;

use super::McpServer;
use crate::ui::theme::*;
use crate::ui::widgets::*;

const GREEN: Color32 = OK;
const AMBER: Color32 = WARN;

fn ago(now: f32, at: f32) -> String {
    let s = (now - at).max(0.0);
    if s < 60.0 {
        format!("{s:.0} s ago")
    } else {
        format!("{:.0} min ago", s / 60.0)
    }
}

/// Top-bar chip: a dot (green listening, amber problem, grey off) and "MCP"; click opens the panel.
/// The MCP status for the actions island: (dot colour, tooltip, listening).
pub fn status_info(s: &McpServer, now: f32) -> (Color32, String, bool) {
    let listening = s.enabled && s.error.is_none() && s.running_port.is_some();
    let (dot, tip) = if !s.enabled {
        (
            TEXT_DIM,
            "MCP is off: agents can't use this editor. Click to set it up.".to_string(),
        )
    } else if let Some(e) = &s.error {
        (
            AMBER,
            format!("MCP couldn't start: {e}. Click for details."),
        )
    } else {
        let last = s
            .recent
            .front()
            .map(|c| format!(" · last call {} {}", c.name, ago(now, c.at)))
            .unwrap_or_default();
        (
            GREEN,
            format!(
                "MCP on · 127.0.0.1:{} · {} tools{last}\nAgents (Claude Code, Claude Desktop, Cursor…) can build in this editor. Click for connect instructions.",
                s.port, s.tool_count
            ),
        )
    };
    (dot, tip, listening)
}

/// A copyable block of text.
fn code_block(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(9, 12, 18))
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(text).monospace().size(12.0).color(TEXT)).wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(icon::COPY).size(14.0)).frame(false))
                        .on_hover_text("Copy")
                        .clicked()
                    {
                        ui.ctx().copy_text(text.to_string());
                    }
                });
            });
        });
}

/// The "Connect your agent" panel.
pub fn connect_window(ctx: &egui::Context, s: &mut McpServer, now: f32) {
    let mut open = true;
    egui::Window::new(
        RichText::new("Connect your agent (MCP)").font(FontId::new(17.0, semibold())),
    )
    .collapsible(false)
    .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
    .auto_sized()
    .min_width(560.0)
    .max_width(560.0)
    .open(&mut open)
    .show(ctx, |ui| {
        egui::ScrollArea::vertical()
            .max_height(ctx.content_rect().height() - 140.0)
            .auto_shrink([false, true])
            .show(ui, |ui| panel(ui, s, now));
    });
    if !open {
        s.show_panel = false;
    }
}

/// The panel's content (scrolls when the window is small).
fn panel(ui: &mut egui::Ui, s: &mut McpServer, now: f32) {
    dim(
        ui,
        "AI agents can build in this editor through the Model Context Protocol: sculpt, paint, water, nature, objects, zones, lighting, screenshots and more. Their edits are undoable like yours. Only programs on this computer can connect.",
    );
    ui.add_space(6.0);
    let mut on = s.enabled;
    if toggle(
        ui,
        &mut on,
        RichText::new("Let agents use this editor").color(TEXT),
    )
    .changed()
    {
        s.set_enabled(on);
    }
    ui.horizontal(|ui| {
        let (dot, line) = if !s.enabled {
            (TEXT_DIM, "Off".to_string())
        } else if let Some(e) = &s.error {
            (AMBER, format!("Not running: {e}"))
        } else {
            (
                GREEN,
                format!(
                    "Running at {} · {} tools · {} call{}",
                    s.url(),
                    s.tool_count,
                    s.calls,
                    if s.calls == 1 { "" } else { "s" }
                ),
            )
        };
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, dot);
        ui.label(RichText::new(line).color(TEXT));
        if s.enabled && s.error.is_some() && ui.small_button("Retry").clicked() {
            s.running_port = None;
            s.error = None;
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Port").color(TEXT_DIM));
        ui.add(egui::DragValue::new(&mut s.port_edit).range(1024..=65535));
        if s.port_edit != s.port && ui.small_button("Apply").clicked() {
            let p = s.port_edit;
            s.set_port(p);
        }
    });
    separator(ui);
    section(ui, "Claude Code");
    code_block(
        ui,
        &format!(
            "claude mcp add --transport http world-builder {} --header \"Authorization: {}\"",
            s.url(),
            super::key::bearer()
        ),
    );
    section(ui, "Claude Desktop");
    dim(
        ui,
        "Settings › Developer › Edit Config, add this to claude_desktop_config.json and restart Claude. It starts the editor if it isn't running.",
    );
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "world_builder".into());
    let port = if s.port != super::DEFAULT_PORT {
        format!(", \"--port\", \"{}\"", s.port)
    } else {
        String::new()
    };
    let exe = serde_json::Value::String(exe).to_string();
    code_block(
        ui,
        &format!(
            "{{\n  \"mcpServers\": {{\n    \"world-builder\": {{\n      \"command\": {exe},\n      \"args\": [\"--mcp-stdio\"{port}]\n    }}\n  }}\n}}"
        ),
    );
    section(ui, "Cursor, VS Code and other clients");
    dim(
        ui,
        "Clients that speak Streamable HTTP connect to the URL with this key; the others use the command above. Keep the key private: it lets a program drive this editor.",
    );
    code_block(
        ui,
        &format!(
            "{{\n  \"mcpServers\": {{\n    \"world-builder\": {{\n      \"url\": \"{}\",\n      \"headers\": {{ \"Authorization\": \"{}\" }}\n    }}\n  }}\n}}",
            s.url(),
            super::key::bearer()
        ),
    );
    if !s.recent.is_empty() {
        separator(ui);
        section(ui, "Recent calls");
        for c in s.recent.iter().take(6) {
            let row = ui.horizontal(|ui| {
                let (ic, col) = if c.ok {
                    (icon::CHECK_CIRCLE, GREEN)
                } else {
                    (icon::WARNING_CIRCLE, AMBER)
                };
                ui.label(RichText::new(ic).color(col));
                ui.label(RichText::new(&c.name).monospace().size(12.0).color(TEXT));
                ui.label(
                    RichText::new(format!("{:.0} ms · {}", c.ms, ago(now, c.at)))
                        .small()
                        .color(TEXT_DIM),
                );
            });
            if let Some(e) = &c.error {
                row.response.on_hover_text(e);
            }
        }
    }
}
