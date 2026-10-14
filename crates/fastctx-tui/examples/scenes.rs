//! Draws every scene of `fastctx_tui::preview` into cell dumps, for turning
//! into images:
//! `cargo run -p fastctx-tui --features preview --example scenes -- <dir> [zh|en]`.

use std::path::PathBuf;

use fastctx_tui::app::Ctx;
use fastctx_tui::design::{Depth, Look, Theme};
use fastctx_tui::words::{Lang, Words};
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "scenes".into()));
    let lang = match args.next().as_deref() {
        Some("en") => Lang::En,
        _ => Lang::Zh,
    };
    std::fs::create_dir_all(&dir)?;
    let looks = [
        ("warm-black", Look { depth: Depth::TrueColor, theme: Theme::WarmBlack }),
        ("ink-black", Look { depth: Depth::TrueColor, theme: Theme::InkBlack }),
        ("warm-white", Look { depth: Depth::TrueColor, theme: Theme::WarmWhite }),
        ("256", Look { depth: Depth::Ansi256, theme: Theme::WarmBlack }),
        ("ansi16", Look { depth: Depth::Ansi16, theme: Theme::WarmBlack }),
        ("mono", Look { depth: Depth::Mono, theme: Theme::WarmBlack }),
    ];
    let sizes = [(120u16, 36u16), (84, 28)];
    for (look_name, look) in looks {
        for (width, height) in sizes {
            // The breath half way, as a busy mark would be at some moment.
            // Every scene, so conversations are on.
            let ctx = Ctx {
                look,
                words: Words::new(lang),
                now: Timestamp::now(),
                breath: 0.25,
                frame: 0,
                conversations: true,
            };
            // The first screen, asking the language, before any page.
            let mut scenes: Vec<(String, Option<fastctx_tui::pages::Page>)> = vec![("language".into(), None)];
            scenes.extend(
                fastctx_tui::preview::scenes(&ctx).into_iter().map(|(name, page)| (name.to_string(), Some(page))),
            );
            for (name, mut page) in scenes {
                let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                terminal.draw(|frame| match page.as_mut() {
                    Some(page) => {
                        let area = frame.area();
                        page.draw(&ctx, frame, area, None);
                    }
                    None => fastctx_tui::language::draw(frame, &look, lang, 0),
                })?;
                let cursor = terminal.get_cursor_position().ok();
                let shown = terminal.backend().cursor_visible();
                let buffer = terminal.backend().buffer();
                let cells: Vec<serde_json::Value> = buffer
                    .content
                    .iter()
                    .enumerate()
                    .map(|(index, cell)| {
                        let (x, y) = ((index as u16) % width, (index as u16) / width);
                        let caret = shown && cursor.is_some_and(|at| at.x == x && at.y == y);
                        serde_json::json!({
                            "text": cell.symbol(),
                            "fg": format!("{:?}", cell.fg),
                            "bg": format!("{:?}", cell.bg),
                            "reverse": cell.modifier.contains(Modifier::REVERSED) || caret,
                            "bold": cell.modifier.contains(Modifier::BOLD),
                            "underline": cell.modifier.contains(Modifier::UNDERLINED),
                            "dim": cell.modifier.contains(Modifier::DIM),
                        })
                    })
                    .collect();
                let dump = serde_json::json!({ "width": width, "height": height, "cells": cells });
                let file = dir.join(format!("{name}-{look_name}-{width}x{height}.json"));
                std::fs::write(file, serde_json::to_vec(&dump)?)?;
            }
        }
    }
    Ok(())
}
